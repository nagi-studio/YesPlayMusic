use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode},
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use yesplaymusic_core::cache::{
    AudioCodec, AudioQuality, CacheKey, CacheLease, CacheWriteRequest, TrackCache,
};

const MAX_IMPORT_BODY_BYTES: usize = 513 * 1024 * 1024;
const AUDIO_CODEC_HEADER: &str = "x-ypm-audio-codec";

#[derive(Clone)]
pub struct SharedCacheState {
    cache_root: Arc<PathBuf>,
    client: reqwest::Client,
    enabled: Arc<AtomicBool>,
}

impl SharedCacheState {
    pub fn production() -> Result<Self, reqwest::Error> {
        let base_root = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cache/ypm");
        Self::new(base_root, reqwest::Client::builder().build()?)
    }

    fn new(base_root: PathBuf, client: reqwest::Client) -> Result<Self, reqwest::Error> {
        Ok(Self {
            cache_root: Arc::new(base_root.join("audio")),
            client,
            enabled: Arc::new(AtomicBool::new(false)),
        })
    }

    #[cfg(test)]
    fn testing(base_root: PathBuf) -> Self {
        Self::new(base_root, reqwest::Client::new()).expect("build shared cache test state")
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// True only when the shared tracks directory holds published files.
    /// Checked per request: the TUI creates the directory skeleton on every
    /// launch, and the cache can fill up or be wiped while the sidecar runs.
    fn terminal_cache_detected(&self) -> bool {
        std::fs::read_dir(self.cache_root.join("tracks"))
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SharedCacheStatus {
    enabled: bool,
    terminal_cache_detected: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedCacheSettings {
    enabled: bool,
    /// Size cap in MiB, stored in the shared index so the TUI honors it too.
    cache_limit_mib: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioQuery {
    quality: u32,
    source: Option<String>,
    codec: Option<String>,
    actual_bitrate: Option<u32>,
    cache: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportMetadata {
    track_id: i64,
    quality: u32,
    codec: String,
    actual_bitrate: u32,
    name: String,
    artist: String,
}

enum CacheWriteMessage {
    Chunk(Bytes),
    Complete,
}

type UpstreamStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>>;

pub fn router(state: SharedCacheState) -> Router {
    Router::new()
        .route("/native/shared-cache/status", get(status_handler))
        .route("/native/shared-cache/settings", put(settings_handler))
        .route(
            "/native/shared-cache/import",
            post(import_handler).layer(DefaultBodyLimit::max(MAX_IMPORT_BODY_BYTES)),
        )
        .route(
            "/native/shared-cache/audio/{track_id}",
            get(audio_handler)
                .head(audio_handler)
                .delete(delete_audio_handler),
        )
        .with_state(state)
}

async fn status_handler(State(state): State<SharedCacheState>) -> Json<SharedCacheStatus> {
    Json(SharedCacheStatus {
        enabled: state.is_enabled(),
        terminal_cache_detected: state.terminal_cache_detected() && !state.is_enabled(),
    })
}

async fn settings_handler(
    State(state): State<SharedCacheState>,
    Json(settings): Json<SharedCacheSettings>,
) -> Response<Body> {
    if settings.enabled {
        let root = state.cache_root.as_ref().clone();
        let run_maintenance = !state.is_enabled();
        let limit_mib = settings.cache_limit_mib;
        let opened = tokio::task::spawn_blocking(move || {
            let cache = TrackCache::open(root)?;
            if let Some(limit_mib) = limit_mib {
                match limit_mib.checked_mul(1024 * 1024) {
                    Some(max_bytes) => cache.set_max_bytes(max_bytes)?,
                    None => tracing::warn!("shared audio cache limit is too large"),
                }
            }
            // One sweep per enable, not per request: reconcile is O(entries)
            // and used to run on every open, stalling playback for users
            // with a large terminal-side cache.
            if run_maintenance {
                cache.maintain()?;
            }
            Ok::<_, yesplaymusic_core::cache::CacheError>(())
        })
        .await;
        match opened {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "shared audio cache initialization failed");
                return json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "shared audio cache is unavailable",
                );
            }
            Err(error) => {
                tracing::warn!(%error, "shared audio cache worker failed");
                return json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "shared audio cache is unavailable",
                );
            }
        }
    }
    state.enabled.store(settings.enabled, Ordering::Release);
    Json(SharedCacheStatus {
        enabled: settings.enabled,
        terminal_cache_detected: state.terminal_cache_detected() && !settings.enabled,
    })
    .into_response()
}

async fn import_handler(
    State(state): State<SharedCacheState>,
    mut multipart: Multipart,
) -> Response<Body> {
    if let Some(response) = disabled_response(&state) {
        return response;
    }

    let mut metadata = None;
    let mut audio = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid multipart body"),
        };
        match field.name() {
            Some("metadata") => {
                let value = match field.text().await {
                    Ok(value) => value,
                    Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid cache metadata"),
                };
                metadata = serde_json::from_str::<ImportMetadata>(&value).ok();
            }
            Some("audio") => {
                audio = match field.bytes().await {
                    Ok(bytes) => Some(bytes),
                    Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid audio body"),
                };
            }
            _ => {}
        }
    }

