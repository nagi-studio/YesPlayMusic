use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    extract::{DefaultBodyLimit, Request, State},
    http::{header, HeaderMap, HeaderValue, Response, StatusCode},
    routing::{on, post, MethodFilter},
    Router,
};
use ncm_api_rs::{ApiClient, ApiResponse, CryptoType, NcmError, Query, RequestOption};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    cloud::{upload_song, CloudState},
    session::desktop_session_expiry_cookies,
};

const FRONTEND_ROUTE_COUNT: usize = 57;
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const CLOUD_MULTIPART_BODY_LIMIT_BYTES: usize = 513 * 1024 * 1024;
const NCM_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CLOUD_LYRIC_PATH: &str = "/api/cloud/lyric/get";
const CLOUD_LYRIC_CRYPTO: &str = "eapi";
const ROUTE_MANIFEST: &str = include_str!("../../../src/sidecar-route-manifest.json");

#[derive(Debug, thiserror::Error)]
pub enum NcmRouterError {
    #[error("invalid NCM route manifest: {0}")]
    InvalidManifest(String),
    #[error("cannot decode NCM route manifest: {0}")]
    DecodeManifest(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct NcmState {
    backend: Arc<dyn NcmBackend>,
    cloud: CloudState,
    request_timeout: Duration,
}

impl NcmState {
    pub fn production(client: Arc<ApiClient>) -> Self {
        Self {
            backend: Arc::new(ProductionBackend {
                client: client.clone(),
            }),
            cloud: CloudState::production(client),
            request_timeout: NCM_REQUEST_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn testing(backend: Arc<dyn NcmBackend>) -> Self {
        Self::testing_with_timeout(backend, NCM_REQUEST_TIMEOUT)
    }

    #[cfg(test)]
    fn testing_with_timeout(backend: Arc<dyn NcmBackend>, request_timeout: Duration) -> Self {
        Self {
            backend,
            cloud: CloudState::production(Arc::new(ApiClient::new(None))),
            request_timeout,
        }
    }
}

#[derive(Clone)]
struct RuntimeState {
    backend: Arc<dyn NcmBackend>,
    adapters: Arc<HashMap<String, Arc<str>>>,
    request_timeout: Duration,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouteManifestEntry {
    method: String,
    path: String,
    rust_adapter: String,
    api_forward: Option<ApiForward>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiForward {
    allowed_paths: Vec<String>,
    crypto: String,
}

macro_rules! define_production_dispatch {
    ($($method:ident),+ $(,)?) => {
        const SUPPORTED_ADAPTERS: &[&str] = &[
            $(concat!("ncm::", stringify!($method))),+
        ];

        impl ProductionBackend {
            async fn dispatch_api(
                &self,
                adapter: &str,
                query: &Query,
            ) -> Result<ApiResponse, NcmError> {
                match adapter {
                    $(concat!("ncm::", stringify!($method)) => self.client.$method(query).await,)+
                    _ => Err(NcmError::Unknown(format!(
                        "unsupported manifest adapter: {adapter}"
                    ))),
                }
            }
        }
    };
}

define_production_dispatch!(
    album,
    album_detail_dynamic,
    album_new,
    album_sub,
    album_sublist,
    api,
    artist_album,
    artist_mv,
    artist_sub,
    artist_sublist,
    artists,
    daily_signin,
    fm_trash,
    like,
    likelist,
    login,
    login_cellphone,
    login_qr_check,
    login_qr_create,
    login_qr_key,
    login_refresh,
    logout,
    lyric,
    mv_detail,
    mv_sub,
    mv_sublist,
    mv_url,
    personal_fm,
    personalized,
    playlist_catlist,
    playlist_create,
    playlist_delete,
    playlist_detail,
    playlist_subscribe,
    playlist_tracks,
    playmode_intelligence_list,
    recommend_resource,
    recommend_songs,
    scrobble,
    search,
    simi_artist,
    simi_mv,
    song_detail,
    song_url,
    top_playlist,
    top_playlist_highquality,
    top_song,
    toplist,
    toplist_artist,
    user_account,
    user_cloud,
    user_cloud_del,
    user_cloud_detail,
    user_detail,
    user_playlist,
    user_record,
    login_status,
);

struct ProductionBackend {
    client: Arc<ApiClient>,
}

impl ProductionBackend {
    async fn playlist_subscribe_node_compatible(
        &self,
        query: &Query,
    ) -> Result<ApiResponse, NcmError> {
        let subscribing = query.get_or("t", "1") == "1";
        let operation = if subscribing {
            "subscribe"
        } else {
            "unsubscribe"
        };
        let mut data = json!({ "id": query.get_or("id", "0") });
        if subscribing {
            data["checkToken"] = Value::String(ncm_api_rs::util::config::CHECK_TOKEN.to_owned());
        }
        self.client
            .request(
                &format!("/api/playlist/{operation}"),
                data,
                RequestOption {
                    crypto: CryptoType::Eapi,
                    cookie: query.cookie.clone(),
                    ua: query.ua.clone(),
                    proxy: query.proxy.clone(),
                    real_ip: query.real_ip.clone(),
                    random_cn_ip: query.random_cn_ip,
                    e_r: query.e_r,
                    domain: query.domain.clone(),
                    check_token: true,
                },
            )
            .await
    }
}

#[async_trait]
trait NcmBackend: Send + Sync {
    async fn dispatch(&self, adapter: &str, query: &Query) -> Result<ApiResponse, NcmError>;
}

#[async_trait]
impl NcmBackend for ProductionBackend {
    async fn dispatch(&self, adapter: &str, query: &Query) -> Result<ApiResponse, NcmError> {
        if adapter == "ncm::playlist_subscribe" {
            self.playlist_subscribe_node_compatible(query).await
        } else {
            self.dispatch_api(adapter, query).await
        }
    }
}

fn parse_manifest() -> Result<Vec<RouteManifestEntry>, NcmRouterError> {
    let manifest: Vec<RouteManifestEntry> = serde_json::from_str(ROUTE_MANIFEST)?;
    if manifest.len() != FRONTEND_ROUTE_COUNT {
        return Err(NcmRouterError::InvalidManifest(format!(
            "expected {FRONTEND_ROUTE_COUNT} frontend routes, found {}",
            manifest.len()
        )));
    }

    let mut paths = std::collections::HashSet::with_capacity(manifest.len());
    for entry in &manifest {
        if !paths.insert(entry.path.as_str()) {
            return Err(NcmRouterError::InvalidManifest(format!(
                "duplicate frontend path {}",
                entry.path
            )));
        }
        if !matches!(entry.method.as_str(), "GET" | "POST") {
            return Err(NcmRouterError::InvalidManifest(format!(
                "unsupported method {} for {}",
                entry.method, entry.path
            )));
        }
        if entry.rust_adapter != "ncm::cloud"
            && !SUPPORTED_ADAPTERS.contains(&entry.rust_adapter.as_str())
        {
            return Err(NcmRouterError::InvalidManifest(format!(
                "unsupported adapter {} for {}",
                entry.rust_adapter, entry.path
            )));
        }
    }

    let cloud = manifest
        .iter()
        .find(|entry| entry.path == "/cloud")
        .ok_or_else(|| NcmRouterError::InvalidManifest("missing /cloud route".to_owned()))?;
    if cloud.method != "POST" || cloud.rust_adapter != "ncm::cloud" {
        return Err(NcmRouterError::InvalidManifest(
            "/cloud must be POST ncm::cloud".to_owned(),
        ));
    }

    let api = manifest
        .iter()
        .find(|entry| entry.path == "/api")
        .ok_or_else(|| NcmRouterError::InvalidManifest("missing /api route".to_owned()))?;
    let forward = api.api_forward.as_ref().ok_or_else(|| {
        NcmRouterError::InvalidManifest("/api is missing its forwarding allowlist".to_owned())
    })?;
    if api.method != "GET"
        || api.rust_adapter != "ncm::api"
        || forward.allowed_paths.as_slice() != [CLOUD_LYRIC_PATH]
        || forward.crypto != CLOUD_LYRIC_CRYPTO
    {
        return Err(NcmRouterError::InvalidManifest(
            "/api may only forward /api/cloud/lyric/get with eapi".to_owned(),
        ));
    }

    Ok(manifest)
}

pub fn router(state: NcmState) -> Result<Router, NcmRouterError> {
    let manifest = parse_manifest()?;
    let mut adapters = HashMap::with_capacity(manifest.len() + 1);
    let mut app: Router<RuntimeState> = Router::new();

    for entry in manifest {
        if entry.path == "/cloud" {
            continue;
        }
        adapters.insert(entry.path.clone(), Arc::<str>::from(entry.rust_adapter));
        let method = match entry.method.as_str() {
            "GET" => MethodFilter::GET,
            "POST" => MethodFilter::POST,
            _ => unreachable!("manifest methods were validated"),
        };
        app = app.route(&entry.path, on(method, dispatch_request));
    }

    adapters.insert(
        "/login/status".to_owned(),
        Arc::<str>::from("ncm::login_status"),
    );
    app = app
        .route("/login/status", on(MethodFilter::GET, dispatch_request))
        .route("/native/logout-session", post(logout_session));

    let runtime = RuntimeState {
        backend: state.backend,
        adapters: Arc::new(adapters),
        request_timeout: state.request_timeout,
    };
    let api_routes = app.with_state(runtime);
    let cloud_route = Router::new()
        .route("/cloud", post(upload_song))
        .layer(DefaultBodyLimit::max(CLOUD_MULTIPART_BODY_LIMIT_BYTES))
        .with_state(state.cloud);
    Ok(api_routes.merge(cloud_route))
}

#[derive(Debug, thiserror::Error)]
enum RequestError {
    #[error("request body is too large")]
    BodyTooLarge,
    #[error("invalid query string: {0}")]
    Query(String),
    #[error("invalid request body: {0}")]
    Body(String),
    #[error("unsupported request content type")]
    UnsupportedContentType,
    #[error("this API forward target is not allowed")]
    ApiForwardDenied,
    #[error("named NCM routes do not allow domain overrides")]
    DomainOverrideDenied,
}

impl RequestError {
    fn into_response(self) -> Response<Body> {
        let status = match self {
            Self::BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::ApiForwardDenied => StatusCode::FORBIDDEN,
            Self::Query(_) | Self::Body(_) | Self::DomainOverrideDenied => StatusCode::BAD_REQUEST,
        };
        json_response(
            status,
            json!({ "code": status.as_u16(), "message": self.to_string() }),
        )
    }
}

fn insert_pairs(
    values: &mut HashMap<String, Value>,
    encoded: &str,
) -> Result<(), serde_urlencoded::de::Error> {
    for (key, value) in serde_urlencoded::from_str::<Vec<(String, String)>>(encoded)? {
        values.insert(key, Value::String(value));
    }
    Ok(())
}

fn merge_json_body(values: &mut HashMap<String, Value>, bytes: &[u8]) -> Result<(), RequestError> {
    let Value::Object(body) = serde_json::from_slice::<Value>(bytes)
        .map_err(|error| RequestError::Body(error.to_string()))?
    else {
        return Err(RequestError::Body(
            "JSON request body must be an object".to_owned(),
        ));
    };
    values.extend(body);
    Ok(())
}

fn value_to_parameter(value: Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        value @ (Value::Array(_) | Value::Object(_)) => Some(value.to_string()),
    }
}

fn take_string(values: &mut HashMap<String, Value>, key: &str) -> Option<String> {
    values.remove(key).and_then(value_to_parameter)
}

fn take_bool(values: &mut HashMap<String, Value>, key: &str) -> Option<bool> {
    match values.remove(key)? {
        Value::Bool(value) => Some(value),
        Value::Number(value) => Some(value.as_i64() == Some(1)),
        Value::String(value) if value == "true" || value == "1" => Some(true),
        Value::String(value) if value == "false" || value == "0" => Some(false),
        _ => None,
    }
}

async fn request_query(request: Request) -> Result<Query, RequestError> {
    let (parts, body) = request.into_parts();
    let mut values = HashMap::new();
    if let Some(encoded) = parts.uri.query() {
        insert_pairs(&mut values, encoded)
            .map_err(|error| RequestError::Query(error.to_string()))?;
    }

    let bytes = to_bytes(body, MAX_REQUEST_BODY_BYTES)
        .await
        .map_err(|_| RequestError::BodyTooLarge)?;
    if !bytes.is_empty() {
        let content_type = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        match content_type {
            "application/json" => merge_json_body(&mut values, &bytes)?,
            "application/x-www-form-urlencoded" => insert_pairs(
                &mut values,
                std::str::from_utf8(&bytes)
                    .map_err(|error| RequestError::Body(error.to_string()))?,
            )
            .map_err(|error| RequestError::Body(error.to_string()))?,
            _ => return Err(RequestError::UnsupportedContentType),
        }
    }

    let cookie = take_string(&mut values, "cookie").or_else(|| {
        parts
            .headers
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    });
    let mut query = Query::new();
    query.cookie = cookie;
    query.proxy = take_string(&mut values, "proxy");
    query.real_ip = take_string(&mut values, "realIP");
    query.random_cn_ip = take_bool(&mut values, "randomCNIP").unwrap_or(false);
    query.ua = take_string(&mut values, "ua");
    query.e_r = take_bool(&mut values, "e_r");
    query.domain = take_string(&mut values, "domain");
    query.params = values
        .into_iter()
        .filter_map(|(key, value)| value_to_parameter(value).map(|value| (key, value)))
        .collect();
    Ok(query)
}

fn api_forward_allowed(query: &Query) -> bool {
    query.get("uri") == Some(CLOUD_LYRIC_PATH)
        && query.get("crypto") == Some(CLOUD_LYRIC_CRYPTO)
        && query.domain.is_none()
}

async fn dispatch_with_playlist_compatibility(
    backend: &dyn NcmBackend,
    adapter: &str,
    query: &Query,
) -> Result<ApiResponse, NcmError> {
    let response = backend.dispatch(adapter, query).await;
    if adapter != "ncm::playlist_tracks"
        || !matches!(&response, Err(NcmError::Api { code: 512, .. }))
    {
        return response;
    }
    let Some(tracks) = query.get("tracks").filter(|tracks| !tracks.is_empty()) else {
        return response;
    };
    let mut retry = query.clone();
    retry
        .params
        .insert("tracks".to_owned(), format!("{tracks},{tracks}"));
    backend.dispatch(adapter, &retry).await
}

async fn dispatch_request(State(state): State<RuntimeState>, request: Request) -> Response<Body> {
    let path = request.uri().path();
    let Some(adapter) = state.adapters.get(path).cloned() else {
        return json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": 404, "msg": "Not Found" }),
        );
    };
    let query = match request_query(request).await {
        Ok(query) => query,
        Err(error) => return error.into_response(),
    };
    if adapter.as_ref() == "ncm::api" && !api_forward_allowed(&query) {
        return RequestError::ApiForwardDenied.into_response();
    }
    if adapter.as_ref() != "ncm::api" && query.domain.is_some() {
        return RequestError::DomainOverrideDenied.into_response();
    }
    let suppress_cookies = query
        .get("noCookie")
        .is_some_and(|value| matches!(value, "true" | "1"));
    match tokio::time::timeout(
        state.request_timeout,
        dispatch_with_playlist_compatibility(state.backend.as_ref(), adapter.as_ref(), &query),
    )
    .await
    {
        Ok(Ok(response)) => api_response(adapter.as_ref(), response, suppress_cookies),
        Ok(Err(error)) => ncm_error_response(error),
        Err(_) => ncm_timeout_response(),
    }
}

async fn logout_session(State(state): State<RuntimeState>, headers: HeaderMap) -> Response<Body> {
    let mut query = Query::new();
    query.cookie = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let backend = state.backend;
    let request_timeout = state.request_timeout;
    tokio::spawn(async move {
        match tokio::time::timeout(request_timeout, backend.dispatch("ncm::logout", &query)).await {
            Ok(Ok(_)) => {}
            Ok(Err(_)) => tracing::warn!("remote logout failed"),
            Err(_) => tracing::warn!("remote logout timed out"),
        }
    });

    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    for cookie in desktop_session_expiry_cookies() {
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie).expect("static expiry cookie is valid"),
        );
    }
    response
}

fn recursively_normalize_avatar_ids(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(legacy) = object.remove("avatarImgId_str") {
                object.entry("avatarImgIdStr").or_insert(legacy);
            }
            for child in object.values_mut() {
                recursively_normalize_avatar_ids(child);
            }
        }
        Value::Array(array) => {
            for child in array {
                recursively_normalize_avatar_ids(child);
            }
        }
        _ => {}
    }
}

