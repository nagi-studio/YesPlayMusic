use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{header, HeaderMap, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    routing::{delete, post},
    Json, Router,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
#[cfg(target_os = "macos")]
use tokio::process::Command;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{Mutex as AsyncMutex, Semaphore},
};
use tokio_util::io::ReaderStream;

const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(target_os = "macos")]
const CONVERT_TIMEOUT: Duration = Duration::from_secs(20);

pub fn afconvert_data_format(bits_per_sample: Option<u32>) -> &'static str {
    match bits_per_sample.unwrap_or(16) {
        0..=16 => "LEI16",
        17..=24 => "LEI24",
        _ => "LEI32",
    }
}

fn wav_filename(value: &str) -> Option<String> {
    (value.len() <= 20 && !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| format!("{value}.wav"))
}

#[async_trait]
pub trait WavConverter: Send + Sync {
    async fn convert(&self, input: &Path, output: &Path, data_format: &str) -> Result<(), String>;
}

pub struct Afconvert;

#[async_trait]
impl WavConverter for Afconvert {
    async fn convert(&self, input: &Path, output: &Path, data_format: &str) -> Result<(), String> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (input, output, data_format);
            Err("native WAV conversion is available only on macOS".to_owned())
        }
        #[cfg(target_os = "macos")]
        {
            let mut command = Command::new("afconvert");
            command
                .args(["-f", "WAVE", "-d", data_format])
                .arg(input)
                .arg(output)
                .kill_on_drop(true);
            let output = tokio::time::timeout(CONVERT_TIMEOUT, command.output())
                .await
                .map_err(|_| "afconvert timed out".to_owned())?
                .map_err(|error| error.to_string())?;
            if output.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(200)
                    .collect())
            }
        }
    }
}

#[derive(Clone)]
pub struct PreciseWavState {
    directory: Arc<PathBuf>,
    converter: Arc<dyn WavConverter>,
    conversion: Arc<Semaphore>,
    files: Arc<AsyncMutex<()>>,
    native_conversion: bool,
}

impl PreciseWavState {
    pub async fn production() -> std::io::Result<Self> {
        let directory = std::env::temp_dir().join("yesplaymusic-precise-wav");
        let state = Self {
            directory: Arc::new(directory),
            converter: Arc::new(Afconvert),
            conversion: Arc::new(Semaphore::new(1)),
            files: Arc::new(AsyncMutex::new(())),
            native_conversion: cfg!(target_os = "macos"),
        };
        state.sweep(&[]).await?;
        Ok(state)
    }

    #[cfg(test)]
    async fn for_test(directory: PathBuf, converter: Arc<dyn WavConverter>) -> Self {
        let state = Self {
            directory: Arc::new(directory),
            converter,
            conversion: Arc::new(Semaphore::new(1)),
            files: Arc::new(AsyncMutex::new(())),
            native_conversion: true,
        };
        state.sweep(&[]).await.unwrap();
        state
    }

    async fn sweep(&self, keep: &[&str]) -> std::io::Result<()> {
        tokio::fs::create_dir_all(self.directory.as_ref()).await?;
        let mut entries = tokio::fs::read_dir(self.directory.as_ref()).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            if !keep.iter().any(|keep| name == std::ffi::OsStr::new(keep)) {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct ConvertQuery {
    bits: Option<u32>,
}

async fn persist_body(body: Body, path: &Path) -> Result<u64, String> {
    let mut output = tokio::fs::File::create(path)
        .await
        .map_err(|error| error.to_string())?;
    let mut received = 0_u64;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "upload is too large".to_owned())?;
        if received > MAX_UPLOAD_BYTES {
            return Err("upload is too large".to_owned());
        }
        output
            .write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
    }
    output.flush().await.map_err(|error| error.to_string())?;
    Ok(received)
}

async fn convert(
    State(state): State<PreciseWavState>,
    AxumPath(track_id): AxumPath<String>,
    Query(query): Query<ConvertQuery>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    if !state.native_conversion {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "message": "当前平台不提供原生 WAV 转换" })),
        )
            .into_response();
    }
    let Some(wav_name) = wav_filename(&track_id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "无效的歌曲 ID" })),
        )
            .into_response();
    };
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_UPLOAD_BYTES)
    {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let Ok(_permit) = state.conversion.clone().try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "message": "已有转换在进行中" })),
        )
            .into_response();
    };
    // A conversion waits for an earlier cleanup, while a later cleanup observes this lock and
    // becomes a no-op. This preserves the freshly converted WAV from stale fire-and-forget DELETEs.
    let _files = state.files.lock().await;

    let flac_name = format!("{track_id}.flac");
    if let Err(error) = state.sweep(&[]).await {
        tracing::warn!("precise WAV cleanup failed: {error}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let flac_path = state.directory.join(&flac_name);
    let wav_path = state.directory.join(&wav_name);
    let result = async {
        tokio::time::timeout(UPLOAD_TIMEOUT, persist_body(body, &flac_path))
            .await
            .map_err(|_| "upload timed out".to_owned())??;
        state
            .converter
            .convert(&flac_path, &wav_path, afconvert_data_format(query.bits))
            .await?;
        tokio::fs::remove_file(&flac_path)
            .await
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(track_id, "precise WAV conversion failed: {error}");
        let _ = tokio::fs::remove_file(&flac_path).await;
        let _ = tokio::fs::remove_file(&wav_path).await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "转换失败" })),
        )
            .into_response();
    }
    Json(json!({ "url": format!("/precise-wav/{wav_name}") })).into_response()
}

