use std::{collections::HashMap, future::Future, path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::{FromRequest, Multipart, Request, State},
    http::{header, HeaderMap, HeaderValue, Response, StatusCode, Uri},
    response::IntoResponse,
};
use lofty::{file::TaggedFileExt, probe::Probe, tag::Accessor};
use md5::{Digest, Md5};
use ncm_api_rs::{ApiResponse, NcmError, Query};
use serde::Deserialize;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio_util::io::ReaderStream;

const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;
const BUCKET: &str = "jd-musicrep-privatecloud-audio-public";
// Leave a small envelope above the 360-second sum of the stage deadlines.
const CLOUD_REQUEST_TIMEOUT: Duration = Duration::from_secs(370);
const MULTIPART_UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const API_STAGE_TIMEOUT: Duration = Duration::from_secs(30);
const NOS_UPLOAD_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_CONCURRENT_CLOUD_UPLOADS: usize = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioMetadata {
    pub title: String,
    pub album: String,
    pub artist: String,
}

#[derive(Clone, Debug)]
pub struct UploadFile {
    pub path: std::path::PathBuf,
    pub filename: String,
    pub extension: String,
    pub size: u64,
    pub md5: String,
    pub metadata: AudioMetadata,
}

#[derive(Clone, Debug)]
pub struct CloudContext {
    pub cookie: Option<String>,
    pub proxy: Option<String>,
    pub real_ip: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UploadCheck {
    pub response: ApiResponse,
    pub need_upload: bool,
    pub song_id: String,
}

#[derive(Clone, Debug)]
pub struct UploadToken {
    pub token: String,
    pub object_key: String,
    pub resource_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("{stage} failed: {detail}")]
    Stage { stage: &'static str, detail: String },
    #[error("{stage} failed: {source}")]
    Ncm {
        stage: &'static str,
        #[source]
        source: NcmError,
    },
    #[error("upload is missing songFile")]
    MissingFile,
    #[error("upload exceeds 512 MiB")]
    TooLarge,
    #[error("multipart upload timed out")]
    UploadTimeout,
    #[error("cloud upload request timed out")]
    RequestTimeout,
    #[error("invalid multipart upload: {0}")]
    Multipart(String),
    #[error("temporary file failed: {0}")]
    Io(#[from] std::io::Error),
}

impl CloudError {
    fn stage(stage: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Stage {
            stage,
            detail: error.to_string(),
        }
    }

    fn ncm(stage: &'static str, source: NcmError) -> Self {
        Self::Ncm { stage, source }
    }

    fn response_status(&self) -> StatusCode {
        match self {
            Self::MissingFile | Self::Multipart(_) => StatusCode::BAD_REQUEST,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UploadTimeout => StatusCode::REQUEST_TIMEOUT,
            Self::RequestTimeout => StatusCode::GATEWAY_TIMEOUT,
            Self::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Ncm {
                source: NcmError::AuthRequired(_),
                ..
            } => StatusCode::UNAUTHORIZED,
            Self::Stage { .. } | Self::Ncm { .. } => StatusCode::BAD_GATEWAY,
        }
    }

    fn log_stage(&self) -> &'static str {
        match self {
            Self::Stage { stage, .. } | Self::Ncm { stage, .. } => stage,
            Self::MissingFile => "missing file",
            Self::TooLarge => "size limit",
            Self::UploadTimeout => "multipart timeout",
            Self::RequestTimeout => "request timeout",
            Self::Multipart(_) => "multipart",
            Self::Io(_) => "temporary file",
        }
    }
}

#[async_trait]
pub trait CloudApi: Send + Sync {
    async fn check(
        &self,
        context: &CloudContext,
        file: &UploadFile,
    ) -> Result<UploadCheck, CloudError>;
    async fn allocate_token(
        &self,
        context: &CloudContext,
        file: &UploadFile,
    ) -> Result<UploadToken, CloudError>;
    async fn submit_info(
        &self,
        context: &CloudContext,
        file: &UploadFile,
        check_song_id: &str,
        resource_id: &str,
    ) -> Result<String, CloudError>;
    async fn publish(
        &self,
        context: &CloudContext,
        song_id: &str,
    ) -> Result<ApiResponse, CloudError>;
}

#[async_trait]
pub trait NosUploader: Send + Sync {
    async fn upload(&self, file: &UploadFile, token: &UploadToken) -> Result<(), CloudError>;
}

#[derive(Clone)]
pub struct NcmCloudApi {
    client: Arc<ncm_api_rs::ApiClient>,
}

impl NcmCloudApi {
    pub fn new(client: Arc<ncm_api_rs::ApiClient>) -> Self {
        Self { client }
    }
}

fn base_query(context: &CloudContext) -> Query {
    let mut query = Query::new();
    query.cookie.clone_from(&context.cookie);
    query.proxy.clone_from(&context.proxy);
    query.real_ip.clone_from(&context.real_ip);
    query
}

fn string_field<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str, CloudError> {
    let mut current = value;
    for key in path {
        current = current
            .get(*key)
            .ok_or_else(|| CloudError::stage("decode", format!("missing {}", path.join("."))))?;
    }
    current
        .as_str()
        .or_else(|| current.as_i64().map(|_| ""))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CloudError::stage("decode", format!("invalid {}", path.join("."))))
}

