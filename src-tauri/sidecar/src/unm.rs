use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    extract::{rejection::JsonRejection, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures_util::StreamExt;
use serde_json::{json, Map, Number, Value};
use tokio::sync::Semaphore;
use unm_api_utils::executor::build_full_executor;
use unm_engine::{
    executor::{EngineId, Executor},
    interface::Engine,
};
use unm_types::{
    config::ConfigManager, Album, Artist, Context, RetrievedSongInfo, SearchMode,
    SerializedIdentifier, Song, SongSearchInformation,
};

pub const DEFAULT_SOURCES: [&str; 4] = ["ytdl", "bilibili", "pyncm", "kugou"];
const REQUIRED_REGISTERED_SOURCES: [&str; 7] =
    ["bilibili", "joox", "kugou", "kuwo", "pyncm", "qq", "ytdl"];
pub const BILIBILI_REFERER: &str = "https://www.bilibili.com/";
pub const BILIBILI_USER_AGENT: &str = "okhttp/3.4.1";
const UNM_PROVIDER_TIMEOUT: Duration = Duration::from_secs(15);
const BILIBILI_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const BILIBILI_READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BILIBILI_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CONCURRENT_BILIBILI_DOWNLOADS: usize = 2;
const HERMETIC_SMOKE_ENGINE_ID: &str = "yesplaymusic-hermetic-smoke";
const HERMETIC_SMOKE_URL: &str = "https://fixture.invalid/audio.mp3";

pub type UnmDependencyResult<T> = Result<T, String>;

#[async_trait]
pub trait UnmExecutorBackend: Send + Sync {
    fn list(&self) -> Vec<String>;

    async fn search(
        &self,
        sources: &[String],
        song: &Song,
        context: &Context,
    ) -> UnmDependencyResult<SongSearchInformation>;

    async fn retrieve(
        &self,
        song: &SongSearchInformation,
        context: &Context,
    ) -> UnmDependencyResult<RetrievedSongInfo>;
}

#[async_trait]
impl UnmExecutorBackend for Executor {
    fn list(&self) -> Vec<String> {
        Executor::list(self)
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    async fn search(
        &self,
        sources: &[String],
        song: &Song,
        context: &Context,
    ) -> UnmDependencyResult<SongSearchInformation> {
        let engine_ids: Vec<EngineId> = sources.iter().cloned().map(Cow::Owned).collect();
        Executor::search(self, &engine_ids, song, context)
            .await
            .map_err(|error| error.to_string())
    }

    async fn retrieve(
        &self,
        song: &SongSearchInformation,
        context: &Context,
    ) -> UnmDependencyResult<RetrievedSongInfo> {
        Executor::retrieve(self, song, context)
            .await
            .map_err(|error| error.to_string())
    }
}

struct HermeticSmokeEngine {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait]
impl Engine for HermeticSmokeEngine {
    async fn search<'a>(
        &self,
        song: &'a Song,
        _context: &'a Context,
    ) -> anyhow::Result<Option<SongSearchInformation>> {
        self.calls
            .lock()
            .map_err(|_| anyhow::anyhow!("hermetic UNM call recorder is poisoned"))?
            .push("search");
        if song.id != "424242" || song.name != "Hermetic UNM smoke" {
            anyhow::bail!("unexpected hermetic UNM smoke fixture")
        }
        Ok(Some(
            SongSearchInformation::builder()
                .source(Cow::Borrowed(HERMETIC_SMOKE_ENGINE_ID))
                .identifier("fixture-audio".to_owned())
                .build(),
        ))
    }

    async fn retrieve<'a>(
        &self,
        identifier: &'a SerializedIdentifier,
        _context: &'a Context,
    ) -> anyhow::Result<RetrievedSongInfo> {
        self.calls
            .lock()
            .map_err(|_| anyhow::anyhow!("hermetic UNM call recorder is poisoned"))?
            .push("retrieve");
        if identifier != "fixture-audio" {
            anyhow::bail!("unexpected hermetic UNM identifier")
        }
        Ok(RetrievedSongInfo::builder()
            .source(Cow::Borrowed(HERMETIC_SMOKE_ENGINE_ID))
            .url(HERMETIC_SMOKE_URL.to_owned())
            .build())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BilibiliDownloadRequest {
    pub url: String,
    pub referer: &'static str,
    pub user_agent: &'static str,
}

#[async_trait]
pub trait BilibiliClient: Send + Sync {
    async fn download(&self, request: BilibiliDownloadRequest) -> UnmDependencyResult<Vec<u8>>;
}

#[derive(Clone)]
struct ReqwestBilibiliClient {
    client: reqwest::Client,
    max_response_bytes: usize,
}

#[async_trait]
impl BilibiliClient for ReqwestBilibiliClient {
    async fn download(&self, request: BilibiliDownloadRequest) -> UnmDependencyResult<Vec<u8>> {
        let response = self
            .client
            .get(request.url)
            .header(reqwest::header::REFERER, request.referer)
            .header(reqwest::header::USER_AGENT, request.user_agent)
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?;
        if response
            .content_length()
            .is_some_and(|length| length > self.max_response_bytes as u64)
        {
            return Err("Bilibili response exceeds the configured size limit".to_owned());
        }

        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| error.to_string())?;
            let next_length = bytes
                .len()
                .checked_add(chunk.len())
                .ok_or_else(|| "Bilibili response exceeds the configured size limit".to_owned())?;
            if next_length > self.max_response_bytes {
                return Err("Bilibili response exceeds the configured size limit".to_owned());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

struct RejectingSmokeBilibiliClient;

#[async_trait]
impl BilibiliClient for RejectingSmokeBilibiliClient {
    async fn download(&self, _: BilibiliDownloadRequest) -> UnmDependencyResult<Vec<u8>> {
        Err("hermetic UNM smoke attempted a network download".to_owned())
    }
}

#[derive(Clone)]
pub struct UnmState {
    executor: Arc<dyn UnmExecutorBackend>,
    bilibili_client: Arc<dyn BilibiliClient>,
    bilibili_downloads: Arc<Semaphore>,
    provider_timeout: Duration,
    bilibili_download_timeout: Duration,
}

impl Default for UnmState {
    fn default() -> Self {
        Self::new()
    }
}

impl UnmState {
    pub fn new() -> Self {
        Self {
            executor: Arc::new(build_full_executor()),
            bilibili_client: Arc::new(ReqwestBilibiliClient {
                client: reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(10))
                    .read_timeout(BILIBILI_READ_TIMEOUT)
                    .build()
                    .expect("static UNM HTTP client configuration is valid"),
                max_response_bytes: MAX_BILIBILI_RESPONSE_BYTES,
            }),
            bilibili_downloads: Arc::new(Semaphore::new(MAX_CONCURRENT_BILIBILI_DOWNLOADS)),
            provider_timeout: UNM_PROVIDER_TIMEOUT,
            bilibili_download_timeout: BILIBILI_DOWNLOAD_TIMEOUT,
        }
    }

    pub fn with_dependencies(
        executor: Arc<dyn UnmExecutorBackend>,
        bilibili_client: Arc<dyn BilibiliClient>,
    ) -> Self {
        Self {
            executor,
            bilibili_client,
            bilibili_downloads: Arc::new(Semaphore::new(MAX_CONCURRENT_BILIBILI_DOWNLOADS)),
            provider_timeout: UNM_PROVIDER_TIMEOUT,
            bilibili_download_timeout: BILIBILI_DOWNLOAD_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_timeouts(mut self, provider_timeout: Duration, download_timeout: Duration) -> Self {
        self.provider_timeout = provider_timeout;
        self.bilibili_download_timeout = download_timeout;
        self
    }

    #[cfg(test)]
    fn with_timeout(self, provider_timeout: Duration) -> Self {
        self.with_timeouts(provider_timeout, BILIBILI_DOWNLOAD_TIMEOUT)
    }

    pub fn available_sources(&self) -> Vec<String> {
        self.executor.list()
    }

    pub async fn resolve(
        &self,
        payload: &Value,
    ) -> Result<Option<RetrievedSongInfo>, InvalidTrack> {
        let body = payload.as_object().ok_or(InvalidTrack)?;
        let track = parse_track(body.get("track").ok_or(InvalidTrack)?)?;
        let parsed_context = parse_context(body.get("context"));
        let sources = self.select_sources(body.get("sourceListString"), &parsed_context.excluded);

        if sources.is_empty() {
            return Ok(None);
        }

        let resolution = async {
            let matched = self
                .executor
                .search(&sources, &track, &parsed_context.context)
                .await
                .ok()?;
            let retrieved = self
                .executor
                .retrieve(&matched, &parsed_context.context)
                .await
                .ok()?;

            Some(retrieved)
        };

        let Some(mut retrieved) = tokio::time::timeout(self.provider_timeout, resolution)
            .await
            .unwrap_or(None)
        else {
            return Ok(None);
        };
        if retrieved.url.contains("bilivideo.com") {
            let request = BilibiliDownloadRequest {
                url: retrieved.url.clone(),
                referer: BILIBILI_REFERER,
                user_agent: BILIBILI_USER_AGENT,
            };
            let encoded = tokio::time::timeout(self.bilibili_download_timeout, async {
                let _permit = self.bilibili_downloads.acquire().await.ok()?;
                let bytes = self.bilibili_client.download(request).await.ok()?;
                Some(BASE64_STANDARD.encode(bytes))
            })
            .await
            .ok()
            .flatten();
            let Some(encoded) = encoded else {
                return Ok(None);
            };
            retrieved.url = encoded;
        }
        Ok(Some(retrieved))
    }

    fn select_sources(
        &self,
        source_list: Option<&Value>,
        excluded: &HashSet<String>,
    ) -> Vec<String> {
        let mut sources: Vec<String> = match source_list.and_then(Value::as_str) {
            Some(source_list) => {
                let available: HashSet<String> = self
                    .executor
                    .list()
                    .into_iter()
                    .map(|source| source.to_lowercase())
                    .collect();
                source_list
                    .split(',')
                    .map(|source| source.trim().to_lowercase())
                    .filter(|source| available.contains(source))
                    .collect()
            }
            None => DEFAULT_SOURCES
                .iter()
                .map(|source| (*source).to_owned())
                .collect(),
        };
        sources.retain(|source| !excluded.contains(source));
        sources
    }
}

pub async fn run_hermetic_executor_smoke() -> Result<Vec<String>, String> {
    let sources = UnmState::new().available_sources();
    let available: HashSet<&str> = sources.iter().map(String::as_str).collect();
    let missing = REQUIRED_REGISTERED_SOURCES
        .iter()
        .copied()
        .filter(|source| !available.contains(source))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "UNM is missing required production providers: {}",
            missing.join(", ")
        ));
    }

    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut executor = Executor::new();
    executor.register(
        Cow::Borrowed(HERMETIC_SMOKE_ENGINE_ID),
        Arc::new(HermeticSmokeEngine {
            calls: calls.clone(),
        }),
    );
    let state =
        UnmState::with_dependencies(Arc::new(executor), Arc::new(RejectingSmokeBilibiliClient));
    let retrieved = state
        .resolve(&json!({
            "sourceListString": HERMETIC_SMOKE_ENGINE_ID,
            "track": { "id": 424242, "name": "Hermetic UNM smoke" },
            "context": {}
        }))
        .await
        .map_err(|_| "hermetic UNM fixture was rejected".to_owned())?
        .ok_or_else(|| "production UNM executor returned no hermetic result".to_owned())?;

    if retrieved.source.as_ref() != HERMETIC_SMOKE_ENGINE_ID || retrieved.url != HERMETIC_SMOKE_URL
    {
        return Err("production UNM executor returned the wrong hermetic result".to_owned());
    }
    let recorded_calls = calls
        .lock()
        .map_err(|_| "hermetic UNM call recorder is poisoned".to_owned())?;
    if recorded_calls.as_slice() != ["search", "retrieve"] {
        return Err("production UNM executor did not run search then retrieve".to_owned());
    }

    Ok(sources)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidTrack;

pub fn router(state: UnmState) -> Router {
    Router::new()
        .route("/native/unblock-music", post(unblock_music_handler))
        .with_state(state)
}

pub async fn unblock_music_handler(
    State(state): State<UnmState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Response {
    let Json(payload) = match payload {
        Ok(payload) => payload,
        Err(_) => return invalid_track_response(),
    };
    match state.resolve(&payload).await {
        Ok(result) => Json(result).into_response(),
        Err(_) => invalid_track_response(),
    }
}

fn invalid_track_response() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "message": "缺少歌曲信息" })),
    )
        .into_response()
}