async fn clear(State(state): State<PreciseWavState>) -> StatusCode {
    let Ok(_files) = state.files.try_lock() else {
        // `convert` sweeps stale files before writing. A DELETE that arrives after that point
        // belongs to the previous source and must not remove the in-flight conversion's output.
        return StatusCode::NO_CONTENT;
    };
    match state.sweep(&[]).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::warn!("precise WAV cleanup failed: {error}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn parse_range(value: &str, length: u64) -> Option<(u64, u64)> {
    let range = value.strip_prefix("bytes=")?;
    if range.contains(',') || length == 0 {
        return None;
    }
    let (start, end) = range.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?.min(length);
        return (suffix != 0).then_some((length - suffix, length - 1));
    }
    let start = start.parse::<u64>().ok()?;
    let end = if end.is_empty() {
        length - 1
    } else {
        end.parse::<u64>().ok()?.min(length - 1)
    };
    (start <= end && start < length).then_some((start, end))
}

async fn serve(
    State(state): State<PreciseWavState>,
    AxumPath(filename): AxumPath<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let valid = filename
        .strip_suffix(".wav")
        .and_then(wav_filename)
        .is_some_and(|canonical| canonical == filename);
    if !valid {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = state.directory.join(&filename);
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => {
            tracing::warn!("precise WAV read failed: {error}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let length = match file.metadata().await {
        Ok(metadata) => metadata.len(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let requested_range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let (status, start, end) = match requested_range {
        Some(value) => match parse_range(value, length) {
            Some((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end),
            None => {
                let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                response.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{length}")).unwrap(),
                );
                return response;
            }
        },
        None if length != 0 => (StatusCode::OK, 0, length - 1),
        None => (StatusCode::OK, 0, 0),
    };
    if start != 0 && file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let response_length = if length == 0 { 0 } else { end - start + 1 };
    let stream = ReaderStream::new(file.take(response_length));
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    let response_headers = response.headers_mut();
    response_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&response_length.to_string()).unwrap(),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        response_headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{length}")).unwrap(),
        );
    }
    response
}

pub fn router(state: PreciseWavState) -> Router {
    Router::new()
        .route("/precise-wav", delete(clear))
        .route("/precise-wav/{track_id}", post(convert).get(serve))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::body::to_bytes;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    use super::*;

    struct FakeConverter(Mutex<Vec<String>>);

    #[async_trait]
    impl WavConverter for FakeConverter {
        async fn convert(&self, input: &Path, output: &Path, format: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(format.to_owned());
            let bytes = tokio::fs::read(input)
                .await
                .map_err(|error| error.to_string())?;
            tokio::fs::write(output, bytes)
                .await
                .map_err(|error| error.to_string())
        }
    }

    struct ControlledConverter {
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl WavConverter for ControlledConverter {
        async fn convert(&self, input: &Path, output: &Path, _: &str) -> Result<(), String> {
            let bytes = tokio::fs::read(input)
                .await
                .map_err(|error| error.to_string())?;
            tokio::fs::write(output, &bytes)
                .await
                .map_err(|error| error.to_string())?;
            self.started.notify_one();
            self.release.notified().await;

            tokio::fs::read(input)
                .await
                .map_err(|error| format!("input removed during conversion: {error}"))?;
            tokio::fs::read(output)
                .await
                .map_err(|error| format!("output removed during conversion: {error}"))?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn conversion_preserves_depth_choice_and_serves_real_ranges() {
        let directory = tempfile::tempdir().unwrap();
        let converter = Arc::new(FakeConverter(Mutex::new(Vec::new())));
        let state = PreciseWavState::for_test(directory.path().to_owned(), converter.clone()).await;
        let app = router(state);
        let converted = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/precise-wav/42?bits=24")
                    .body(Body::from("0123456789"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(converted.status(), StatusCode::OK);
        assert_eq!(converter.0.lock().unwrap().as_slice(), ["LEI24"]);

        let partial = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/precise-wav/42.wav")
                    .header(header::RANGE, "bytes=3-6")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.headers()[header::CONTENT_RANGE], "bytes 3-6/10");
        assert_eq!(to_bytes(partial.into_body(), 16).await.unwrap(), "3456");
    }

    #[tokio::test]
    async fn stale_clear_during_conversion_never_removes_the_new_result() {
        let directory = tempfile::tempdir().unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let converter = Arc::new(ControlledConverter {
            started: started.clone(),
            release: release.clone(),
        });
        let state = PreciseWavState::for_test(directory.path().to_owned(), converter.clone()).await;
        let app = router(state);
        let conversion = tokio::spawn(
            app.clone().oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/precise-wav/42")
                    .body(Body::from("encoded-flac"))
                    .unwrap(),
            ),
        );

        started.notified().await;
        let flac_path = directory.path().join("42.flac");
        let wav_path = directory.path().join("42.wav");
        assert!(flac_path.is_file());
        assert!(wav_path.is_file());

        let cleared = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri("/precise-wav")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cleared.status(), StatusCode::NO_CONTENT);
        assert!(flac_path.is_file());
        assert!(wav_path.is_file());

        release.notify_one();
        let converted = conversion.await.unwrap().unwrap();
        assert_eq!(converted.status(), StatusCode::OK);
        assert!(!flac_path.exists());
        assert!(wav_path.is_file());

        let served = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/precise-wav/42.wav")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(served.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(served.into_body(), 32).await.unwrap(),
            "encoded-flac"
        );
    }

    #[test]
    fn bit_depth_mapping_never_requests_float_or_resampling() {
        assert_eq!(afconvert_data_format(Some(16)), "LEI16");
        assert_eq!(afconvert_data_format(Some(24)), "LEI24");
        assert_eq!(afconvert_data_format(Some(32)), "LEI32");
    }
}
