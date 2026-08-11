use std::{collections::HashSet, sync::Arc};

use axum::{
    body::Body,
    http::{
        header::{self, HeaderName},
        HeaderValue, Method, Request, Response, StatusCode,
    },
    middleware::Next,
};
use serde_json::json;

use crate::health::secrets_match;

pub const NATIVE_AUTH_HEADER: &str = "x-yesplaymusic-native-token";

#[derive(Clone)]
pub struct RequestBoundary {
    allowed_origins: Arc<HashSet<String>>,
    native_token: Option<Arc<str>>,
}

impl RequestBoundary {
    pub fn new(
        allowed_origins: impl IntoIterator<Item = String>,
        native_token: Option<String>,
    ) -> Self {
        Self {
            allowed_origins: Arc::new(allowed_origins.into_iter().collect()),
            native_token: native_token.map(Into::into),
        }
    }
}

fn json_error(status: StatusCode, message: &'static str) -> Response<Body> {
    let mut response = Response::new(Body::from(json!({ "message": message }).to_string()));
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

fn append_vary_origin(response: &mut Response<Body>) {
    let contains_origin = response
        .headers()
        .get_all(header::VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim().eq_ignore_ascii_case("origin"));
    if !contains_origin {
        response
            .headers_mut()
            .append(header::VARY, HeaderValue::from_static("Origin"));
    }
}

fn is_renderer_entry_navigation(boundary: &RequestBoundary, request: &Request<Body>) -> bool {
    let safe_method = matches!(*request.method(), Method::GET | Method::HEAD);
    let root_path = request.uri().path() == "/";
    let top_level_navigation = request
        .headers()
        .get(HeaderName::from_static("sec-fetch-mode"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("navigate"))
        && request
            .headers()
            .get(HeaderName::from_static("sec-fetch-dest"))
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("document"));
    let accepts_html = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|item| {
                item.split(';')
                    .next()
                    .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/html"))
            })
        });

    boundary.native_token.is_none()
        && safe_method
        && root_path
        && top_level_navigation
        && accepts_html
}