fn body_code_is(body: &Value, expected: i64) -> bool {
    body.get("code")
        .and_then(|code| code.as_i64().or_else(|| code.as_str()?.parse().ok()))
        == Some(expected)
}

fn normalize_api_body(adapter: &str, status: i64, body: &mut Value, cookies: &[String]) {
    match adapter {
        "ncm::login" | "ncm::login_cellphone" => {
            recursively_normalize_avatar_ids(body);
            if body_code_is(body, 200) {
                if let Value::Object(object) = body {
                    object.insert("cookie".to_owned(), Value::String(cookies.join(";")));
                }
            }
        }
        "ncm::login_qr_check" => {
            recursively_normalize_avatar_ids(body);
            if let Value::Object(object) = body {
                object.insert("cookie".to_owned(), Value::String(cookies.join(";")));
            }
        }
        "ncm::login_refresh" if body_code_is(body, 200) => {
            if let Value::Object(object) = body {
                object.insert("cookie".to_owned(), Value::String(cookies.join(";")));
            }
        }
        "ncm::login_status" if body_code_is(body, 200) => {
            *body = json!({ "data": std::mem::take(body) });
        }
        "ncm::login_qr_key" => {
            *body = json!({ "data": std::mem::take(body), "code": 200 });
        }
        "ncm::playlist_tracks" => {
            *body = json!({
                "status": status,
                "body": std::mem::take(body),
                "cookie": cookies,
            });
        }
        "ncm::top_playlist" | "ncm::user_detail" => recursively_normalize_avatar_ids(body),
        _ => {}
    }
}