struct ParsedContext {
    context: Context,
    excluded: HashSet<String>,
}

fn parse_context(value: Option<&Value>) -> ParsedContext {
    let Some(object) = value.and_then(Value::as_object) else {
        return ParsedContext {
            context: Context::default(),
            excluded: HashSet::new(),
        };
    };

    let excluded = object
        .get("excludedSources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_lowercase)
        .collect();

    let mut context = Context::default();
    context.proxy_uri = object
        .get("proxyUri")
        .and_then(Value::as_str)
        .map(|proxy| Cow::Owned(proxy.to_owned()));
    context.enable_flac = object
        .get("enableFlac")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    context.search_mode = match object.get("searchMode").and_then(Value::as_i64) {
        Some(1) => SearchMode::OrderFirst,
        _ => SearchMode::FastFirst,
    };
    context.config = object
        .get("config")
        .and_then(Value::as_object)
        .map(|config| {
            let values: HashMap<Cow<'static, str>, String> = config
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .as_str()
                        .map(|value| (Cow::Owned(key.clone()), value.to_owned()))
                })
                .collect();
            ConfigManager::new(values)
        });

    ParsedContext { context, excluded }
}

fn parse_track(value: &Value) -> Result<Song, InvalidTrack> {
    let track = value.as_object().ok_or(InvalidTrack)?;
    let id = finite_number(track.get("id").ok_or(InvalidTrack)?)?;
    let name = optional_string(track, "name")?.unwrap_or_default();
    let duration = match track.get("dt") {
        Some(value) => Some(number_to_i64(finite_number(value)?)),
        None => None,
    };

    let album = match track.get("al") {
        Some(value) => {
            let album = value.as_object().ok_or(InvalidTrack)?;
            let id = finite_number(album.get("id").ok_or(InvalidTrack)?)?;
            Some(
                Album::builder()
                    .id(id.to_string())
                    .name(optional_string(album, "name")?.unwrap_or_default())
                    .build(),
            )
        }
        None => None,
    };

    let artists = match track.get("ar") {
        Some(value) => value
            .as_array()
            .ok_or(InvalidTrack)?
            .iter()
            .map(|value| {
                let artist = value.as_object().ok_or(InvalidTrack)?;
                let id = finite_number(artist.get("id").ok_or(InvalidTrack)?)?;
                Ok(Artist::builder()
                    .id(id.to_string())
                    .name(optional_string(artist, "name")?.unwrap_or_default())
                    .build())
            })
            .collect::<Result<Vec<_>, InvalidTrack>>()?,
        None => Vec::new(),
    };

    Ok(Song::builder()
        .id(id.to_string())
        .name(name)
        .duration(duration)
        .artists(artists)
        .album(album)
        .build())
}

