use std::{path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    extract::State,
    http::{
        header::{self, HeaderName},
        HeaderValue, Request, Response, StatusCode, Uri,
    },
    middleware::Next,
    response::IntoResponse,
};
use futures_util::TryStreamExt;
use reqwest::redirect::Policy;
use serde_json::json;
use tower::{ServiceBuilder, ServiceExt};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};

use crate::session::{harden_auth_cookie, NATIVE_AUTH_HEADER};

pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; base-uri 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: http: https:; media-src 'self' data: blob: http: https:; font-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost http: https:; worker-src 'self' blob:; object-src 'none'; frame-ancestors 'none'; form-action 'self'";

#[derive(Clone)]
pub struct ApiProxy {
    client: reqwest::Client,
    upstream: Arc<str>,
    native_token: Arc<str>,
}

impl ApiProxy {
    pub fn new(api_port: u16, native_token: String) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(Policy::none())
                .build()?,
            upstream: format!("http://127.0.0.1:{api_port}").into(),
            native_token: native_token.into(),
        })
    }
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn proxy_error(message: &str) -> Response<Body> {
    (
        StatusCode::BAD_GATEWAY,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        json!({ "message": message }).to_string(),
    )
        .into_response()
}

pub async fn proxy_api(State(proxy): State<ApiProxy>, request: Request<Body>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/api");
    let stripped = path_and_query
        .strip_prefix("/api")
        .filter(|value| value.is_empty() || value.starts_with('/') || value.starts_with('?'))
        .unwrap_or(path_and_query);
    let upstream_path = if stripped.is_empty() { "/" } else { stripped };
    let url = format!("{}{upstream_path}", proxy.upstream);

    let mut builder = proxy.client.request(parts.method, url);
    for (name, value) in &parts.headers {
        if name != header::HOST && name.as_str() != NATIVE_AUTH_HEADER && !is_hop_by_hop(name) {
            builder = builder.header(name, value);
        }
    }
    if parts.uri.path().starts_with("/api/native/") {
        builder = builder.header(NATIVE_AUTH_HEADER, proxy.native_token.as_ref());
    }
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let upstream = match builder
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!(path = %parts.uri.path(), "local API proxy failed");
            return proxy_error("local API is unavailable");
        }
    };

    let status = upstream.status();
    let upstream_headers = upstream.headers().clone();
    let body = Body::from_stream(upstream.bytes_stream().map_err(std::io::Error::other));
    let mut response = Response::new(body);
    *response.status_mut() = status;
    for (name, value) in &upstream_headers {
        if name != header::SET_COOKIE && !is_hop_by_hop(name) {
            response.headers_mut().append(name, value.clone());
        }
    }
    for value in upstream_headers.get_all(header::SET_COOKIE) {
        if let Ok(cookie) = value.to_str() {
            if let Ok(value) = HeaderValue::from_str(&harden_auth_cookie(cookie)) {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
        }
    }
    response
}

pub async fn apply_security_headers(request: Request<Body>, next: Next) -> Response<Body> {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

pub async fn serve_renderer(renderer_dir: PathBuf, request: Request<Body>) -> Response<Body> {
    let service = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=0"),
        ))
        .service(ServeDir::new(renderer_dir).append_index_html_on_directories(true));
    match service.oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(error) => {
            tracing::warn!("renderer service failed: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub fn append_query(uri: &Uri, path: &str) -> String {
    match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::{
        body::to_bytes,
        extract::State,
        http::{HeaderMap, Method},
        routing::any,
        Router,
    };
    use tokio::{io::AsyncWriteExt, net::TcpListener, process::Command};

    use super::*;

    #[derive(Clone)]
    struct CaptureState(Arc<tokio::sync::Mutex<Option<String>>>);

    async fn upstream(State(state): State<CaptureState>, headers: HeaderMap) -> impl IntoResponse {
        *state.0.lock().await = headers
            .get(NATIVE_AUTH_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let mut response = Response::new(Body::from("{}"));
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_static("MUSIC_U=one; Path=/"),
        );
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_static("__csrf=two; Path=/; SameSite=Lax"),
        );
        response
    }

    #[tokio::test]
    async fn same_origin_proxy_injects_native_token_and_preserves_cookie_lines() {
        let captured = CaptureState(Arc::new(tokio::sync::Mutex::new(None)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new()
            .route("/native/action", any(upstream))
            .with_state(captured.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let proxy = ApiProxy::new(port, "native-secret".to_owned()).unwrap();
        let response = proxy_api(
            State(proxy),
            Request::builder()
                .method(Method::POST)
                .uri("/api/native/action")
                .body(Body::from("payload"))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(cookies.len(), 2);
        assert!(cookies.iter().all(|cookie| cookie.contains("HttpOnly")));
        assert!(cookies
            .iter()
            .all(|cookie| cookie.contains("SameSite=Strict")));
        assert_eq!(captured.0.lock().await.as_deref(), Some("native-secret"));
        assert_eq!(to_bytes(response.into_body(), 32).await.unwrap(), "{}");
        task.abort();
    }

    #[tokio::test]
    async fn api_proxy_ignores_environment_proxies() {
        const CHILD_MARKER: &str = "YPM_RENDERER_NO_PROXY_TEST_CHILD";
        const TEST_NAME: &str = "renderer::tests::api_proxy_ignores_environment_proxies";

        if std::env::var_os(CHILD_MARKER).is_none() {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy_url = format!("http://{}", listener.local_addr().unwrap());
            let proxy_hit = Arc::new(AtomicBool::new(false));
            let child_proxy_hit = proxy_hit.clone();
            let poison_proxy = tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    child_proxy_hit.store(true, Ordering::Release);
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 418 I'm a teapot\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                }
            });

            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
                .env(CHILD_MARKER, "1")
                .env("HTTP_PROXY", &proxy_url)
                .env("HTTPS_PROXY", &proxy_url)
                .env("ALL_PROXY", &proxy_url)
                .env("NO_PROXY", "")
                .env("http_proxy", &proxy_url)
                .env("https_proxy", &proxy_url)
                .env("all_proxy", &proxy_url)
                .env("no_proxy", "")
                .output()
                .await
                .unwrap();
            poison_proxy.abort();

            assert!(
                output.status.success(),
                "isolated proxy test failed:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!proxy_hit.load(Ordering::Acquire));
            return;
        }

        let captured = CaptureState(Arc::new(tokio::sync::Mutex::new(None)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new()
            .route("/native/action", any(upstream))
            .with_state(captured.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let proxy = ApiProxy::new(port, "native-secret".to_owned()).unwrap();
        let response = proxy_api(
            State(proxy),
            Request::builder()
                .method(Method::POST)
                .uri("/api/native/action")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(captured.0.lock().await.as_deref(), Some("native-secret"));
        task.abort();
    }
}
