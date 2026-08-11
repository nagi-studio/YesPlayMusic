use std::{io, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderName, HeaderValue, Response, StatusCode},
    routing::get,
    Router,
};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

pub const HEALTH_PATH: &str = "/__yesplaymusic/health";
pub const HEALTH_BODY: &str = r#"{"service":"yesplaymusic-sidecar","protocol":1}"#;
pub const HEALTH_TOKEN_HEADER: &str = "X-YesPlayMusic-Health-Token";
pub const BACKEND_HEADER: &str = "X-YesPlayMusic-Backend";
pub const PARENT_SHUTDOWN_SIGNAL: u8 = 0;

#[derive(Clone)]
pub struct HealthState {
    token: Arc<str>,
}

impl HealthState {
    pub fn new(token: String) -> Result<Self, &'static str> {
        validate_health_token(&token)?;
        Ok(Self {
            token: token.into(),
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

pub fn validate_health_token(token: &str) -> Result<(), &'static str> {
    if token.len() == 64
        && token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err("sidecar startup token must contain 64 lowercase hexadecimal characters")
    }
}

pub fn secrets_match(received: &str, expected: &str) -> bool {
    received.len() == expected.len() && received.as_bytes().ct_eq(expected.as_bytes()).into()
}

pub async fn read_health_token<R>(reader: &mut R) -> io::Result<String>
where
    R: AsyncBufRead + Unpin,
{
    let mut token = String::new();
    let bytes = reader.read_line(&mut token).await?;
    if bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "parent did not provide a sidecar startup token",
        ));
    }
    let token = token.trim_end_matches(['\r', '\n']).to_owned();
    validate_health_token(&token)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
    Ok(token)
}

async fn health(State(state): State<HealthState>) -> Response<Body> {
    let mut response = Response::new(Body::from(HEALTH_BODY));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        HeaderName::from_static("x-yesplaymusic-health-token"),
        HeaderValue::from_str(state.token()).expect("validated token is a header value"),
    );
    headers.insert(
        HeaderName::from_static("x-yesplaymusic-backend"),
        HeaderValue::from_static("rust"),
    );
    response
}

pub fn router(state: HealthState) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(health))
        .with_state(state)
}

pub async fn cancel_on_parent_input<R>(mut input: R, shutdown: CancellationToken)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 1];
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            result = input.read(&mut buffer) => match result {
                Ok(0) | Err(_) => {
                    shutdown.cancel();
                    return;
                }
                Ok(_) if buffer[0] == PARENT_SHUTDOWN_SIGNAL => {
                    shutdown.cancel();
                    return;
                }
                Ok(_) => {}
            }
        }
    }
}

pub async fn cancel_when_parent_exits(parent_pid: u32, shutdown: CancellationToken) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = interval.tick() => {
                if !process_is_alive(parent_pid) {
                    shutdown.cancel();
                    return;
                }
            }
        }
    }
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    // SAFETY: signal 0 performs an existence/permission check and does not signal the process.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };

    // SAFETY: the handle is checked for null and closed exactly once below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut exit_code = 0_u32;
    // SAFETY: exit_code is writable and handle remains valid until CloseHandle.
    let alive =
        unsafe { GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code == STILL_ACTIVE };
    // SAFETY: handle was returned by OpenProcess and is no longer used.
    unsafe { CloseHandle(handle) };
    alive
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncWriteExt, BufReader};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_identity_matches_the_supervisor_protocol() {
        let token = "a".repeat(64);
        let response = router(HealthState::new(token.clone()).unwrap())
            .oneshot(
                axum::http::Request::builder()
                    .uri(HEALTH_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[HEALTH_TOKEN_HEADER], token);
        assert_eq!(response.headers()[BACKEND_HEADER], "rust");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body, HEALTH_BODY);
    }

    #[tokio::test]
    async fn token_is_read_once_and_the_remaining_pipe_stays_available() {
        let input = format!("{}\npending", "b".repeat(64));
        let mut reader = BufReader::new(input.as_bytes());
        assert_eq!(
            read_health_token(&mut reader).await.unwrap(),
            "b".repeat(64)
        );
        let mut remaining = String::new();
        reader.read_to_string(&mut remaining).await.unwrap();
        assert_eq!(remaining, "pending");
    }

    #[tokio::test]
    async fn parent_shutdown_signal_and_eof_both_cancel_the_server() {
        for explicit_signal in [true, false] {
            let (mut writer, reader) = tokio::io::duplex(8);
            let shutdown = CancellationToken::new();
            let monitor = tokio::spawn(cancel_on_parent_input(reader, shutdown.clone()));
            if explicit_signal {
                writer.write_all(&[PARENT_SHUTDOWN_SIGNAL]).await.unwrap();
            } else {
                drop(writer);
            }
            tokio::time::timeout(Duration::from_secs(1), shutdown.cancelled())
                .await
                .unwrap();
            monitor.await.unwrap();
        }
    }
}