fn string_or_number(value: &Value, path: &[&str]) -> Result<String, CloudError> {
    let mut current = value;
    for key in path {
        current = current
            .get(*key)
            .ok_or_else(|| CloudError::stage("decode", format!("missing {}", path.join("."))))?;
    }
    if let Some(value) = current.as_str() {
        Ok(value.to_owned())
    } else if let Some(value) = current.as_i64() {
        Ok(value.to_string())
    } else {
        Err(CloudError::stage(
            "decode",
            format!("invalid {}", path.join(".")),
        ))
    }
}

fn decode_upload_token(body: &Value) -> Result<UploadToken, CloudError> {
    Ok(UploadToken {
        token: string_field(body, &["result", "token"])?.to_owned(),
        object_key: string_field(body, &["result", "objectKey"])?.to_owned(),
        resource_id: string_or_number(body, &["result", "resourceId"])?,
    })
}

#[async_trait]
impl CloudApi for NcmCloudApi {
    async fn check(
        &self,
        context: &CloudContext,
        file: &UploadFile,
    ) -> Result<UploadCheck, CloudError> {
        let mut query = base_query(context);
        query.params.extend([
            ("bitrate".to_owned(), "999000".to_owned()),
            ("fileSize".to_owned(), file.size.to_string()),
            ("md5".to_owned(), file.md5.clone()),
            ("filename".to_owned(), file.filename.clone()),
        ]);
        let response = self
            .client
            .cloud_upload_token_check(&query)
            .await
            .map_err(|error| CloudError::ncm("upload check", error))?;
        let need_upload = response
            .body
            .get("needUpload")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let song_id = string_or_number(&response.body, &["songId"])?;
        Ok(UploadCheck {
            response,
            need_upload,
            song_id,
        })
    }

    async fn allocate_token(
        &self,
        context: &CloudContext,
        file: &UploadFile,
    ) -> Result<UploadToken, CloudError> {
        let mut query = base_query(context);
        query.params.extend([
            ("filename".to_owned(), file.filename.clone()),
            ("md5".to_owned(), file.md5.clone()),
        ]);
        let response = self
            .client
            .cloud_upload_token_alloc(&query)
            .await
            .map_err(|error| CloudError::ncm("NOS token", error))?;
        decode_upload_token(&response.body)
    }

    async fn submit_info(
        &self,
        context: &CloudContext,
        file: &UploadFile,
        check_song_id: &str,
        resource_id: &str,
    ) -> Result<String, CloudError> {
        let mut query = base_query(context);
        query.params.extend([
            ("md5".to_owned(), file.md5.clone()),
            ("songId".to_owned(), check_song_id.to_owned()),
            ("filename".to_owned(), file.filename.clone()),
            ("song".to_owned(), file.metadata.title.clone()),
            ("album".to_owned(), file.metadata.album.clone()),
            ("artist".to_owned(), file.metadata.artist.clone()),
            ("bitrate".to_owned(), "999000".to_owned()),
            ("resourceId".to_owned(), resource_id.to_owned()),
        ]);
        let response = self
            .client
            .cloud_upload_complete_info(&query)
            .await
            .map_err(|error| CloudError::ncm("cloud info", error))?;
        string_or_number(&response.body, &["songId"])
    }

    async fn publish(
        &self,
        context: &CloudContext,
        song_id: &str,
    ) -> Result<ApiResponse, CloudError> {
        let mut query = base_query(context);
        query.params.insert("songId".to_owned(), song_id.to_owned());
        self.client
            .cloud_upload_complete_pub(&query)
            .await
            .map_err(|error| CloudError::ncm("cloud publish", error))
    }
}

#[derive(Clone)]
pub struct HttpNosUploader {
    client: reqwest::Client,
    lbs_url: Arc<str>,
}

impl Default for HttpNosUploader {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
            lbs_url: format!("https://wanproxy.127.net/lbs?version=1.0&bucketname={BUCKET}").into(),
        }
    }
}

#[derive(Deserialize)]
struct LbsResponse {
    upload: Vec<String>,
}

#[async_trait]
impl NosUploader for HttpNosUploader {
    async fn upload(&self, file: &UploadFile, token: &UploadToken) -> Result<(), CloudError> {
        let lbs = self
            .client
            .get(self.lbs_url.as_ref())
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| CloudError::stage("NOS discovery", error))?
            .json::<LbsResponse>()
            .await
            .map_err(|error| CloudError::stage("NOS discovery", error))?;
        let endpoint = lbs
            .upload
            .first()
            .ok_or_else(|| CloudError::stage("NOS discovery", "no upload endpoint"))?;
        let object_key = token.object_key.replacen('/', "%2F", 1);
        let url = format!("{endpoint}/{BUCKET}/{object_key}?offset=0&complete=true&version=1.0");
        let input = tokio::fs::File::open(&file.path).await?;
        self.client
            .post(url)
            .header("x-nos-token", &token.token)
            .header("Content-MD5", &file.md5)
            .header(header::CONTENT_TYPE, "audio/mpeg")
            .header(header::CONTENT_LENGTH, file.size)
            .body(reqwest::Body::wrap_stream(ReaderStream::new(input)))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| CloudError::stage("NOS upload", error))?;
        Ok(())
    }
}