    let Some(metadata) = metadata else {
        return json_error(StatusCode::BAD_REQUEST, "cache metadata is required");
    };
    let Some(audio) = audio.filter(|bytes| !bytes.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "audio bytes are required");
    };
    let request = match cache_write_request(
        metadata.track_id,
        metadata.quality,
        &metadata.codec,
        metadata.actual_bitrate,
        Some(audio.len() as u64),
    ) {
        Ok(request) => request,
        Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
    };
    if metadata.name.len() > 1024 || metadata.artist.len() > 1024 {
        return json_error(StatusCode::BAD_REQUEST, "song metadata is too long");
    }

    let root = state.cache_root.as_ref().clone();
    let result = tokio::task::spawn_blocking(move || {
        let cache = TrackCache::open(root)?;
        let mut writer = cache.begin_write(request)?;
        writer.write_all(&audio)?;
        writer.finish()
    })
    .await;
    match result {
        Ok(Ok(saved)) => (
            StatusCode::CREATED,
            Json(json!({ "imported": true, "bytes": saved.bytes })),
        )
            .into_response(),
        Ok(Err(error)) => {
            tracing::warn!(track_id = metadata.track_id, %error, "shared cache import failed");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not import the cached track",
            )
        }
        Err(error) => {
            tracing::warn!(track_id = metadata.track_id, %error, "shared cache import worker failed");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not import the cached track",
            )
        }
    }
}

async fn audio_handler(
    State(state): State<SharedCacheState>,
    Path(track_id): Path<i64>,
    Query(query): Query<AudioQuery>,
    method: Method,
    headers: HeaderMap,
) -> Response<Body> {
    if let Some(response) = disabled_response(&state) {
        return response;
    }
    let Some(quality) = AudioQuality::from_bitrate(query.quality) else {
        return json_error(StatusCode::BAD_REQUEST, "unsupported audio quality");
    };
    let key = CacheKey::new(track_id, quality);
    match lookup_cache(state.cache_root.as_ref().clone(), key).await {
        Ok(Some(lease)) => {
            return cached_audio_response(lease, method, headers.get(header::RANGE)).await;
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(track_id, %error, "shared audio cache lookup failed");
        }
    }

    let Some(source) = query.source.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    proxy_upstream(state, track_id, quality, query, &source, headers).await
}

async fn delete_audio_handler(
    State(state): State<SharedCacheState>,
    Path(track_id): Path<i64>,
    Query(query): Query<AudioQuery>,
) -> Response<Body> {
    if let Some(response) = disabled_response(&state) {
        return response;
    }
    let Some(quality) = AudioQuality::from_bitrate(query.quality) else {
        return json_error(StatusCode::BAD_REQUEST, "unsupported audio quality");
    };
    let root = state.cache_root.as_ref().clone();
    let result = tokio::task::spawn_blocking(move || {
        let cache = TrackCache::open(&root)?;
        let Some(lease) = cache.lookup(CacheKey::new(track_id, quality))? else {
            return Ok(false);
        };
        let metadata = *lease.metadata();
        drop(lease);
        cache.invalidate(&metadata)
    })
    .await;
    match result {
        Ok(Ok(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(error)) => {
            tracing::warn!(track_id, %error, "shared audio cache invalidation failed");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not invalidate the cached track",
            )
        }
        Err(error) => {
            tracing::warn!(track_id, %error, "shared audio cache invalidation worker failed");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not invalidate the cached track",
            )
        }
    }
}