fn api_response(
    adapter: &str,
    mut response: ApiResponse,
    suppress_cookies: bool,
) -> Response<Body> {
    normalize_api_body(
        adapter,
        response.status,
        &mut response.body,
        &response.cookie,
    );
    let status = u16::try_from(response.status)
        .ok()
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    let mut result = json_response(status, response.body);
    if !suppress_cookies {
        for cookie in response.cookie {
            match HeaderValue::from_str(&cookie) {
                Ok(value) => {
                    result.headers_mut().append(header::SET_COOKIE, value);
                }
                Err(error) => {
                    tracing::warn!(error = %error, "upstream returned an invalid cookie header")
                }
            }
        }
    }
    result
}

fn ncm_error_response(error: NcmError) -> Response<Body> {
    let (status, code, message) = match error {
        NcmError::AuthRequired(_) => (StatusCode::UNAUTHORIZED, 301_i64, "需要登录".to_owned()),
        NcmError::InvalidParam(message) => (StatusCode::BAD_REQUEST, 400, message),
        NcmError::RateLimited(message) => (StatusCode::SERVICE_UNAVAILABLE, 503, message),
        NcmError::Timeout(message) => (StatusCode::GATEWAY_TIMEOUT, 504, message),
        NcmError::Http(error) if error.is_timeout() => (
            StatusCode::GATEWAY_TIMEOUT,
            504,
            "upstream request timed out".to_owned(),
        ),
        NcmError::Api { code, msg } => {
            let status = u16::try_from(code)
                .ok()
                .filter(|code| (400..=599).contains(code))
                .and_then(|code| StatusCode::from_u16(code).ok())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            (status, code, msg)
        }
        NcmError::Http(_) | NcmError::Crypto(_) | NcmError::Json(_) | NcmError::Unknown(_) => (
            StatusCode::BAD_GATEWAY,
            502,
            "upstream NCM request failed".to_owned(),
        ),
    };
    tracing::warn!(
        http_status = status.as_u16(),
        ncm_code = code,
        "NCM request failed"
    );
    json_response(status, json!({ "code": code, "msg": message }))
}