fn merge_bodies(mut first: Value, second: Value) -> Value {
    match (&mut first, second) {
        (Value::Object(first), Value::Object(second)) => {
            first.extend(second);
            Value::Object(std::mem::take(first))
        }
        (_, second) => second,
    }
}

pub async fn execute_upload(
    api: &dyn CloudApi,
    nos: &dyn NosUploader,
    context: &CloudContext,
    file: &UploadFile,
) -> Result<ApiResponse, CloudError> {
    execute_upload_with_timeouts(
        api,
        nos,
        context,
        file,
        API_STAGE_TIMEOUT,
        NOS_UPLOAD_TIMEOUT,
    )
    .await
}

async fn execute_upload_with_timeouts(
    api: &dyn CloudApi,
    nos: &dyn NosUploader,
    context: &CloudContext,
    file: &UploadFile,
    api_timeout: std::time::Duration,
    nos_timeout: std::time::Duration,
) -> Result<ApiResponse, CloudError> {
    let check = tokio::time::timeout(api_timeout, api.check(context, file))
        .await
        .map_err(|_| CloudError::stage("upload check", "timed out"))??;
    let token = tokio::time::timeout(api_timeout, api.allocate_token(context, file))
        .await
        .map_err(|_| CloudError::stage("NOS token", "timed out"))??;
    if check.need_upload {
        tokio::time::timeout(nos_timeout, nos.upload(file, &token))
            .await
            .map_err(|_| CloudError::stage("NOS upload", "timed out"))??;
    }
    let song_id = tokio::time::timeout(
        api_timeout,
        api.submit_info(context, file, &check.song_id, &token.resource_id),
    )
    .await
    .map_err(|_| CloudError::stage("cloud info", "timed out"))??;
    let published = tokio::time::timeout(api_timeout, api.publish(context, &song_id))
        .await
        .map_err(|_| CloudError::stage("cloud publish", "timed out"))??;
    Ok(ApiResponse {
        status: 200,
        body: merge_bodies(check.response.body, published.body),
        cookie: check.response.cookie,
    })
}

async fn within_request_deadline<T>(
    request_timeout: Duration,
    operation: impl Future<Output = Result<T, CloudError>>,
) -> Result<T, CloudError> {
    tokio::time::timeout(request_timeout, operation)
        .await
        .map_err(|_| CloudError::RequestTimeout)?
}

fn repair_filename(filename: &str) -> String {
    if filename.is_ascii() {
        return filename.to_owned();
    }
    let bytes = filename
        .chars()
        .map(|character| u8::try_from(character as u32))
        .collect::<Result<Vec<_>, _>>();
    bytes
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| filename.to_owned())
}

fn file_extension(filename: &str) -> String {
    Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        })
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "mp3".to_owned())
}

fn fallback_title(filename: &str, extension: &str) -> String {
    filename
        .strip_suffix(&format!(".{extension}"))
        .unwrap_or(filename)
        .replace(char::is_whitespace, "")
        .replace('.', "_")
}

fn read_audio_metadata(path: &Path, filename: &str, extension: &str) -> AudioMetadata {
    let tagged = Probe::open(path).and_then(|probe| probe.read()).ok();
    let tag = tagged
        .as_ref()
        .and_then(|file| file.primary_tag().or_else(|| file.first_tag()));
    AudioMetadata {
        title: tag
            .and_then(Accessor::title)
            .map(|value| value.into_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback_title(filename, extension)),
        album: tag
            .and_then(Accessor::album)
            .map(|value| value.into_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "未知专辑".to_owned()),
        artist: tag
            .and_then(Accessor::artist)
            .map(|value| value.into_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "未知艺术家".to_owned()),
    }
}

async fn persist_upload(mut multipart: Multipart) -> Result<(TempDir, UploadFile), CloudError> {
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| CloudError::Multipart(error.to_string()))?
    {
        if !matches!(field.name(), Some("songFile") | Some("file")) {
            continue;
        }
        let filename = repair_filename(field.file_name().unwrap_or("audio.mp3"));
        let extension = file_extension(&filename);
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(format!("upload.{extension}"));
        let mut output = tokio::fs::File::create(&path).await?;
        let mut hasher = Md5::new();
        let mut size = 0_u64;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|error| CloudError::Multipart(error.to_string()))?
        {
            size = size
                .checked_add(chunk.len() as u64)
                .ok_or(CloudError::TooLarge)?;
            if size > MAX_UPLOAD_BYTES {
                return Err(CloudError::TooLarge);
            }
            hasher.update(&chunk);
            output.write_all(&chunk).await?;
        }
        output.flush().await?;
        drop(output);
        let metadata_path = path.clone();
        let metadata_filename = filename.clone();
        let metadata_extension = extension.clone();
        let metadata = tokio::task::spawn_blocking(move || {
            read_audio_metadata(&metadata_path, &metadata_filename, &metadata_extension)
        })
        .await
        .map_err(|error| CloudError::stage("metadata", error))?;
        return Ok((
            directory,
            UploadFile {
                path,
                filename,
                extension,
                size,
                md5: format!("{:x}", hasher.finalize()),
                metadata,
            },
        ));
    }
    Err(CloudError::MissingFile)
}