async fn lookup_cache(root: PathBuf, key: CacheKey) -> Result<Option<CacheLease>, String> {
    tokio::task::spawn_blocking(move || {
        TrackCache::open(root)
            .and_then(|cache| cache.lookup(key))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn cached_audio_response(
    mut lease: CacheLease,
    method: Method,
    range: Option<&HeaderValue>,
) -> Response<Body> {
    let metadata = *lease.metadata();
    let selected = match parse_range(range, metadata.bytes) {
        Ok(selected) => selected,
        Err(()) => return range_not_satisfiable(metadata.bytes),
    };
    let (start, end) = selected.unwrap_or_else(|| (0, metadata.bytes.saturating_sub(1)));
    let length = if metadata.bytes == 0 {
        0
    } else {
        end - start + 1
    };
    if start != 0 && lease.seek(SeekFrom::Start(start)).is_err() {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not read the cached track",
        );
    }
    let partial = selected.is_some();
    let body = if method == Method::HEAD || length == 0 {
        Body::empty()
    } else {
        lease_body(lease, length)
    };
    let mut response = Response::new(body);
    *response.status_mut() = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    insert_audio_headers(&mut response, metadata.codec, length);
    if partial {
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{}", metadata.bytes))
                .expect("valid content range"),
        );
    }
    response
}

fn lease_body(lease: CacheLease, remaining: u64) -> Body {
    let stream =
        futures_util::stream::try_unfold((lease, remaining), |(mut lease, remaining)| async move {
            if remaining == 0 {
                return Ok(None);
            }
            tokio::task::spawn_blocking(move || {
                let buffer_size = usize::try_from(remaining.min(64 * 1024)).unwrap_or(64 * 1024);
                let mut buffer = vec![0_u8; buffer_size];
                let read = lease.read(&mut buffer)?;
                if read == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "cached audio ended early",
                    ));
                }
                buffer.truncate(read);
                Ok(Some((
                    Bytes::from(buffer),
                    (lease, remaining - read as u64),
                )))
            })
            .await
            .map_err(io::Error::other)?
        });
    Body::from_stream(stream)
}

async fn proxy_upstream(
    state: SharedCacheState,
    track_id: i64,
    quality: AudioQuality,
    query: AudioQuery,
    source: &str,
    headers: HeaderMap,
) -> Response<Body> {
    let source = match Url::parse(source) {
        Ok(source) if matches!(source.scheme(), "http" | "https") => source,
        _ => return json_error(StatusCode::BAD_REQUEST, "invalid upstream audio URL"),
    };
    let mut request = state.client.get(source);
    if let Some(range) = headers.get(header::RANGE) {
        request = request.header(header::RANGE, range);
    }
    let upstream = match request.send().await {
        Ok(response) => response,
        Err(_) => return json_error(StatusCode::BAD_GATEWAY, "upstream audio is unavailable"),
    };
    let status = upstream.status();
    let upstream_headers = upstream.headers().clone();
    let full_range_bytes = full_content_range_bytes(&upstream_headers);
    let complete_response = status == StatusCode::OK
        || (status == StatusCode::PARTIAL_CONTENT && full_range_bytes.is_some());
    let expected_bytes = if status == StatusCode::OK {
        upstream.content_length()
    } else {
        full_range_bytes
    };
    let cache_sender = if query.cache.unwrap_or(false) && complete_response {
        let Some(codec) = query.codec.as_deref() else {
            return json_error(
                StatusCode::BAD_REQUEST,
                "audio codec is required for caching",
            );
        };
        let request = match cache_write_request(
            track_id,
            quality.bitrate(),
            codec,
            query.actual_bitrate.unwrap_or_else(|| quality.bitrate()),
            expected_bytes,
        ) {
            Ok(request) => request,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
        };
        Some(spawn_cache_writer(
            state.cache_root.as_ref().clone(),
            request,
        ))
    } else {
        None
    };

    let stream: UpstreamStream = Box::pin(upstream.bytes_stream());
    let body_stream = futures_util::stream::try_unfold(
        (stream, cache_sender),
        |(mut stream, sender)| async move {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    if let Some(sender) = sender.as_ref() {
                        let _ = sender.send(CacheWriteMessage::Chunk(chunk.clone())).await;
                    }
                    Ok(Some((chunk, (stream, sender))))
                }
                Some(Err(error)) => Err(io::Error::other(error)),
                None => {
                    if let Some(sender) = sender {
                        let _ = sender.send(CacheWriteMessage::Complete).await;
                    }
                    Ok(None)
                }
            }
        },
    );
    let mut response = Response::new(Body::from_stream(body_stream));
    *response.status_mut() = status;
    copy_upstream_audio_headers(&upstream_headers, response.headers_mut());
    response
}