pub async fn enforce_local_boundary(
    axum::extract::State(boundary): axum::extract::State<RequestBoundary>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let cross_site = request
        .headers()
        .get(HeaderName::from_static("sec-fetch-site"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"));
    let permitted_navigation = cross_site && is_renderer_entry_navigation(&boundary, &request);
    if (cross_site && !permitted_navigation)
        || origin
            .as_ref()
            .is_some_and(|value| !boundary.allowed_origins.contains(value))
    {
        return json_error(
            StatusCode::FORBIDDEN,
            "cross-site access to the local service is denied",
        );
    }

    if let Some(expected) = boundary.native_token.as_deref() {
        if request.uri().path().starts_with("/native/") {
            let received = request
                .headers()
                .get(NATIVE_AUTH_HEADER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if !secrets_match(received, expected) {
                return json_error(StatusCode::UNAUTHORIZED, "native API authentication failed");
            }
        }
    }

    let is_preflight = request.method() == Method::OPTIONS;
    let mut response = if is_preflight {
        Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .expect("valid preflight response")
    } else {
        next.run(request).await
    };

    if let Some(origin) = origin {
        if let Ok(value) = HeaderValue::from_str(&origin) {
            response
                .headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("X-Requested-With, Content-Type"),
            );
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("PUT, POST, GET, DELETE, OPTIONS"),
            );
            append_vary_origin(&mut response);
        }
    }
    response
}

fn cookie_name(cookie: &str) -> &str {
    cookie
        .split_once('=')
        .map(|(name, _)| name.trim())
        .unwrap_or_default()
}

pub fn harden_auth_cookie(cookie: &str) -> String {
    if !matches!(cookie_name(cookie), "MUSIC_U" | "__csrf") {
        return cookie.to_owned();
    }
    let mut parts = cookie
        .split(';')
        .map(str::trim)
        .filter(|part| {
            !part.is_empty()
                && !part.eq_ignore_ascii_case("httponly")
                && !part.to_ascii_lowercase().starts_with("samesite=")
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    parts.push("HttpOnly".to_owned());
    parts.push("SameSite=Strict".to_owned());
    parts.join("; ")
}

pub fn desktop_session_expiry_cookies() -> [String; 2] {
    let attributes =
        "Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; HttpOnly; SameSite=Strict";
    [
        format!("MUSIC_U=; {attributes}"),
        format!("__csrf=; {attributes}"),
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::{middleware, routing::any, routing::get, Router};
    use tower::ServiceExt;

    use super::*;

    fn protected_router(counter: Arc<AtomicUsize>, native_token: Option<String>) -> Router {
        let boundary = RequestBoundary::new(["http://127.0.0.1:28232".to_owned()], native_token);
        Router::new()
            .route(
                "/native/action",
                get(move || {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::Relaxed);
                        StatusCode::NO_CONTENT
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(
                boundary,
                enforce_local_boundary,
            ))
    }

    #[tokio::test]
    async fn unsafe_origins_and_missing_native_tokens_never_reach_handlers() {
        let counter = Arc::new(AtomicUsize::new(0));
        let app = protected_router(counter.clone(), Some("a".repeat(64)));
        let cross_site = Request::builder()
            .uri("/native/action?secret=value")
            .header(header::ORIGIN, "https://attacker.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(cross_site).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let missing_token = Request::builder()
            .uri("/native/action")
            .header(header::ORIGIN, "http://127.0.0.1:28232")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(missing_token).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(counter.load(Ordering::Relaxed), 0);

        let authorized = Request::builder()
            .uri("/native/action")
            .header(header::ORIGIN, "http://127.0.0.1:28232")
            .header(NATIVE_AUTH_HEADER, "a".repeat(64))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(authorized).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    fn cross_site_navigation(path: &str, method: Method) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header("sec-fetch-site", "cross-site")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document")
            .header(header::ACCEPT, "text/html,application/xhtml+xml;q=0.9")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn cross_site_top_level_renderer_navigation_is_the_only_exception() {
        let counter = Arc::new(AtomicUsize::new(0));
        let handler_counter = counter.clone();
        let boundary = RequestBoundary::new(["http://127.0.0.1:28232".to_owned()], None);
        let app = Router::new()
            .route(
                "/",
                any(move || {
                    let counter = handler_counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::Relaxed);
                        StatusCode::NO_CONTENT
                    }
                }),
            )
            .route(
                "/api/login/status",
                any(|| async { StatusCode::NO_CONTENT }),
            )
            .route("/native/action", any(|| async { StatusCode::NO_CONTENT }))
            .layer(middleware::from_fn_with_state(
                boundary,
                enforce_local_boundary,
            ));

        assert_eq!(
            app.clone()
                .oneshot(cross_site_navigation("/?token=oauth", Method::GET))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(counter.load(Ordering::Relaxed), 1);

        let rejected = [
            cross_site_navigation("/", Method::POST),
            cross_site_navigation("/api/login/status", Method::GET),
            cross_site_navigation("/native/action", Method::GET),
            Request::builder()
                .uri("/")
                .header("sec-fetch-site", "cross-site")
                .header("sec-fetch-mode", "cors")
                .header("sec-fetch-dest", "empty")
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .uri("/")
                .header(header::ORIGIN, "https://www.last.fm")
                .header("sec-fetch-site", "cross-site")
                .header("sec-fetch-mode", "navigate")
                .header("sec-fetch-dest", "document")
                .header(header::ACCEPT, "text/html")
                .body(Body::empty())
                .unwrap(),
        ];
        for request in rejected {
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn auth_cookies_are_hardened_without_collapsing_other_cookies() {
        assert_eq!(
            harden_auth_cookie("MUSIC_U=value; Path=/; SameSite=Lax"),
            "MUSIC_U=value; Path=/; HttpOnly; SameSite=Strict"
        );
        assert_eq!(
            harden_auth_cookie("NMTID=value; Path=/"),
            "NMTID=value; Path=/"
        );
        assert_eq!(desktop_session_expiry_cookies().len(), 2);
    }
}