fn ncm_timeout_response() -> Response<Body> {
    tracing::warn!(http_status = 504, ncm_code = 504, "NCM request timed out");
    json_response(
        StatusCode::GATEWAY_TIMEOUT,
        json!({ "code": 504, "msg": "upstream NCM request timed out" }),
    )
}

fn json_response(status: StatusCode, body: Value) -> Response<Body> {
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeSet, VecDeque},
        future::pending,
        sync::Mutex,
        time::Duration,
    };

    use axum::{http::Method, middleware, routing::any};
    use http_body_util::BodyExt;
    use reqwest::cookie::{CookieStore, Jar};
    use tokio::{net::TcpListener, sync::mpsc};
    use tower::ServiceExt;

    use crate::{
        renderer::{self, ApiProxy},
        session::{self, RequestBoundary},
    };

    use super::*;

    #[derive(Debug)]
    struct RecordedCall {
        adapter: String,
        params: HashMap<String, String>,
        cookie: Option<String>,
        proxy: Option<String>,
        real_ip: Option<String>,
    }

    struct RecordingBackend {
        calls: mpsc::UnboundedSender<RecordedCall>,
        responses: Mutex<VecDeque<Result<ApiResponse, NcmError>>>,
        hang_on_logout: bool,
    }

    #[derive(Clone)]
    struct UpstreamCapture {
        calls: mpsc::UnboundedSender<CapturedUpstreamRequest>,
    }

    #[derive(Debug)]
    struct CapturedUpstreamRequest {
        path: String,
        cookie: Option<String>,
        body: Vec<u8>,
    }

    #[async_trait]
    impl NcmBackend for RecordingBackend {
        async fn dispatch(&self, adapter: &str, query: &Query) -> Result<ApiResponse, NcmError> {
            let _ = self.calls.send(RecordedCall {
                adapter: adapter.to_owned(),
                params: query.params.clone(),
                cookie: query.cookie.clone(),
                proxy: query.proxy.clone(),
                real_ip: query.real_ip.clone(),
            });
            if self.hang_on_logout && adapter == "ncm::logout" {
                return pending::<Result<ApiResponse, NcmError>>().await;
            }
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| {
                    Ok(ApiResponse {
                        status: 200,
                        body: json!({ "code": 200 }),
                        cookie: Vec::new(),
                    })
                })
        }
    }

    struct PendingBackend;

    #[derive(Deserialize)]
    struct AudioQualityCase {
        setting: Value,
        wire: String,
    }

    #[async_trait]
    impl NcmBackend for PendingBackend {
        async fn dispatch(&self, _: &str, _: &Query) -> Result<ApiResponse, NcmError> {
            pending::<Result<ApiResponse, NcmError>>().await
        }
    }

    fn recording_router(
        response: Result<ApiResponse, NcmError>,
        hang_on_logout: bool,
    ) -> (Router, mpsc::UnboundedReceiver<RecordedCall>) {
        recording_router_with_responses([response], hang_on_logout)
    }

    fn recording_router_with_responses(
        responses: impl IntoIterator<Item = Result<ApiResponse, NcmError>>,
        hang_on_logout: bool,
    ) -> (Router, mpsc::UnboundedReceiver<RecordedCall>) {
        let (calls, receiver) = mpsc::unbounded_channel();
        let backend = Arc::new(RecordingBackend {
            calls,
            responses: Mutex::new(responses.into_iter().collect()),
            hang_on_logout,
        });
        (router(NcmState::testing(backend)).unwrap(), receiver)
    }

    async fn capture_upstream(
        State(state): State<UpstreamCapture>,
        request: Request,
    ) -> Response<Body> {
        let (parts, body) = request.into_parts();
        let body = to_bytes(body, MAX_REQUEST_BODY_BYTES)
            .await
            .unwrap()
            .to_vec();
        state
            .calls
            .send(CapturedUpstreamRequest {
                path: parts.uri.path().to_owned(),
                cookie: parts
                    .headers
                    .get(header::COOKIE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
                body,
            })
            .unwrap();
        json_response(StatusCode::OK, json!({ "code": 200 }))
    }

    async fn start_upstream_capture() -> (
        String,
        mpsc::UnboundedReceiver<CapturedUpstreamRequest>,
        tokio::task::JoinHandle<()>,
    ) {
        let (calls, receiver) = mpsc::unbounded_channel();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .fallback(any(capture_upstream))
            .with_state(UpstreamCapture { calls });
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (origin, receiver, task)
    }

    fn assert_cookie_pairs(actual: Option<&str>, expected: &[&str]) {
        let actual = actual
            .expect("request must include the cookie jar")
            .split(';')
            .map(str::trim)
            .collect::<BTreeSet<_>>();
        let expected = expected.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);
    }

    async fn response_json(response: Response<Body>) -> Value {
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
    }

    fn multipart_song_body(size: usize, boundary: &str) -> Vec<u8> {
        let head = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"songFile\"; filename=\"fixture.mp3\"\r\nContent-Type: audio/mpeg\r\n\r\n"
        );
        let tail = format!("\r\n--{boundary}--\r\n");
        let mut body = Vec::with_capacity(head.len() + size + tail.len());
        body.extend_from_slice(head.as_bytes());
        body.resize(body.len() + size, 0);
        body.extend_from_slice(tail.as_bytes());
        body
    }

    #[tokio::test]
    async fn post_login_preserves_transport_context_and_normalizes_the_response() {
        let response = ApiResponse {
            status: 200,
            body: json!({
                "code": 200,
                "profile": { "avatarImgId_str": "9007199254740993" }
            }),
            cookie: vec![
                "MUSIC_U=secret; Path=/".to_owned(),
                "__csrf=token; Path=/".to_owned(),
            ],
        };
        let (app, mut calls) = recording_router(Ok(response), false);
        let wrong_method = Request::builder()
            .uri("/login")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(wrong_method).await.unwrap().status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert!(calls.try_recv().is_err());

        let request = Request::builder()
            .method(Method::POST)
            .uri("/login?proxy=http%3A%2F%2F127.0.0.1%3A8888&realIP=211.161.244.70&source=query")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, "MUSIC_U=old; __csrf=old-token")
            .body(Body::from(r#"{"email":"me@example.com","source":"body"}"#))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .count(),
            2
        );
        let body = response_json(response).await;
        assert_eq!(
            body["cookie"],
            "MUSIC_U=secret; Path=/;__csrf=token; Path=/"
        );
        assert_eq!(body["profile"]["avatarImgIdStr"], "9007199254740993");
        assert!(body["profile"].get("avatarImgId_str").is_none());

        let call = calls.recv().await.unwrap();
        assert_eq!(call.adapter, "ncm::login");
        assert_eq!(call.params["email"], "me@example.com");
        assert_eq!(call.params["source"], "body");
        assert_eq!(
            call.cookie.as_deref(),
            Some("MUSIC_U=old; __csrf=old-token")
        );
        assert_eq!(call.proxy.as_deref(), Some("http://127.0.0.1:8888"));
        assert_eq!(call.real_ip.as_deref(), Some("211.161.244.70"));
    }

    #[tokio::test]
    async fn cookie_jar_survives_login_refresh_and_native_logout_through_both_http_servers() {
        let (calls, mut recorded_calls) = mpsc::unbounded_channel();
        let responses = [
            ApiResponse {
                status: 200,
                body: json!({ "code": 200 }),
                cookie: vec![
                    "MUSIC_U=login-session; Path=/".to_owned(),
                    "__csrf=login-csrf; Path=/".to_owned(),
                ],
            },
            ApiResponse {
                status: 200,
                body: json!({ "code": 200 }),
                cookie: Vec::new(),
            },
            ApiResponse {
                status: 200,
                body: json!({ "code": 200 }),
                cookie: vec![
                    "MUSIC_U=refreshed-session; Path=/".to_owned(),
                    "__csrf=refreshed-csrf; Path=/".to_owned(),
                ],
            },
            ApiResponse {
                status: 200,
                body: json!({ "code": 200 }),
                cookie: Vec::new(),
            },
            ApiResponse {
                status: 200,
                body: json!({ "code": 200 }),
                cookie: Vec::new(),
            },
            ApiResponse {
                status: 200,
                body: json!({ "code": 200 }),
                cookie: Vec::new(),
            },
        ];
        let backend = Arc::new(RecordingBackend {
            calls,
            responses: Mutex::new(responses.into_iter().map(Ok).collect()),
            hang_on_logout: false,
        });

        let api_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_port = api_listener.local_addr().unwrap().port();
        let renderer_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let renderer_port = renderer_listener.local_addr().unwrap().port();
        let renderer_origin = format!("http://127.0.0.1:{renderer_port}");
        let native_token = "native-cookie-flow-secret".to_owned();

        let api =
            router(NcmState::testing(backend))
                .unwrap()
                .layer(middleware::from_fn_with_state(
                    RequestBoundary::new([renderer_origin.clone()], Some(native_token.clone())),
                    session::enforce_local_boundary,
                ));
        let api_task = tokio::spawn(async move { axum::serve(api_listener, api).await.unwrap() });

        let proxy = ApiProxy::new(api_port, native_token).unwrap();
        let renderer = Router::new()
            .route("/api", any(renderer::proxy_api))
            .route("/api/{*path}", any(renderer::proxy_api))
            .with_state(proxy)
            .layer(middleware::from_fn_with_state(
                RequestBoundary::new([renderer_origin.clone()], None),
                session::enforce_local_boundary,
            ));
        let renderer_task =
            tokio::spawn(async move { axum::serve(renderer_listener, renderer).await.unwrap() });

        let jar = Arc::new(Jar::default());
        let client = reqwest::Client::builder()
            .no_proxy()
            .cookie_provider(jar.clone())
            .build()
            .unwrap();

        let login = client
            .post(format!("{renderer_origin}/api/login"))
            .header(header::ORIGIN, &renderer_origin)
            .json(&json!({ "email": "fixture@example.com", "password": "fixture" }))
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        assert_eq!(
            login.headers().get_all(header::SET_COOKIE).iter().count(),
            2
        );
        assert!(login
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .all(|value| value.to_str().unwrap().contains("SameSite=Strict")));
        assert_eq!(login.json::<Value>().await.unwrap()["code"], 200);
        let login_call = recorded_calls.recv().await.unwrap();
        assert_eq!(login_call.adapter, "ncm::login");
        assert!(login_call.cookie.is_none());

        let account = client
            .get(format!("{renderer_origin}/api/user/account"))
            .header(header::ORIGIN, &renderer_origin)
            .send()
            .await
            .unwrap();
        assert_eq!(account.status(), StatusCode::OK);
        let _ = account.bytes().await.unwrap();
        let account_call = recorded_calls.recv().await.unwrap();
        assert_eq!(account_call.adapter, "ncm::user_account");
        assert_cookie_pairs(
            account_call.cookie.as_deref(),
            &["MUSIC_U=login-session", "__csrf=login-csrf"],
        );

        let refresh = client
            .post(format!("{renderer_origin}/api/login/refresh"))
            .header(header::ORIGIN, &renderer_origin)
            .send()
            .await
            .unwrap();
        assert_eq!(refresh.status(), StatusCode::OK);
        assert_eq!(
            refresh.headers().get_all(header::SET_COOKIE).iter().count(),
            2
        );
        let _ = refresh.bytes().await.unwrap();
        let refresh_call = recorded_calls.recv().await.unwrap();
        assert_eq!(refresh_call.adapter, "ncm::login_refresh");
        assert_cookie_pairs(
            refresh_call.cookie.as_deref(),
            &["MUSIC_U=login-session", "__csrf=login-csrf"],
        );

        let refreshed_account = client
            .get(format!("{renderer_origin}/api/user/account"))
            .header(header::ORIGIN, &renderer_origin)
            .send()
            .await
            .unwrap();
        assert_eq!(refreshed_account.status(), StatusCode::OK);
        let _ = refreshed_account.bytes().await.unwrap();
        let refreshed_account_call = recorded_calls.recv().await.unwrap();
        assert_eq!(refreshed_account_call.adapter, "ncm::user_account");
        assert_cookie_pairs(
            refreshed_account_call.cookie.as_deref(),
            &["MUSIC_U=refreshed-session", "__csrf=refreshed-csrf"],
        );

        let logout = client
            .post(format!("{renderer_origin}/api/native/logout-session"))
            .header(header::ORIGIN, &renderer_origin)
            .send()
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            logout.headers().get_all(header::SET_COOKIE).iter().count(),
            2
        );
        let _ = logout.bytes().await.unwrap();
        let logout_call = tokio::time::timeout(Duration::from_secs(1), recorded_calls.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(logout_call.adapter, "ncm::logout");
        assert_cookie_pairs(
            logout_call.cookie.as_deref(),
            &["MUSIC_U=refreshed-session", "__csrf=refreshed-csrf"],
        );

        let jar_url = renderer_origin.parse().unwrap();
        assert!(jar.cookies(&jar_url).is_none());
        let logged_out_account = client
            .get(format!("{renderer_origin}/api/user/account"))
            .header(header::ORIGIN, &renderer_origin)
            .send()
            .await
            .unwrap();
        assert_eq!(logged_out_account.status(), StatusCode::OK);
        let _ = logged_out_account.bytes().await.unwrap();
        let logged_out_account_call = recorded_calls.recv().await.unwrap();
        assert_eq!(logged_out_account_call.adapter, "ncm::user_account");
        assert!(logged_out_account_call.cookie.is_none());

        renderer_task.abort();
        api_task.abort();
        let _ = renderer_task.await;
        let _ = api_task.await;
    }

    #[tokio::test]
    async fn qr_login_exposes_the_rotated_cookie_in_headers_and_json() {
        let response = ApiResponse {
            status: 200,
            body: json!({
                "code": 200,
                "account": { "profile": { "avatarImgId_str": "42" } }
            }),
            cookie: vec!["MUSIC_U=qr-session; Path=/".to_owned()],
        };
        let (app, mut calls) = recording_router(Ok(response), false);
        let request = Request::builder()
            .uri("/login/qr/check?key=fixture-key")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::SET_COOKIE],
            "MUSIC_U=qr-session; Path=/"
        );
        let body = response_json(response).await;
        assert_eq!(body["cookie"], "MUSIC_U=qr-session; Path=/");
        assert_eq!(body["account"]["profile"]["avatarImgIdStr"], "42");
        let call = calls.recv().await.unwrap();
        assert_eq!(call.adapter, "ncm::login_qr_check");
        assert_eq!(call.params["key"], "fixture-key");
    }

    #[tokio::test]
    async fn named_routes_reject_domain_overrides_before_login_cookies_leave_loopback() {
        let (origin, mut captured, capture_task) = start_upstream_capture().await;
        let app = router(NcmState::production(Arc::new(ApiClient::new(None)))).unwrap();
        let query = serde_urlencoded::to_string([("domain", origin.as_str())]).unwrap();
        let request = Request::builder()
            .uri(format!("/user/account?{query}"))
            .header(header::COOKIE, "MUSIC_U=secret; __csrf=token")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        let leaked = tokio::time::timeout(Duration::from_millis(100), captured.recv())
            .await
            .ok()
            .flatten();

        assert!(
            leaked.is_none(),
            "caller-controlled domain received an authenticated request: {leaked:?}"
        );
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        capture_task.abort();
    }

    #[tokio::test]
    async fn qr_key_uses_the_node_compatible_data_envelope() {
        let response = ApiResponse {
            status: 200,
            body: json!({ "code": 200, "unikey": "fixture-key" }),
            cookie: Vec::new(),
        };
        let (app, mut calls) = recording_router(Ok(response), false);
        let request = Request::builder()
            .uri("/login/qr/key")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            json!({
                "code": 200,
                "data": { "code": 200, "unikey": "fixture-key" }
            })
        );
        assert_eq!(calls.recv().await.unwrap().adapter, "ncm::login_qr_key");
    }

    #[tokio::test]
    async fn playlist_tracks_wraps_direct_success_for_the_frontend_decoder() {
        let response = ApiResponse {
            status: 200,
            body: json!({ "code": 200 }),
            cookie: vec!["fixture=1".to_owned()],
        };
        let (app, mut calls) = recording_router(Ok(response), false);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/playlist/tracks?op=add&pid=9&tracks=1%2C2")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            json!({
                "status": 200,
                "body": { "code": 200 },
                "cookie": ["fixture=1"]
            })
        );
        assert_eq!(calls.recv().await.unwrap().adapter, "ncm::playlist_tracks");
    }

    #[tokio::test]
    async fn playlist_tracks_wraps_success_and_retries_code_512_once() {
        let responses = [
            Err(NcmError::Api {
                code: 512,
                msg: "retry with duplicate tracks".to_owned(),
            }),
            Ok(ApiResponse {
                status: 200,
                body: json!({ "code": 200 }),
                cookie: Vec::new(),
            }),
        ];
        let (app, mut calls) = recording_router_with_responses(responses, false);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/playlist/tracks?op=add&pid=9&tracks=1%2C2")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            json!({
                "status": 200,
                "body": { "code": 200 },
                "cookie": []
            })
        );

        let first = calls.try_recv().unwrap();
        let second = calls.try_recv().unwrap();
        assert_eq!(first.params["tracks"], "1,2");
        assert_eq!(second.params["tracks"], "1,2,1,2");
        assert!(calls.try_recv().is_err(), "playlist retry must be bounded");
    }

    #[tokio::test]
    async fn playlist_tracks_does_not_retry_code_512_more_than_once() {
        let error = || {
            Err(NcmError::Api {
                code: 512,
                msg: "still rejected".to_owned(),
            })
        };
        let responses = [
            error(),
            error(),
            Ok(ApiResponse {
                status: 200,
                body: json!({ "code": 200 }),
                cookie: Vec::new(),
            }),
        ];
        let (app, mut calls) = recording_router_with_responses(responses, false);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/playlist/tracks?op=add&pid=9&tracks=1%2C2")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status().as_u16(), 512);
        assert_eq!(calls.try_recv().unwrap().params["tracks"], "1,2");
        assert_eq!(calls.try_recv().unwrap().params["tracks"], "1,2,1,2");
        assert!(calls.try_recv().is_err(), "a second 512 must stop retrying");
    }

    #[tokio::test]
    async fn playlist_subscribe_sends_the_locked_node_anti_cheat_contract() {
        let (origin, mut captured, capture_task) = start_upstream_capture().await;
        let backend = ProductionBackend {
            client: Arc::new(ApiClient::new(None)),
        };

        for (t, expected_path, expects_body_token) in [
            ("1", "/eapi/playlist/subscribe", true),
            ("0", "/eapi/playlist/unsubscribe", false),
        ] {
            let mut query = Query::new();
            query.params.insert("t".to_owned(), t.to_owned());
            query.params.insert("id".to_owned(), "9".to_owned());
            query.domain = Some(origin.clone());
            backend
                .dispatch("ncm::playlist_subscribe", &query)
                .await
                .unwrap();

            let captured = tokio::time::timeout(Duration::from_secs(1), captured.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(captured.path, expected_path);
            let form: HashMap<String, String> =
                serde_urlencoded::from_bytes(&captured.body).unwrap();
            let (_, decrypted) = ncm_api_rs::crypto::eapi_req_decrypt(&form["params"]).unwrap();
            assert_eq!(
                decrypted["header"]["X-antiCheatToken"],
                ncm_api_rs::util::config::CHECK_TOKEN
            );
            assert_eq!(decrypted.get("checkToken").is_some(), expects_body_token);
            assert!(
                captured
                    .cookie
                    .as_deref()
                    .is_some_and(|cookie| cookie.contains("X-antiCheatToken=")),
                "EAPI cookie header is missing X-antiCheatToken"
            );
        }

        capture_task.abort();
    }

    #[tokio::test]
    async fn all_shared_audio_quality_values_reach_the_song_url_adapter() {
        let quality_cases: Vec<AudioQualityCase> =
            serde_json::from_str(include_str!("fixtures/audio-quality-cases.json")).unwrap();
        assert_eq!(quality_cases.len(), 5);

        for quality_case in quality_cases {
            let response = ApiResponse {
                status: 200,
                body: json!({ "code": 200, "data": [] }),
                cookie: Vec::new(),
            };
            let (app, mut calls) = recording_router(Ok(response), false);
            let request = Request::builder()
                .uri(format!("/song/url?id=42&br={}", quality_case.wire))
                .body(Body::empty())
                .unwrap();

            assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
            let call = calls.recv().await.unwrap();
            assert_eq!(call.adapter, "ncm::song_url");
            assert_eq!(
                call.params.get("br"),
                Some(&quality_case.wire),
                "setting {}",
                quality_case.setting
            );
        }
    }

    #[tokio::test]
    async fn generic_api_rejects_every_target_outside_the_cloud_lyric_contract() {
        let response = ApiResponse {
            status: 200,
            body: json!({ "lrc": "fixture" }),
            cookie: Vec::new(),
        };
        let (app, mut calls) = recording_router(Ok(response), false);
        let denied = Request::builder()
            .uri("/api?uri=%2Fapi%2Fuser%2Faccount&data=%7B%7D&crypto=eapi")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(denied).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert!(calls.try_recv().is_err());

        let wrong_crypto = Request::builder()
            .uri("/api?uri=%2Fapi%2Fcloud%2Flyric%2Fget&data=%7B%7D&crypto=weapi")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(wrong_crypto).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert!(calls.try_recv().is_err());

        let alternate_domain = Request::builder()
            .uri("/api?uri=%2Fapi%2Fcloud%2Flyric%2Fget&data=%7B%7D&crypto=eapi&domain=https%3A%2F%2Fattacker.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone()
                .oneshot(alternate_domain)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(calls.try_recv().is_err());

        let allowed = Request::builder()
            .uri("/api?uri=%2Fapi%2Fcloud%2Flyric%2Fget&data=%7B%22songId%22%3A7%7D&crypto=eapi")
            .body(Body::empty())
            .unwrap();
        assert_eq!(app.oneshot(allowed).await.unwrap().status(), StatusCode::OK);
        let call = calls.recv().await.unwrap();
        assert_eq!(call.adapter, "ncm::api");
        assert_eq!(call.params["data"], r#"{"songId":7}"#);
    }

    #[tokio::test]
    async fn cloud_accepts_large_multipart_without_relaxing_generic_body_limits() {
        let response = ApiResponse {
            status: 200,
            body: json!({ "code": 200 }),
            cookie: Vec::new(),
        };
        let (app, mut calls) = recording_router(Ok(response), false);
        let oversized_generic = Request::builder()
            .method(Method::POST)
            .uri("/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(vec![b' '; MAX_REQUEST_BODY_BYTES + 1]))
            .unwrap();
        assert_eq!(
            app.clone()
                .oneshot(oversized_generic)
                .await
                .unwrap()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert!(calls.try_recv().is_err());

        let boundary = "yesplaymusic-large-upload";
        let multipart = multipart_song_body(2 * 1024 * 1024 + 1, boundary);
        let cloud = Request::builder()
            .method(Method::POST)
            .uri("/cloud?proxy=%00")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(multipart))
            .unwrap();
        assert_eq!(
            app.oneshot(cloud).await.unwrap().status(),
            StatusCode::BAD_GATEWAY
        );
    }

    #[tokio::test]
    async fn native_logout_expires_the_local_session_before_remote_logout_finishes() {
        let response = ApiResponse {
            status: 200,
            body: json!({ "code": 200 }),
            cookie: Vec::new(),
        };
        let (app, mut calls) = recording_router(Ok(response), true);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/native/logout-session")
            .header(header::COOKIE, "MUSIC_U=secret; __csrf=token")
            .body(Body::empty())
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(1), app.oneshot(request))
            .await
            .expect("local logout must not wait for the remote service")
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let expiry_cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(expiry_cookies.len(), 2);
        assert!(expiry_cookies
            .iter()
            .any(|cookie| cookie.starts_with("MUSIC_U=;")));
        assert!(expiry_cookies
            .iter()
            .any(|cookie| cookie.starts_with("__csrf=;")));

        let call = tokio::time::timeout(Duration::from_secs(1), calls.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(call.adapter, "ncm::logout");
        assert_eq!(call.cookie.as_deref(), Some("MUSIC_U=secret; __csrf=token"));
    }

    #[tokio::test]
    async fn authentication_errors_keep_the_ncm_code_without_redirecting() {
        let (app, _) = recording_router(Err(NcmError::AuthRequired("expired".to_owned())), false);
        let request = Request::builder()
            .uri("/user/account")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().get(header::LOCATION).is_none());
        assert_eq!(
            response_json(response).await,
            json!({ "code": 301, "msg": "需要登录" })
        );
    }

    #[tokio::test]
    async fn transport_and_upstream_errors_have_stable_http_statuses() {
        let cases = [
            (
                NcmError::InvalidParam("missing id".to_owned()),
                StatusCode::BAD_REQUEST,
                400,
            ),
            (
                NcmError::Api {
                    code: 429,
                    msg: "too many requests".to_owned(),
                },
                StatusCode::TOO_MANY_REQUESTS,
                429,
            ),
            (
                NcmError::RateLimited("risk control".to_owned()),
                StatusCode::SERVICE_UNAVAILABLE,
                503,
            ),
            (
                NcmError::Timeout("deadline".to_owned()),
                StatusCode::GATEWAY_TIMEOUT,
                504,
            ),
            (
                NcmError::Api {
                    code: 418,
                    msg: "upstream rejected the request".to_owned(),
                },
                StatusCode::IM_A_TEAPOT,
                418,
            ),
        ];

        for (error, expected_status, expected_code) in cases {
            let (app, _) = recording_router(Err(error), false);
            let request = Request::builder()
                .uri("/user/account")
                .body(Body::empty())
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected_status);
            assert_eq!(response_json(response).await["code"], json!(expected_code));
        }
    }

    #[tokio::test]
    async fn stalled_upstream_requests_end_at_the_configured_deadline() {
        let app = router(NcmState::testing_with_timeout(
            Arc::new(PendingBackend),
            Duration::from_millis(10),
        ))
        .unwrap();
        let request = Request::builder()
            .uri("/user/account")
            .body(Body::empty())
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(1), app.oneshot(request))
            .await
            .expect("the injected NCM deadline must finish the request")
            .unwrap();

        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            response_json(response).await,
            json!({ "code": 504, "msg": "upstream NCM request timed out" })
        );
    }
}