fn spawn_cache_writer(
    root: PathBuf,
    request: CacheWriteRequest,
) -> mpsc::Sender<CacheWriteMessage> {
    let track_id = request.key.track_id;
    let (sender, mut receiver) = mpsc::channel(8);
    tokio::task::spawn_blocking(move || {
        let result = (|| {
            let cache = TrackCache::open(root)?;
            let mut writer = cache.begin_write(request)?;
            while let Some(message) = receiver.blocking_recv() {
                match message {
                    CacheWriteMessage::Chunk(bytes) => writer.write_all(&bytes)?,
                    CacheWriteMessage::Complete => {
                        writer.finish()?;
                        return Ok::<(), yesplaymusic_core::cache::CacheError>(());
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(track_id, %error, "shared audio cache write failed");
        }
    });
    sender
}

fn cache_write_request(
    track_id: i64,
    quality: u32,
    codec: &str,
    actual_bitrate: u32,
    expected_bytes: Option<u64>,
) -> Result<CacheWriteRequest, &'static str> {
    if track_id <= 0 {
        return Err("invalid track ID");
    }
    let Some(quality) = AudioQuality::from_bitrate(quality) else {
        return Err("unsupported audio quality");
    };
    let codec = codec
        .parse::<AudioCodec>()
        .map_err(|_| "unsupported audio codec")?;
    let mut request =
        CacheWriteRequest::new(CacheKey::new(track_id, quality), codec, actual_bitrate);
    if let Some(expected_bytes) = expected_bytes {
        request = request.with_expected_bytes(expected_bytes);
    }
    Ok(request)
}

fn parse_range(range: Option<&HeaderValue>, total: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(range) = range else {
        return Ok(None);
    };
    let value = range.to_str().map_err(|_| ())?;
    let value = value.strip_prefix("bytes=").ok_or(())?;
    if value.contains(',') || total == 0 {
        return Err(());
    }
    let (start, end) = value.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let length = suffix.min(total);
        return Ok(Some((total - length, total - 1)));
    }
    let start = start.parse::<u64>().map_err(|_| ())?;
    if start >= total {
        return Err(());
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<u64>().map_err(|_| ())?.min(total - 1)
    };
    if end < start {
        return Err(());
    }
    Ok(Some((start, end)))
}

fn full_content_range_bytes(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get(header::CONTENT_RANGE)?.to_str().ok()?;
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    let total = total.parse::<u64>().ok()?;
    (start == 0 && end.checked_add(1) == Some(total)).then_some(total)
}

fn range_not_satisfiable(total: u64) -> Response<Body> {
    let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    response.headers_mut().insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{total}")).expect("valid content range"),
    );
    response
}

fn insert_audio_headers(response: &mut Response<Body>, codec: AudioCodec, length: u64) {
    let content_type = match codec {
        AudioCodec::Mp3 => "audio/mpeg",
        AudioCodec::Flac => "audio/flac",
        AudioCodec::Aac => "audio/aac",
        AudioCodec::M4a => "audio/mp4",
    };
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).expect("valid content length"),
    );
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response.headers_mut().insert(
        AUDIO_CODEC_HEADER,
        HeaderValue::from_static(codec.extension()),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

fn copy_upstream_audio_headers(source: &HeaderMap, target: &mut HeaderMap) {
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
        header::CACHE_CONTROL,
        header::ETAG,
        header::LAST_MODIFIED,
    ] {
        if let Some(value) = source.get(&name) {
            target.insert(name, value.clone());
        }
    }
}

fn disabled_response(state: &SharedCacheState) -> Option<Response<Body>> {
    (!state.is_enabled())
        .then(|| json_error(StatusCode::CONFLICT, "shared audio cache is not enabled"))
}