fn finite_number(value: &Value) -> Result<&Number, InvalidTrack> {
    let number = value.as_number().ok_or(InvalidTrack)?;
    match number.as_f64() {
        Some(value) if value.is_finite() => Ok(number),
        _ => Err(InvalidTrack),
    }
}

fn number_to_i64(number: &Number) -> i64 {
    number
        .as_i64()
        .or_else(|| number.as_u64().and_then(|value| value.try_into().ok()))
        .unwrap_or_else(|| number.as_f64().unwrap_or_default() as i64)
}

fn optional_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, InvalidTrack> {
    match object.get(key) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(InvalidTrack),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::mpsc,
    };

    use super::*;

    type RecordedSearches = Arc<Mutex<Vec<(Vec<String>, Song, Context)>>>;

    struct RecordingExecutor {
        available: Vec<String>,
        fail_search: bool,
        retrieve_url: String,
        calls: Arc<Mutex<Vec<&'static str>>>,
        searches: RecordedSearches,
    }

    #[async_trait]
    impl UnmExecutorBackend for RecordingExecutor {
        fn list(&self) -> Vec<String> {
            self.available.clone()
        }

        async fn search(
            &self,
            sources: &[String],
            song: &Song,
            context: &Context,
        ) -> UnmDependencyResult<SongSearchInformation> {
            self.calls.lock().unwrap().push("search");
            self.searches
                .lock()
                .unwrap()
                .push((sources.to_vec(), song.clone(), context.clone()));
            if self.fail_search {
                return Err("offline".to_owned());
            }
            Ok(SongSearchInformation::builder()
                .source(Cow::Borrowed("bilibili"))
                .identifier("video-a".to_owned())
                .build())
        }

        async fn retrieve(
            &self,
            _song: &SongSearchInformation,
            _context: &Context,
        ) -> UnmDependencyResult<RetrievedSongInfo> {
            self.calls.lock().unwrap().push("retrieve");
            Ok(RetrievedSongInfo::builder()
                .source(Cow::Borrowed("bilibili"))
                .url(self.retrieve_url.clone())
                .build())
        }
    }

    struct RecordingBilibiliClient {
        requests: Arc<Mutex<Vec<BilibiliDownloadRequest>>>,
    }

    #[async_trait]
    impl BilibiliClient for RecordingBilibiliClient {
        async fn download(&self, request: BilibiliDownloadRequest) -> UnmDependencyResult<Vec<u8>> {
            self.requests.lock().unwrap().push(request);
            Ok(b"audio".to_vec())
        }
    }

    struct DelayedBilibiliClient {
        delay: Duration,
    }

    #[async_trait]
    impl BilibiliClient for DelayedBilibiliClient {
        async fn download(&self, _: BilibiliDownloadRequest) -> UnmDependencyResult<Vec<u8>> {
            tokio::time::sleep(self.delay).await;
            Ok(b"audio".to_vec())
        }
    }

    struct GatedBilibiliClient {
        entered: mpsc::UnboundedSender<()>,
        release: Arc<tokio::sync::Semaphore>,
    }

    #[async_trait]
    impl BilibiliClient for GatedBilibiliClient {
        async fn download(&self, _: BilibiliDownloadRequest) -> UnmDependencyResult<Vec<u8>> {
            self.entered
                .send(())
                .map_err(|_| "download observer closed".to_owned())?;
            let permit = self
                .release
                .acquire()
                .await
                .map_err(|_| "download gate closed".to_owned())?;
            permit.forget();
            Ok(b"audio".to_vec())
        }
    }

    struct StalledExecutor;

    #[async_trait]
    impl UnmExecutorBackend for StalledExecutor {
        fn list(&self) -> Vec<String> {
            vec!["pyncm".to_owned()]
        }

        async fn search(
            &self,
            _: &[String],
            _: &Song,
            _: &Context,
        ) -> UnmDependencyResult<SongSearchInformation> {
            std::future::pending().await
        }

        async fn retrieve(
            &self,
            _: &SongSearchInformation,
            _: &Context,
        ) -> UnmDependencyResult<RetrievedSongInfo> {
            unreachable!("a stalled search must time out first")
        }
    }

    async fn start_chunked_server(
        chunks: Vec<&'static [u8]>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            for chunk in chunks {
                socket
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await
                    .unwrap();
                socket.write_all(chunk).await.unwrap();
                socket.write_all(b"\r\n").await.unwrap();
            }
            socket.write_all(b"0\r\n\r\n").await.unwrap();
        });
        (address, task)
    }

    #[tokio::test]
    async fn preserves_unm_inputs_order_and_bilibili_transport() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let searches = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = UnmState::with_dependencies(
            Arc::new(RecordingExecutor {
                available: vec!["qq".to_owned(), "bilibili".to_owned()],
                fail_search: false,
                retrieve_url: "https://example.bilivideo.com/audio".to_owned(),
                calls: calls.clone(),
                searches: searches.clone(),
            }),
            Arc::new(RecordingBilibiliClient {
                requests: requests.clone(),
            }),
        );
        let payload = json!({
            "sourceListString": " QQ, bilibili, QQ, bilibili, invalid ",
            "track": {
                "id": 123,
                "name": "测试歌曲",
                "dt": 456,
                "al": { "id": 789, "name": "测试专辑" },
                "ar": [{ "id": 101, "name": "测试歌手" }]
            },
            "context": {
                "excludedSources": ["qQ"],
                "proxyUri": "http://proxy.example:8080",
                "enableFlac": true,
                "searchMode": 1,
                "config": { "qq:cookie": "secret", "ytdl:exe": null }
            }
        });

        let result = state.resolve(&payload).await.unwrap().unwrap();
        assert_eq!(result.source.as_ref(), "bilibili");
        assert_eq!(result.url, "YXVkaW8=");
        assert_eq!(*calls.lock().unwrap(), ["search", "retrieve"]);

        let searches = searches.lock().unwrap();
        let (sources, song, context) = &searches[0];
        assert_eq!(sources, &["bilibili", "bilibili"]);
        assert_eq!(song.id, "123");
        assert_eq!(song.name, "测试歌曲");
        assert_eq!(song.duration, Some(456));
        assert_eq!(song.album.as_ref().unwrap().id, "789");
        assert_eq!(song.artists[0].id, "101");
        assert_eq!(
            context.proxy_uri.as_deref(),
            Some("http://proxy.example:8080")
        );
        assert!(context.enable_flac);
        assert!(matches!(context.search_mode, SearchMode::OrderFirst));
        assert_eq!(
            context
                .config
                .as_ref()
                .unwrap()
                .get_deref(Cow::Borrowed("qq:cookie")),
            Some("secret")
        );
        assert_eq!(
            context
                .config
                .as_ref()
                .unwrap()
                .get_deref(Cow::Borrowed("ytdl:exe")),
            None
        );

        assert_eq!(
            requests.lock().unwrap().as_slice(),
            &[BilibiliDownloadRequest {
                url: "https://example.bilivideo.com/audio".to_owned(),
                referer: BILIBILI_REFERER,
                user_agent: BILIBILI_USER_AGENT,
            }]
        );
    }

    #[tokio::test]
    async fn uses_default_sources_and_turns_provider_failure_into_null() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let searches = Arc::new(Mutex::new(Vec::new()));
        let state = UnmState::with_dependencies(
            Arc::new(RecordingExecutor {
                available: Vec::new(),
                fail_search: true,
                retrieve_url: "https://example.bilivideo.com/audio".to_owned(),
                calls,
                searches: searches.clone(),
            }),
            Arc::new(RecordingBilibiliClient {
                requests: Arc::new(Mutex::new(Vec::new())),
            }),
        );

        let result = state
            .resolve(&json!({ "track": { "id": 1 }, "context": {} }))
            .await
            .unwrap();

        assert!(result.is_none());
        assert_eq!(searches.lock().unwrap()[0].0, DEFAULT_SOURCES);
    }

    #[tokio::test]
    async fn stalled_provider_is_bounded_by_the_frontend_request_budget() {
        let state = UnmState::with_dependencies(
            Arc::new(StalledExecutor),
            Arc::new(RecordingBilibiliClient {
                requests: Arc::new(Mutex::new(Vec::new())),
            }),
        )
        .with_timeout(Duration::from_millis(10));

        let result = state
            .resolve(&json!({ "track": { "id": 1 }, "context": {} }))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn bilibili_download_has_a_separate_budget_from_provider_lookup() {
        let state = UnmState::with_dependencies(
            Arc::new(RecordingExecutor {
                available: vec!["bilibili".to_owned()],
                fail_search: false,
                retrieve_url: "https://example.bilivideo.com/audio".to_owned(),
                calls: Arc::new(Mutex::new(Vec::new())),
                searches: Arc::new(Mutex::new(Vec::new())),
            }),
            Arc::new(DelayedBilibiliClient {
                delay: Duration::from_millis(30),
            }),
        )
        .with_timeouts(Duration::from_millis(10), Duration::from_millis(100));

        let result = state
            .resolve(&json!({ "track": { "id": 1 }, "context": {} }))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.url, BASE64_STANDARD.encode(b"audio"));
    }

    #[tokio::test]
    async fn bilibili_downloads_wait_at_the_shared_concurrency_limit() {
        let (entered_tx, mut entered_rx) = mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let state = UnmState::with_dependencies(
            Arc::new(RecordingExecutor {
                available: vec!["bilibili".to_owned()],
                fail_search: false,
                retrieve_url: "https://example.bilivideo.com/audio".to_owned(),
                calls: Arc::new(Mutex::new(Vec::new())),
                searches: Arc::new(Mutex::new(Vec::new())),
            }),
            Arc::new(GatedBilibiliClient {
                entered: entered_tx,
                release: release.clone(),
            }),
        );

        let mut requests = Vec::new();
        for id in 1..=3 {
            let state = state.clone();
            requests.push(tokio::spawn(async move {
                state
                    .resolve(&json!({ "track": { "id": id }, "context": {} }))
                    .await
            }));
        }

        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(1), entered_rx.recv())
                .await
                .expect("two downloads should enter promptly")
                .expect("download observer should remain open");
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), entered_rx.recv())
                .await
                .is_err(),
            "a third download entered before capacity was released"
        );

        release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(1), entered_rx.recv())
            .await
            .expect("the waiting download should enter after capacity is released")
            .expect("download observer should remain open");
        release.add_permits(2);

        for request in requests {
            let result = tokio::time::timeout(Duration::from_secs(1), request)
                .await
                .expect("download should finish after release")
                .expect("download task should not panic")
                .expect("track should remain valid")
                .expect("Bilibili should return an audio result");
            assert_eq!(result.url, BASE64_STANDARD.encode(b"audio"));
        }
    }

    #[tokio::test]
    async fn chunked_bilibili_download_enforces_the_streaming_size_limit() {
        let (allowed_address, allowed_server) =
            start_chunked_server(vec![b"abc".as_slice(), b"de".as_slice()]).await;
        let (oversized_address, oversized_server) =
            start_chunked_server(vec![b"abc".as_slice(), b"def".as_slice()]).await;
        let client = ReqwestBilibiliClient {
            client: reqwest::Client::builder()
                .no_proxy()
                .resolve("allowed.bilivideo.com", allowed_address)
                .resolve("oversized.bilivideo.com", oversized_address)
                .build()
                .unwrap(),
            max_response_bytes: 5,
        };

        let allowed = client
            .download(BilibiliDownloadRequest {
                url: format!(
                    "http://allowed.bilivideo.com:{}/audio",
                    allowed_address.port()
                ),
                referer: BILIBILI_REFERER,
                user_agent: BILIBILI_USER_AGENT,
            })
            .await
            .unwrap();
        assert_eq!(allowed, b"abcde");
        allowed_server.await.unwrap();

        let calls = Arc::new(Mutex::new(Vec::new()));
        let searches = Arc::new(Mutex::new(Vec::new()));
        let state = UnmState::with_dependencies(
            Arc::new(RecordingExecutor {
                available: vec!["bilibili".to_owned()],
                fail_search: false,
                retrieve_url: format!(
                    "http://oversized.bilivideo.com:{}/audio",
                    oversized_address.port()
                ),
                calls,
                searches,
            }),
            Arc::new(client),
        );
        let result = state
            .resolve(&json!({ "track": { "id": 1 }, "context": {} }))
            .await
            .unwrap();
        assert!(result.is_none());
        oversized_server.await.unwrap();
    }
}