#[derive(Clone)]
pub struct CloudState {
    api: Arc<dyn CloudApi>,
    nos: Arc<dyn NosUploader>,
    uploads: Arc<Semaphore>,
}

impl CloudState {
    fn with_dependencies(api: Arc<dyn CloudApi>, nos: Arc<dyn NosUploader>) -> Self {
        Self {
            api,
            nos,
            uploads: Arc::new(Semaphore::new(MAX_CONCURRENT_CLOUD_UPLOADS)),
        }
    }

    pub fn production(client: Arc<ncm_api_rs::ApiClient>) -> Self {
        Self::with_dependencies(
            Arc::new(NcmCloudApi::new(client)),
            Arc::new(HttpNosUploader::default()),
        )
    }
}

fn request_context(headers: &HeaderMap, uri: &Uri) -> CloudContext {
    let params = uri
        .query()
        .and_then(|query| serde_urlencoded::from_str::<HashMap<String, String>>(query).ok())
        .unwrap_or_default();
    CloudContext {
        cookie: headers
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        proxy: params.get("proxy").cloned(),
        real_ip: params.get("realIP").cloned(),
    }
}

fn api_response(response: ApiResponse) -> Response<Body> {
    let mut result = Response::new(Body::from(response.body.to_string()));
    *result.status_mut() = StatusCode::from_u16(response.status as u16).unwrap_or(StatusCode::OK);
    result.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    for cookie in response.cookie {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            result.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    result
}

fn cloud_error_response(error: CloudError) -> Response<Body> {
    tracing::warn!(stage = error.log_stage(), "cloud upload failed");
    match error {
        CloudError::Ncm {
            source: NcmError::AuthRequired(_),
            ..
        } => (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({ "code": 301, "msg": "需要登录" })),
        )
            .into_response(),
        CloudError::UploadTimeout => (
            StatusCode::REQUEST_TIMEOUT,
            axum::Json(json!({ "code": 408, "message": "云盘文件上传超时" })),
        )
            .into_response(),
        CloudError::RequestTimeout => (
            StatusCode::GATEWAY_TIMEOUT,
            axum::Json(json!({ "code": 504, "message": "云盘上传超时" })),
        )
            .into_response(),
        error @ (CloudError::MissingFile
        | CloudError::TooLarge
        | CloudError::Multipart(_)
        | CloudError::Io(_)) => (
            error.response_status(),
            axum::Json(json!({
                "code": error.response_status().as_u16(),
                "message": error.to_string(),
            })),
        )
            .into_response(),
        error => (
            error.response_status(),
            axum::Json(json!({ "code": 502, "message": "云盘上传失败" })),
        )
            .into_response(),
    }
}

async fn process_upload(
    state: &CloudState,
    context: &CloudContext,
    multipart: Multipart,
) -> Result<ApiResponse, CloudError> {
    let (_directory, file) =
        tokio::time::timeout(MULTIPART_UPLOAD_TIMEOUT, persist_upload(multipart))
            .await
            .map_err(|_| CloudError::UploadTimeout)??;
    execute_upload(state.api.as_ref(), state.nos.as_ref(), context, &file).await
}

fn upload_busy_response() -> Response<Body> {
    (
        StatusCode::TOO_MANY_REQUESTS,
        axum::Json(json!({
            "code": StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "message": "已有云盘上传正在进行",
        })),
    )
        .into_response()
}