fn json_error(status: StatusCode, message: &'static str) -> Response<Body> {
    (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        Json(json!({ "message": message })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        sync::atomic::{AtomicUsize, Ordering},
    };

    use axum::{body::to_bytes, routing::get, Router};
    use http_body_util::BodyExt;
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    use super::*;

    async fn enable(app: &Router) {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::PUT)
                    .uri("/native/shared-cache/settings")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn status(app: &Router) -> serde_json::Value {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/native/shared-cache/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn terminal_cache_detection_requires_real_cached_audio() {
        let directory = tempdir().unwrap();
        let base_root = directory.path().join("ypm");
        // The TUI creates this skeleton on every launch, even with nothing cached.
        TrackCache::open(base_root.join("audio")).unwrap();
        let app = router(SharedCacheState::testing(base_root.clone()));
        assert_eq!(status(&app).await["terminalCacheDetected"], false);

        let cache = TrackCache::open(base_root.join("audio")).unwrap();
        let request = CacheWriteRequest::new(
            CacheKey::new(99, AudioQuality::High320),
            AudioCodec::Mp3,
            320_000,
        );
        let mut writer = cache.begin_write(request).unwrap();
        writer.write_all(b"terminal audio").unwrap();
        writer.finish().unwrap();
        assert_eq!(status(&app).await["terminalCacheDetected"], true);
    }

    #[tokio::test]
    async fn audio_proxy_prefers_shared_cache_over_upstream() {
        let directory = tempdir().unwrap();
        let base_root = directory.path().join("ypm");
        let cache = TrackCache::open(base_root.join("audio")).unwrap();
        let request = CacheWriteRequest::new(
            CacheKey::new(42, AudioQuality::High320),
            AudioCodec::Mp3,
            320_000,
        );
        let mut writer = cache.begin_write(request).unwrap();
        writer.write_all(b"cached audio").unwrap();
        writer.finish().unwrap();

        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let handler_hits = upstream_hits.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = Router::new().route(
            "/audio",
            get(move || {
                handler_hits.fetch_add(1, Ordering::Relaxed);
                async { "network audio" }
            }),
        );
        let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

        let app = router(SharedCacheState::testing(base_root));
        enable(&app).await;
        let source = format!("http://{address}/audio");
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("quality", "320000")
            .append_pair("source", &source)
            .append_pair("codec", "mp3")
            .append_pair("actualBitrate", "320000")
            .append_pair("cache", "true")
            .finish();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/native/shared-cache/audio/42?{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "cached audio"
        );
        assert_eq!(upstream_hits.load(Ordering::Relaxed), 0);
        task.abort();
    }

    #[tokio::test]
    async fn completed_proxy_download_is_published_to_shared_cache() {
        let directory = tempdir().unwrap();
        let base_root = directory.path().join("ypm");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = Router::new().route("/audio", get(|| async { "downloaded audio" }));
        let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

        let app = router(SharedCacheState::testing(base_root.clone()));
        enable(&app).await;
        let source = format!("http://{address}/audio");
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("quality", "128000")
            .append_pair("source", &source)
            .append_pair("codec", "mp3")
            .append_pair("actualBitrate", "128000")
            .append_pair("cache", "true")
            .finish();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/native/shared-cache/audio/43?{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "downloaded audio"
        );

        let key = CacheKey::new(43, AudioQuality::Low128);
        // Ten seconds, not two: a loaded Windows CI runner has blown the
        // shorter window while the writer task was starved.
        let mut lease = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(lease) = TrackCache::open(base_root.join("audio"))
                    .unwrap()
                    .lookup(key)
                    .unwrap()
                {
                    break lease;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("completed proxy response should publish the cache");
        let mut audio = Vec::new();
        lease.read_to_end(&mut audio).unwrap();
        assert_eq!(audio, b"downloaded audio");
        task.abort();
    }

    #[tokio::test]
    async fn import_endpoint_publishes_through_track_cache() {
        let directory = tempdir().unwrap();
        let base_root = directory.path().join("ypm");
        let app = router(SharedCacheState::testing(base_root.clone()));
        enable(&app).await;

        let boundary = "ypm-test-boundary";
        let metadata = json!({
            "trackId": 77,
            "quality": 192000,
            "codec": "mp3",
            "actualBitrate": 191000,
            "name": "Imported",
            "artist": "Tester"
        });
        let mut body = Vec::new();
        write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n"
        )
        .unwrap();
        write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"77.mp3\"\r\nContent-Type: audio/mpeg\r\n\r\n"
        )
        .unwrap();
        body.extend_from_slice(b"imported audio");
        write!(body, "\r\n--{boundary}--\r\n").unwrap();

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::POST)
                    .uri("/native/shared-cache/import")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&to_bytes(response.into_body(), 4096).await.unwrap())
        );

        let cache = TrackCache::open(base_root.join("audio")).unwrap();
        let mut lease = cache
            .lookup(CacheKey::new(77, AudioQuality::Medium192))
            .unwrap()
            .unwrap();
        let mut audio = Vec::new();
        lease.read_to_end(&mut audio).unwrap();
        assert_eq!(audio, b"imported audio");
    }

    #[test]
    fn parses_single_http_byte_ranges() {
        assert_eq!(parse_range(None, 10), Ok(None));
        assert_eq!(
            parse_range(Some(&HeaderValue::from_static("bytes=2-5")), 10),
            Ok(Some((2, 5)))
        );
        assert_eq!(
            parse_range(Some(&HeaderValue::from_static("bytes=-3")), 10),
            Ok(Some((7, 9)))
        );
        assert_eq!(
            parse_range(Some(&HeaderValue::from_static("bytes=10-")), 10),
            Err(())
        );
    }

    #[test]
    fn only_a_full_206_range_is_cacheable() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 0-9/10"),
        );
        assert_eq!(full_content_range_bytes(&headers), Some(10));

        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 5-9/10"),
        );
        assert_eq!(full_content_range_bytes(&headers), None);
    }
}