pub async fn upload_song(State(state): State<CloudState>, request: Request) -> Response<Body> {
    let Ok(_upload) = state.uploads.clone().try_acquire_owned() else {
        return upload_busy_response();
    };
    let headers = request.headers().clone();
    let uri = request.uri().clone();
    let multipart = match Multipart::from_request(request, &state).await {
        Ok(multipart) => multipart,
        Err(error) => return error.into_response(),
    };
    let context = request_context(&headers, &uri);
    match within_request_deadline(
        CLOUD_REQUEST_TIMEOUT,
        process_upload(&state, &context, multipart),
    )
    .await
    {
        Ok(response) => api_response(response),
        Err(error) => cloud_error_response(error),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    use axum::{routing::post, Router};
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
    use futures_util::stream;
    use tokio::sync::mpsc;
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn upload_token_accepts_numeric_resource_ids_from_ncm() {
        let token = decode_upload_token(&json!({
            "result": {
                "token": "upload-token",
                "objectKey": "object/key",
                "resourceId": 123456789
            }
        }))
        .unwrap();

        assert_eq!(token.token, "upload-token");
        assert_eq!(token.object_key, "object/key");
        assert_eq!(token.resource_id, "123456789");
    }

    struct FakeCloud {
        events: Mutex<Vec<String>>,
        need_upload: bool,
        fail_at: Option<&'static str>,
        stall_at: Option<&'static str>,
        stall_for: Duration,
    }

    impl Default for FakeCloud {
        fn default() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                need_upload: true,
                fail_at: None,
                stall_at: None,
                stall_for: Duration::ZERO,
            }
        }
    }

    impl FakeCloud {
        fn fail_if_requested(&self, stage: &'static str) -> Result<(), CloudError> {
            if self.fail_at == Some(stage) {
                Err(CloudError::stage(stage, "fixture failure"))
            } else {
                Ok(())
            }
        }

        async fn stall_if_requested(&self, stage: &'static str) {
            if self.stall_at == Some(stage) {
                tokio::time::sleep(self.stall_for).await;
            }
        }
    }

    #[async_trait]
    impl CloudApi for FakeCloud {
        async fn check(&self, _: &CloudContext, _: &UploadFile) -> Result<UploadCheck, CloudError> {
            self.events.lock().unwrap().push("check".to_owned());
            self.stall_if_requested("check").await;
            self.fail_if_requested("check")?;
            Ok(UploadCheck {
                response: ApiResponse {
                    status: 200,
                    body: json!({ "needUpload": self.need_upload, "songId": 7 }),
                    cookie: vec!["MUSIC_U=value".to_owned()],
                },
                need_upload: self.need_upload,
                song_id: "7".to_owned(),
            })
        }

        async fn allocate_token(
            &self,
            _: &CloudContext,
            _: &UploadFile,
        ) -> Result<UploadToken, CloudError> {
            self.events.lock().unwrap().push("token".to_owned());
            self.stall_if_requested("token").await;
            self.fail_if_requested("token")?;
            Ok(UploadToken {
                token: "token".to_owned(),
                object_key: "object/key".to_owned(),
                resource_id: "resource".to_owned(),
            })
        }

        async fn submit_info(
            &self,
            _: &CloudContext,
            file: &UploadFile,
            _: &str,
            _: &str,
        ) -> Result<String, CloudError> {
            assert_eq!(file.metadata.title, "雨夜");
            assert_eq!(file.extension, "flac");
            self.events.lock().unwrap().push("info".to_owned());
            self.stall_if_requested("info").await;
            self.fail_if_requested("info")?;
            Ok("8".to_owned())
        }

        async fn publish(
            &self,
            _: &CloudContext,
            song_id: &str,
        ) -> Result<ApiResponse, CloudError> {
            assert_eq!(song_id, "8");
            self.events.lock().unwrap().push("publish".to_owned());
            self.stall_if_requested("publish").await;
            self.fail_if_requested("publish")?;
            Ok(ApiResponse {
                status: 200,
                body: json!({ "code": 200, "privateCloud": { "songId": 8 } }),
                cookie: vec![],
            })
        }
    }

    struct FakeNos {
        events: Arc<Mutex<Vec<String>>>,
        fail: bool,
    }

    #[async_trait]
    impl NosUploader for FakeNos {
        async fn upload(&self, _: &UploadFile, _: &UploadToken) -> Result<(), CloudError> {
            self.events.lock().unwrap().push("upload".to_owned());
            if self.fail {
                Err(CloudError::stage("NOS upload", "fixture failure"))
            } else {
                Ok(())
            }
        }
    }

    struct StalledCloud;

    #[async_trait]
    impl CloudApi for StalledCloud {
        async fn check(&self, _: &CloudContext, _: &UploadFile) -> Result<UploadCheck, CloudError> {
            std::future::pending().await
        }

        async fn allocate_token(
            &self,
            _: &CloudContext,
            _: &UploadFile,
        ) -> Result<UploadToken, CloudError> {
            unreachable!("the stalled check must time out first")
        }

        async fn submit_info(
            &self,
            _: &CloudContext,
            _: &UploadFile,
            _: &str,
            _: &str,
        ) -> Result<String, CloudError> {
            unreachable!("the stalled check must time out first")
        }

        async fn publish(&self, _: &CloudContext, _: &str) -> Result<ApiResponse, CloudError> {
            unreachable!("the stalled check must time out first")
        }
    }

    struct ExpiredCloud;

    #[async_trait]
    impl CloudApi for ExpiredCloud {
        async fn check(&self, _: &CloudContext, _: &UploadFile) -> Result<UploadCheck, CloudError> {
            Err(CloudError::ncm(
                "upload check",
                NcmError::AuthRequired("expired fixture session".to_owned()),
            ))
        }

        async fn allocate_token(
            &self,
            _: &CloudContext,
            _: &UploadFile,
        ) -> Result<UploadToken, CloudError> {
            unreachable!("authentication must fail before token allocation")
        }

        async fn submit_info(
            &self,
            _: &CloudContext,
            _: &UploadFile,
            _: &str,
            _: &str,
        ) -> Result<String, CloudError> {
            unreachable!("authentication must fail before metadata submission")
        }

        async fn publish(&self, _: &CloudContext, _: &str) -> Result<ApiResponse, CloudError> {
            unreachable!("authentication must fail before publication")
        }
    }

    struct UnusedNos;

    #[async_trait]
    impl NosUploader for UnusedNos {
        async fn upload(&self, _: &UploadFile, _: &UploadToken) -> Result<(), CloudError> {
            unreachable!("authentication must fail before NOS upload")
        }
    }

    struct GatedCloud {
        checks: Arc<AtomicUsize>,
        entered: mpsc::UnboundedSender<()>,
        release: Arc<tokio::sync::Semaphore>,
    }

    #[async_trait]
    impl CloudApi for GatedCloud {
        async fn check(&self, _: &CloudContext, _: &UploadFile) -> Result<UploadCheck, CloudError> {
            self.checks.fetch_add(1, Ordering::SeqCst);
            self.entered
                .send(())
                .map_err(|_| CloudError::stage("fixture", "upload observer closed"))?;
            let permit = self
                .release
                .acquire()
                .await
                .map_err(|_| CloudError::stage("fixture", "upload gate closed"))?;
            permit.forget();
            Err(CloudError::stage("fixture", "upload released"))
        }

        async fn allocate_token(
            &self,
            _: &CloudContext,
            _: &UploadFile,
        ) -> Result<UploadToken, CloudError> {
            unreachable!("the gated check finishes the fixture")
        }

        async fn submit_info(
            &self,
            _: &CloudContext,
            _: &UploadFile,
            _: &str,
            _: &str,
        ) -> Result<String, CloudError> {
            unreachable!("the gated check finishes the fixture")
        }

        async fn publish(&self, _: &CloudContext, _: &str) -> Result<ApiResponse, CloudError> {
            unreachable!("the gated check finishes the fixture")
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct CapturedUpload {
        filename: String,
        extension: String,
        size: u64,
        md5: String,
        metadata: AudioMetadata,
    }

    struct CaptureUploadCloud(Arc<Mutex<Option<CapturedUpload>>>);

    #[async_trait]
    impl CloudApi for CaptureUploadCloud {
        async fn check(
            &self,
            _: &CloudContext,
            file: &UploadFile,
        ) -> Result<UploadCheck, CloudError> {
            *self.0.lock().unwrap() = Some(CapturedUpload {
                filename: file.filename.clone(),
                extension: file.extension.clone(),
                size: file.size,
                md5: file.md5.clone(),
                metadata: file.metadata.clone(),
            });
            Err(CloudError::stage("fixture capture", "complete"))
        }

        async fn allocate_token(
            &self,
            _: &CloudContext,
            _: &UploadFile,
        ) -> Result<UploadToken, CloudError> {
            unreachable!("capture completes during the upload check")
        }

        async fn submit_info(
            &self,
            _: &CloudContext,
            _: &UploadFile,
            _: &str,
            _: &str,
        ) -> Result<String, CloudError> {
            unreachable!("capture completes during the upload check")
        }

        async fn publish(&self, _: &CloudContext, _: &str) -> Result<ApiResponse, CloudError> {
            unreachable!("capture completes during the upload check")
        }
    }

    fn fixture_file(directory: &TempDir) -> UploadFile {
        UploadFile {
            path: directory.path().join("fixture.flac"),
            filename: "雨夜.flac".to_owned(),
            extension: "flac".to_owned(),
            size: 4,
            md5: "fixture-md5".to_owned(),
            metadata: AudioMetadata {
                title: "雨夜".to_owned(),
                album: "夜色".to_owned(),
                artist: "测试歌手".to_owned(),
            },
        }
    }

    #[tokio::test]
    async fn upload_state_machine_preserves_metadata_and_step_order() {
        let api = FakeCloud::default();
        let nos_events = Arc::new(Mutex::new(Vec::new()));
        let nos = FakeNos {
            events: nos_events.clone(),
            fail: false,
        };
        let directory = tempfile::tempdir().unwrap();
        let response = execute_upload(
            &api,
            &nos,
            &CloudContext {
                cookie: None,
                proxy: None,
                real_ip: None,
            },
            &fixture_file(&directory),
        )
        .await
        .unwrap();
        assert_eq!(
            api.events.lock().unwrap().as_slice(),
            ["check", "token", "info", "publish"]
        );
        assert_eq!(nos_events.lock().unwrap().as_slice(), ["upload"]);
        assert_eq!(response.body["privateCloud"]["songId"], 8);
        assert_eq!(response.cookie, ["MUSIC_U=value"]);
    }

    #[tokio::test]
    async fn failed_nos_upload_never_commits_cloud_metadata() {
        let api = FakeCloud::default();
        let nos = FakeNos {
            events: Arc::new(Mutex::new(Vec::new())),
            fail: true,
        };
        let directory = tempfile::tempdir().unwrap();
        assert!(execute_upload(
            &api,
            &nos,
            &CloudContext {
                cookie: None,
                proxy: None,
                real_ip: None,
            },
            &fixture_file(&directory),
        )
        .await
        .is_err());
        assert_eq!(api.events.lock().unwrap().as_slice(), ["check", "token"]);
    }

    #[tokio::test]
    async fn deduplicated_upload_skips_nos_but_still_commits_metadata() {
        let api = FakeCloud {
            need_upload: false,
            ..FakeCloud::default()
        };
        let nos_events = Arc::new(Mutex::new(Vec::new()));
        let nos = FakeNos {
            events: nos_events.clone(),
            fail: false,
        };
        let directory = tempfile::tempdir().unwrap();

        let response = execute_upload(
            &api,
            &nos,
            &CloudContext {
                cookie: None,
                proxy: None,
                real_ip: None,
            },
            &fixture_file(&directory),
        )
        .await
        .unwrap();

        assert!(nos_events.lock().unwrap().is_empty());
        assert_eq!(
            api.events.lock().unwrap().as_slice(),
            ["check", "token", "info", "publish"]
        );
        assert_eq!(response.body["privateCloud"]["songId"], 8);
    }

    #[tokio::test]
    async fn each_failed_api_stage_stops_before_later_side_effects() {
        for (failed_stage, expected_api, expected_nos) in [
            ("check", &["check"][..], &[][..]),
            ("token", &["check", "token"][..], &[][..]),
            ("info", &["check", "token", "info"][..], &["upload"][..]),
            (
                "publish",
                &["check", "token", "info", "publish"][..],
                &["upload"][..],
            ),
        ] {
            let api = FakeCloud {
                fail_at: Some(failed_stage),
                ..FakeCloud::default()
            };
            let nos_events = Arc::new(Mutex::new(Vec::new()));
            let nos = FakeNos {
                events: nos_events.clone(),
                fail: false,
            };
            let directory = tempfile::tempdir().unwrap();

            assert!(execute_upload(
                &api,
                &nos,
                &CloudContext {
                    cookie: None,
                    proxy: None,
                    real_ip: None,
                },
                &fixture_file(&directory),
            )
            .await
            .is_err());
            assert_eq!(api.events.lock().unwrap().as_slice(), expected_api);
            assert_eq!(nos_events.lock().unwrap().as_slice(), expected_nos);
        }
    }

    #[tokio::test]
    async fn stalled_cloud_stage_is_bounded_before_any_commit() {
        let directory = tempfile::tempdir().unwrap();
        let nos = FakeNos {
            events: Arc::new(Mutex::new(Vec::new())),
            fail: false,
        };
        let error = execute_upload_with_timeouts(
            &StalledCloud,
            &nos,
            &CloudContext {
                cookie: None,
                proxy: None,
                real_ip: None,
            },
            &fixture_file(&directory),
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(10),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            CloudError::Stage {
                stage: "upload check",
                ref detail,
            } if detail == "timed out"
        ));
        assert!(nos.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn request_deadline_cancels_the_upload_chain_before_metadata_commit() {
        let api = FakeCloud {
            stall_at: Some("token"),
            stall_for: Duration::from_millis(40),
            ..FakeCloud::default()
        };
        let nos_events = Arc::new(Mutex::new(Vec::new()));
        let nos = FakeNos {
            events: nos_events.clone(),
            fail: false,
        };
        let directory = tempfile::tempdir().unwrap();

        let error = within_request_deadline(
            Duration::from_millis(10),
            execute_upload_with_timeouts(
                &api,
                &nos,
                &CloudContext {
                    cookie: None,
                    proxy: None,
                    real_ip: None,
                },
                &fixture_file(&directory),
                Duration::from_secs(1),
                Duration::from_secs(1),
            ),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, CloudError::RequestTimeout));
        assert_eq!(api.events.lock().unwrap().as_slice(), ["check", "token"]);
        assert!(nos_events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn request_deadline_uses_the_gateway_timeout_json_contract() {
        let response = cloud_error_response(CloudError::RequestTimeout);
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({ "code": 504, "message": "云盘上传超时" })
        );
    }

    #[tokio::test]
    async fn concurrent_upload_is_rejected_before_its_body_or_state_machine_is_read() {
        let checks = Arc::new(AtomicUsize::new(0));
        let (entered_tx, mut entered_rx) = mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let state = CloudState::with_dependencies(
            Arc::new(GatedCloud {
                checks: checks.clone(),
                entered: entered_tx,
                release: release.clone(),
            }),
            Arc::new(UnusedNos),
        );
        let app = Router::new()
            .route("/cloud", post(upload_song))
            .with_state(state);

        let first_boundary = "yesplaymusic-first-upload";
        let first_body = format!(
            "--{first_boundary}\r\nContent-Disposition: form-data; name=\"songFile\"; filename=\"first.mp3\"\r\nContent-Type: audio/mpeg\r\n\r\nfirst\r\n--{first_boundary}--\r\n"
        );
        let first_request = axum::http::Request::builder()
            .method("POST")
            .uri("/cloud")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={first_boundary}"),
            )
            .body(Body::from(first_body))
            .unwrap();
        let first = tokio::spawn(app.clone().oneshot(first_request));
        tokio::time::timeout(Duration::from_secs(1), entered_rx.recv())
            .await
            .expect("the first upload should reach the state machine")
            .expect("upload observer should remain open");

        let second_boundary = "yesplaymusic-second-upload";
        let second_multipart = format!(
            "--{second_boundary}\r\nContent-Disposition: form-data; name=\"songFile\"; filename=\"second.mp3\"\r\nContent-Type: audio/mpeg\r\n\r\nsecond\r\n--{second_boundary}--\r\n"
        );
        let body_reads = Arc::new(AtomicUsize::new(0));
        let observed_body_reads = body_reads.clone();
        let second_body = Body::from_stream(stream::once(async move {
            observed_body_reads.fetch_add(1, Ordering::SeqCst);
            Ok::<_, std::io::Error>(axum::body::Bytes::from(second_multipart))
        }));
        let second_request = axum::http::Request::builder()
            .method("POST")
            .uri("/cloud")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={second_boundary}"),
            )
            .body(second_body)
            .unwrap();
        let second =
            tokio::time::timeout(Duration::from_secs(1), app.clone().oneshot(second_request))
                .await
                .expect("a concurrent upload should receive an immediate response")
                .unwrap();

        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = axum::body::to_bytes(second.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({ "code": 429, "message": "已有云盘上传正在进行" })
        );
        assert_eq!(body_reads.load(Ordering::SeqCst), 0);
        assert_eq!(checks.load(Ordering::SeqCst), 1);
        assert!(entered_rx.try_recv().is_err());

        release.add_permits(1);
        let first = tokio::time::timeout(Duration::from_secs(1), first)
            .await
            .expect("the first upload should finish after release")
            .expect("the first upload task should not panic")
            .unwrap();
        assert_eq!(first.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn expired_cloud_session_preserves_the_desktop_logout_contract() {
        let state = CloudState::with_dependencies(Arc::new(ExpiredCloud), Arc::new(UnusedNos));
        let boundary = "yesplaymusic-cloud-auth";
        let multipart = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"songFile\"; filename=\"fixture.mp3\"\r\nContent-Type: audio/mpeg\r\n\r\nfixture\r\n--{boundary}--\r\n"
        );
        let response = Router::new()
            .route("/cloud", post(upload_song))
            .with_state(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/cloud")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(multipart))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({ "code": 301, "msg": "需要登录" })
        );
    }

    #[tokio::test]
    async fn multipart_mp3_and_flac_are_streamed_with_utf8_metadata_and_real_md5() {
        for extension in ["mp3", "flac"] {
            let captured = Arc::new(Mutex::new(None));
            let state = CloudState::with_dependencies(
                Arc::new(CaptureUploadCloud(captured.clone())),
                Arc::new(UnusedNos),
            );
            let boundary = format!("yesplaymusic-{extension}-capture");
            let filename = format!("雨 夜.demo.{extension}");
            let mut multipart = format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"songFile\"; filename=\"{filename}\"\r\nContent-Type: audio/{extension}\r\n\r\n"
            )
            .into_bytes();
            multipart.extend_from_slice(b"abc");
            multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

            let response = Router::new()
                .route("/cloud", post(upload_song))
                .with_state(state)
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/cloud")
                        .header(
                            header::CONTENT_TYPE,
                            format!("multipart/form-data; boundary={boundary}"),
                        )
                        .body(Body::from(multipart))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            assert_eq!(
                captured.lock().unwrap().clone().unwrap(),
                CapturedUpload {
                    filename,
                    extension: extension.to_owned(),
                    size: 3,
                    md5: "900150983cd24fb0d6963f7d28e17f72".to_owned(),
                    metadata: AudioMetadata {
                        title: "雨夜_demo".to_owned(),
                        album: "未知专辑".to_owned(),
                        artist: "未知艺术家".to_owned(),
                    },
                }
            );
        }
    }

    #[tokio::test]
    async fn multipart_reads_embedded_tags_from_real_mp3_and_flac_containers() {
        let fixtures = [
            ("mp3", include_str!("fixtures/audio-tags/tagged.mp3.b64")),
            ("flac", include_str!("fixtures/audio-tags/tagged.flac.b64")),
        ];

        for (extension, encoded) in fixtures {
            let audio = BASE64_STANDARD.decode(encoded.trim()).unwrap();
            let captured = Arc::new(Mutex::new(None));
            let state = CloudState::with_dependencies(
                Arc::new(CaptureUploadCloud(captured.clone())),
                Arc::new(UnusedNos),
            );
            let boundary = format!("yesplaymusic-tagged-{extension}");
            let filename = format!("fallback-name.{extension}");
            let mut multipart = format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"songFile\"; filename=\"{filename}\"\r\nContent-Type: audio/{extension}\r\n\r\n"
            )
            .into_bytes();
            multipart.extend_from_slice(&audio);
            multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

            let response = Router::new()
                .route("/cloud", post(upload_song))
                .with_state(state)
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/cloud")
                        .header(
                            header::CONTENT_TYPE,
                            format!("multipart/form-data; boundary={boundary}"),
                        )
                        .body(Body::from(multipart))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            let captured = captured.lock().unwrap().clone().unwrap();
            assert_eq!(captured.filename, filename);
            assert_eq!(captured.extension, extension);
            assert_eq!(captured.size, audio.len() as u64);
            assert_eq!(
                captured.metadata,
                AudioMetadata {
                    title: "迁移曲目".to_owned(),
                    album: "测试专辑".to_owned(),
                    artist: "云音乐艺术家".to_owned(),
                }
            );
        }
    }
}
