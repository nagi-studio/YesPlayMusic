use std::{fmt, io, sync::Arc, time::Duration};

use rustls::{
    pki_types::{pem::PemObject, CertificateDer, ServerName},
    ClientConfig, RootCertStore,
};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{watch, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::{sleep_until, timeout, Instant},
};
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;
use url::Url;

pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_CONNECTIONS: usize = 128;
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_IO_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
pub const DEFAULT_LOOPBACK_IDLE_TIMEOUT: Duration = Duration::from_secs(380);
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyRelayLimits {
    pub max_connections: usize,
    pub connect_timeout: Duration,
    pub header_timeout: Duration,
    pub io_idle_timeout: Duration,
    pub loopback_idle_timeout: Duration,
}

impl Default for ProxyRelayLimits {
    fn default() -> Self {
        Self {
            max_connections: DEFAULT_MAX_CONNECTIONS,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            header_timeout: DEFAULT_HEADER_TIMEOUT,
            io_idle_timeout: DEFAULT_IO_IDLE_TIMEOUT,
            loopback_idle_timeout: DEFAULT_LOOPBACK_IDLE_TIMEOUT,
        }
    }
}

impl ProxyRelayLimits {
    fn validate(&self) -> Result<(), ProxyRelayError> {
        if self.max_connections == 0
            || self.connect_timeout.is_zero()
            || self.header_timeout.is_zero()
            || self.io_idle_timeout.is_zero()
            || self.loopback_idle_timeout.is_zero()
        {
            return Err(ProxyRelayError::InvalidLimits);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProxyScheme {
    Http,
    Https,
}

#[derive(Clone)]
struct ProxyEndpoint {
    scheme: ProxyScheme,
    host: String,
    port: u16,
    tls_config: Option<Arc<ClientConfig>>,
}

/// Routing for non-loopback proxy targets.
///
/// `Default` and [`UpstreamProxy::direct`] connect to the target without an
/// upstream proxy. A parsed HTTP(S) URL sends external targets through that
/// proxy. Loopback targets always bypass this route.
#[derive(Clone, Default)]
pub struct UpstreamProxy {
    endpoint: Option<ProxyEndpoint>,
}

impl fmt::Debug for UpstreamProxy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.endpoint {
            Some(endpoint) => formatter
                .debug_struct("UpstreamProxy")
                .field("scheme", &endpoint.scheme)
                .field("host", &endpoint.host)
                .field("port", &endpoint.port)
                .finish_non_exhaustive(),
            None => formatter.write_str("UpstreamProxy::Direct"),
        }
    }
}

impl UpstreamProxy {
    pub fn direct() -> Self {
        Self::default()
    }

    pub fn parse(value: &str) -> Result<Self, ProxyRelayError> {
        if value.trim() != value {
            return Err(ProxyRelayError::InvalidUpstreamProxy);
        }

        let url = Url::parse(value).map_err(|_| ProxyRelayError::InvalidUpstreamProxy)?;
        let scheme = match url.scheme() {
            "http" => ProxyScheme::Http,
            "https" => ProxyScheme::Https,
            _ => return Err(ProxyRelayError::InvalidUpstreamProxy),
        };
        if url.host_str().is_none()
            || has_explicit_url_userinfo(value)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port() == Some(0)
        {
            return Err(ProxyRelayError::InvalidUpstreamProxy);
        }

        let port = url
            .port_or_known_default()
            .filter(|port| *port != 0)
            .ok_or(ProxyRelayError::InvalidUpstreamProxy)?;
        let tls_config = match scheme {
            ProxyScheme::Http => None,
            ProxyScheme::Https => Some(build_tls_config(None)?),
        };

        Ok(Self {
            endpoint: Some(ProxyEndpoint {
                scheme,
                host: unbracket_host(url.host_str().expect("host was checked")),
                port,
                tls_config,
            }),
        })
    }

    /// Replaces the platform roots for an HTTPS upstream with the supplied PEM
    /// certificate bundle. This is primarily useful for enterprise proxies and
    /// hermetic tests.
    pub fn with_tls_ca_pem(mut self, pem: &[u8]) -> Result<Self, ProxyRelayError> {
        let endpoint = self
            .endpoint
            .as_mut()
            .filter(|endpoint| endpoint.scheme == ProxyScheme::Https)
            .ok_or(ProxyRelayError::TlsCaRequiresHttps)?;
        let certificates = CertificateDer::pem_slice_iter(pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ProxyRelayError::InvalidTlsCa)?;
        if certificates.is_empty() {
            return Err(ProxyRelayError::InvalidTlsCa);
        }
        endpoint.tls_config = Some(build_tls_config(Some(certificates))?);
        Ok(self)
    }

    pub fn is_direct(&self) -> bool {
        self.endpoint.is_none()
    }
}

#[derive(Debug, Error)]
pub enum ProxyRelayError {
    #[error("upstream proxy must contain only an HTTP(S) host and port")]
    InvalidUpstreamProxy,
    #[error("upstream TLS CA must contain at least one valid PEM certificate")]
    InvalidTlsCa,
    #[error("upstream TLS CA can only be configured for an HTTPS proxy")]
    TlsCaRequiresHttps,
    #[error("proxy relay limits must be positive")]
    InvalidLimits,
}

#[derive(Debug)]
enum ConnectionError {
    Cancelled,
    Io(io::Error),
}

impl From<io::Error> for ConnectionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

trait RelayStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> RelayStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

type BoxedRelayStream = Box<dyn RelayStream>;

#[derive(Debug)]
struct RequestHead {
    method: String,
    target: String,
    version: String,
    headers: Vec<HeaderLine>,
    host: Option<String>,
}

#[derive(Debug)]
struct HeaderLine {
    name: Vec<u8>,
    raw: Vec<u8>,
}

#[derive(Debug)]
struct Target {
    host: String,
    port: u16,
}

#[derive(Debug)]
struct HttpRoute {
    target: Target,
    direct_request_target: String,
    upstream_request_target: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeaderReadError {
    Closed,
    Incomplete,
    TooLarge,
}

/// Serves a forward proxy on a caller-bound listener until `shutdown` is
/// cancelled. Existing sockets are cancelled and awaited before this returns.
pub async fn serve(
    listener: TcpListener,
    upstream: UpstreamProxy,
    shutdown: CancellationToken,
) -> Result<(), ProxyRelayError> {
    serve_with_limits(listener, upstream, shutdown, ProxyRelayLimits::default()).await
}

pub async fn serve_with_limits(
    listener: TcpListener,
    upstream: UpstreamProxy,
    shutdown: CancellationToken,
    limits: ProxyRelayLimits,
) -> Result<(), ProxyRelayError> {
    limits.validate()?;
    let semaphore = Arc::new(Semaphore::new(limits.max_connections));
    let mut connections = JoinSet::new();

    loop {
        while let Some(result) = connections.try_join_next() {
            if let Err(error) = result {
                tracing::warn!(
                    cancelled = error.is_cancelled(),
                    panic = error.is_panic(),
                    "proxy relay connection task failed"
                );
            }
        }

        let accepted = tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let (mut stream, _) = match accepted {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(error_kind = ?error.kind(), "proxy relay accept failed; retrying");
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(ACCEPT_RETRY_DELAY) => continue,
                }
            }
        };
        let permit = match semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let _ = write_error(&mut stream, 503, "Service Unavailable").await;
                continue;
            }
        };
        let upstream = upstream.clone();
        let limits = limits.clone();
        let connection_shutdown = shutdown.clone();
        connections.spawn(async move {
            run_connection(stream, upstream, limits, connection_shutdown, permit).await;
        });
    }

    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            tracing::warn!(
                cancelled = error.is_cancelled(),
                panic = error.is_panic(),
                "proxy relay connection task failed during shutdown"
            );
        }
    }
    Ok(())
}

async fn run_connection(
    mut client: TcpStream,
    upstream: UpstreamProxy,
    limits: ProxyRelayLimits,
    shutdown: CancellationToken,
    _permit: OwnedSemaphorePermit,
) {
    let _ = client.set_nodelay(true);
    let result = tokio::select! {
        biased;
        _ = shutdown.cancelled() => Err(ConnectionError::Cancelled),
        result = handle_connection(&mut client, &upstream, &limits) => result,
    };
    if let Err(ConnectionError::Io(error)) = result {
        let _ = error.kind();
    }
}

async fn handle_connection(
    client: &mut TcpStream,
    upstream: &UpstreamProxy,
    limits: &ProxyRelayLimits,
) -> Result<(), ConnectionError> {
    let mut initial = Vec::with_capacity(4096);
    let header_end = match timeout(limits.header_timeout, read_header(client, &mut initial)).await {
        Err(_) => {
            write_error(client, 408, "Request Timeout").await?;
            return Ok(());
        }
        Ok(Err(HeaderReadError::Closed)) => return Ok(()),
        Ok(Err(HeaderReadError::Incomplete)) => {
            write_error(client, 400, "Bad Request").await?;
            return Ok(());
        }
        Ok(Err(HeaderReadError::TooLarge)) => {
            write_error(client, 431, "Request Header Fields Too Large").await?;
            return Ok(());
        }
        Ok(Ok(header_end)) => header_end,
    };

    let request = match parse_request_head(&initial[..header_end]) {
        Ok(request) => request,
        Err(()) => {
            write_error(client, 400, "Bad Request").await?;
            return Ok(());
        }
    };
    let buffered_body = &initial[header_end..];

    if request.method == "CONNECT" {
        handle_connect(client, &request, buffered_body, upstream, limits).await
    } else {
        handle_http(client, &request, buffered_body, upstream, limits).await
    }
}

async fn copy_with_activity<R, W>(
    mut reader: R,
    mut writer: W,
    activity: watch::Sender<Instant>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            let _ = writer.shutdown().await;
            return Ok(());
        }
        writer.write_all(&buffer[..read]).await?;
        activity.send_replace(Instant::now());
    }
}

async fn relay_with_idle_timeout<A, B>(
    client: &mut A,
    server: &mut B,
    idle_timeout: Duration,
) -> io::Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    let (client_reader, client_writer) = tokio::io::split(client);
    let (server_reader, server_writer) = tokio::io::split(server);
    let (activity_tx, mut activity_rx) = watch::channel(Instant::now());
    let client_to_server = copy_with_activity(client_reader, server_writer, activity_tx.clone());
    let server_to_client = copy_with_activity(server_reader, client_writer, activity_tx);
    tokio::pin!(client_to_server, server_to_client);
    let mut client_done = false;
    let mut server_done = false;

    loop {
        if client_done && server_done {
            return Ok(());
        }
        let deadline = *activity_rx.borrow() + idle_timeout;
        let idle = sleep_until(deadline);
        tokio::pin!(idle);
        tokio::select! {
            result = &mut client_to_server, if !client_done => {
                result?;
                client_done = true;
            }
            result = &mut server_to_client, if !server_done => {
                result?;
                server_done = true;
            }
            changed = activity_rx.changed() => {
                if changed.is_err() && client_done && server_done {
                    return Ok(());
                }
            }
            _ = &mut idle => return Ok(()),
        }
    }
}

async fn handle_http(
    client: &mut TcpStream,
    request: &RequestHead,
    buffered_body: &[u8],
    upstream: &UpstreamProxy,
    limits: &ProxyRelayLimits,
) -> Result<(), ConnectionError> {
    let route = match parse_http_route(request) {
        Ok(route) => route,
        Err(()) => {
            write_error(client, 400, "Bad Request").await?;
            return Ok(());
        }
    };

    let loopback = is_loopback_proxy_host(&route.target.host);
    let (mut server, request_target) = if loopback || upstream.is_direct() {
        match connect_plain(&route.target, limits.connect_timeout).await {
            Ok(server) => (
                Box::new(server) as BoxedRelayStream,
                &route.direct_request_target,
            ),
            Err(ConnectError::Timeout) => {
                write_error(client, 504, "Gateway Timeout").await?;
                return Ok(());
            }
            Err(ConnectError::Failed) => {
                write_error(client, 502, "Bad Gateway").await?;
                return Ok(());
            }
        }
    } else {
        match connect_upstream(upstream, limits.connect_timeout).await {
            Ok(server) => (server, &route.upstream_request_target),
            Err(ConnectError::Timeout) => {
                write_error(client, 504, "Gateway Timeout").await?;
                return Ok(());
            }
            Err(ConnectError::Failed) => {
                write_error(client, 502, "Bad Gateway").await?;
                return Ok(());
            }
        }
    };

    let outgoing_head = build_outgoing_head(request, request_target);
    server.write_all(&outgoing_head).await?;
    if !buffered_body.is_empty() {
        server.write_all(buffered_body).await?;
    }

    let idle_timeout = if loopback {
        limits.loopback_idle_timeout
    } else {
        limits.io_idle_timeout
    };
    relay_with_idle_timeout(client, server.as_mut(), idle_timeout)
        .await
        .map_err(ConnectionError::Io)
}

async fn handle_connect(
    client: &mut TcpStream,
    request: &RequestHead,
    buffered_head: &[u8],
    upstream: &UpstreamProxy,
    limits: &ProxyRelayLimits,
) -> Result<(), ConnectionError> {
    let target = match parse_connect_target(&request.target) {
        Ok(target) => target,
        Err(()) => {
            write_error(client, 400, "Bad Request").await?;
            return Ok(());
        }
    };

    if is_loopback_proxy_host(&target.host) || upstream.is_direct() {
        let mut server = match connect_plain(&target, limits.connect_timeout).await {
            Ok(server) => server,
            Err(ConnectError::Timeout) => {
                write_error(client, 504, "Gateway Timeout").await?;
                return Ok(());
            }
            Err(ConnectError::Failed) => {
                write_error(client, 502, "Bad Gateway").await?;
                return Ok(());
            }
        };
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        if !buffered_head.is_empty() {
            server.write_all(buffered_head).await?;
        }
        let idle_timeout = if is_loopback_proxy_host(&target.host) {
            limits.loopback_idle_timeout
        } else {
            limits.io_idle_timeout
        };
        relay_with_idle_timeout(client, &mut server, idle_timeout).await?;
        return Ok(());
    }

    let mut server = match connect_upstream(upstream, limits.connect_timeout).await {
        Ok(server) => server,
        Err(ConnectError::Timeout) => {
            write_error(client, 504, "Gateway Timeout").await?;
            return Ok(());
        }
        Err(ConnectError::Failed) => {
            write_error(client, 502, "Bad Gateway").await?;
            return Ok(());
        }
    };
    let connect_request = format!(
        "CONNECT {} HTTP/1.1\r\nHost: {}\r\nProxy-Connection: keep-alive\r\n\r\n",
        request.target, request.target
    );
    server.write_all(connect_request.as_bytes()).await?;

    let mut response = Vec::with_capacity(1024);
    let response_end = match timeout(
        limits.header_timeout,
        read_header(&mut server, &mut response),
    )
    .await
    {
        Err(_) => {
            write_error(client, 504, "Gateway Timeout").await?;
            return Ok(());
        }
        Ok(Err(_)) => {
            write_error(client, 502, "Bad Gateway").await?;
            return Ok(());
        }
        Ok(Ok(response_end)) => response_end,
    };
    let status = parse_response_status(&response[..response_end]);
    client.write_all(&response[..response_end]).await?;
    if status != Some(200) {
        return Ok(());
    }
    if response.len() > response_end {
        client.write_all(&response[response_end..]).await?;
    }
    if !buffered_head.is_empty() {
        server.write_all(buffered_head).await?;
    }
    relay_with_idle_timeout(client, server.as_mut(), limits.io_idle_timeout).await?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectError {
    Timeout,
    Failed,
}

async fn connect_plain(target: &Target, deadline: Duration) -> Result<TcpStream, ConnectError> {
    match timeout(
        deadline,
        TcpStream::connect((target.host.as_str(), target.port)),
    )
    .await
    {
        Ok(Ok(stream)) => {
            let _ = stream.set_nodelay(true);
            Ok(stream)
        }
        Ok(Err(_)) => Err(ConnectError::Failed),
        Err(_) => Err(ConnectError::Timeout),
    }
}

async fn connect_upstream(
    upstream: &UpstreamProxy,
    deadline: Duration,
) -> Result<BoxedRelayStream, ConnectError> {
    let endpoint = upstream.endpoint.as_ref().ok_or(ConnectError::Failed)?;
    let connect = async {
        let stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
            .await
            .map_err(|_| ConnectError::Failed)?;
        let _ = stream.set_nodelay(true);
        match endpoint.scheme {
            ProxyScheme::Http => Ok(Box::new(stream) as BoxedRelayStream),
            ProxyScheme::Https => {
                let server_name = ServerName::try_from(endpoint.host.clone())
                    .map_err(|_| ConnectError::Failed)?;
                let config = endpoint.tls_config.as_ref().ok_or(ConnectError::Failed)?;
                let stream = TlsConnector::from(config.clone())
                    .connect(server_name, stream)
                    .await
                    .map_err(|_| ConnectError::Failed)?;
                Ok(Box::new(stream) as BoxedRelayStream)
            }
        }
    };

    match timeout(deadline, connect).await {
        Ok(result) => result,
        Err(_) => Err(ConnectError::Timeout),
    }
}

fn build_tls_config(
    custom_certificates: Option<Vec<CertificateDer<'static>>>,
) -> Result<Arc<ClientConfig>, ProxyRelayError> {
    let mut roots = RootCertStore::empty();
    if let Some(certificates) = custom_certificates {
        for certificate in certificates {
            roots
                .add(certificate)
                .map_err(|_| ProxyRelayError::InvalidTlsCa)?;
        }
    } else {
        let native = rustls_native_certs::load_native_certs();
        for certificate in native.certs {
            let _ = roots.add(certificate);
        }
    }
    if roots.is_empty() {
        return Err(ProxyRelayError::InvalidTlsCa);
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| ProxyRelayError::InvalidTlsCa)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

async fn read_header<S>(stream: &mut S, buffer: &mut Vec<u8>) -> Result<usize, HeaderReadError>
where
    S: AsyncRead + Unpin + ?Sized,
{
    loop {
        if let Some(boundary) = find_header_boundary(buffer) {
            let header_end = boundary + 4;
            return if header_end <= MAX_HEADER_BYTES {
                Ok(header_end)
            } else {
                Err(HeaderReadError::TooLarge)
            };
        }
        if buffer.len() >= MAX_HEADER_BYTES {
            return Err(HeaderReadError::TooLarge);
        }

        let mut chunk = [0_u8; 8192];
        let read_capacity = chunk.len().min(MAX_HEADER_BYTES + 1 - buffer.len());
        let read = stream
            .read(&mut chunk[..read_capacity])
            .await
            .map_err(|_| HeaderReadError::Incomplete)?;
        if read == 0 {
            return if buffer.is_empty() {
                Err(HeaderReadError::Closed)
            } else {
                Err(HeaderReadError::Incomplete)
            };
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn find_header_boundary(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_request_head(bytes: &[u8]) -> Result<RequestHead, ()> {
    let first_end = find_crlf(bytes).ok_or(())?;
    let request_line = std::str::from_utf8(&bytes[..first_end]).map_err(|_| ())?;
    let mut parts = request_line.split(' ');
    let method = parts
        .next()
        .filter(|part| is_http_token(part.as_bytes()))
        .ok_or(())?;
    let target = parts.next().filter(|part| !part.is_empty()).ok_or(())?;
    let version = parts.next().ok_or(())?;
    if parts.next().is_some()
        || !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || !target.is_ascii()
        || target.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(());
    }

    let mut headers = Vec::new();
    let mut host = None;
    let mut cursor = first_end + 2;
    while cursor + 2 <= bytes.len() {
        let relative_end = find_crlf(&bytes[cursor..]).ok_or(())?;
        if relative_end == 0 {
            break;
        }
        let line_end = cursor + relative_end;
        let line = &bytes[cursor..line_end];
        if line
            .first()
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
        {
            return Err(());
        }
        let colon = line.iter().position(|byte| *byte == b':').ok_or(())?;
        let name = &line[..colon];
        let value = &line[colon + 1..];
        if !is_http_token(name)
            || value
                .iter()
                .any(|byte| (*byte < 0x20 && *byte != b'\t') || *byte == 0x7f)
        {
            return Err(());
        }
        if name.eq_ignore_ascii_case(b"host") {
            if host.is_some() {
                return Err(());
            }
            let value = trim_ascii_whitespace(value);
            let value = std::str::from_utf8(value).map_err(|_| ())?;
            if value.is_empty() {
                return Err(());
            }
            host = Some(value.to_owned());
        }
        headers.push(HeaderLine {
            name: name.to_vec(),
            raw: line.to_vec(),
        });
        cursor = line_end + 2;
    }

    Ok(RequestHead {
        method: method.to_owned(),
        target: target.to_owned(),
        version: version.to_owned(),
        headers,
        host,
    })
}

fn parse_http_route(request: &RequestHead) -> Result<HttpRoute, ()> {
    if request.target.len() >= 7 && request.target[..7].eq_ignore_ascii_case("http://") {
        let url = Url::parse(&request.target).map_err(|_| ())?;
        validate_http_target(&url, &request.target)?;
        let host = unbracket_host(url.host_str().ok_or(())?);
        let port = url
            .port_or_known_default()
            .filter(|port| *port != 0)
            .ok_or(())?;
        let mut direct_request_target = url.path().to_owned();
        if direct_request_target.is_empty() {
            direct_request_target.push('/');
        }
        if let Some(query) = url.query() {
            direct_request_target.push('?');
            direct_request_target.push_str(query);
        }
        return Ok(HttpRoute {
            target: Target { host, port },
            direct_request_target,
            upstream_request_target: request.target.clone(),
        });
    }

    if !request.target.starts_with('/') || request.target.contains('#') {
        return Err(());
    }
    let authority = request.host.as_deref().ok_or(())?;
    if authority.contains('@') {
        return Err(());
    }
    let base = Url::parse(&format!("http://{authority}")).map_err(|_| ())?;
    if !base.username().is_empty()
        || base.password().is_some()
        || base.path() != "/"
        || base.query().is_some()
        || base.fragment().is_some()
        || base.port() == Some(0)
    {
        return Err(());
    }
    let host = unbracket_host(base.host_str().ok_or(())?);
    let port = base
        .port_or_known_default()
        .filter(|port| *port != 0)
        .ok_or(())?;
    Ok(HttpRoute {
        target: Target { host, port },
        direct_request_target: request.target.clone(),
        upstream_request_target: format!("http://{authority}{}", request.target),
    })
}

fn validate_http_target(url: &Url, raw: &str) -> Result<(), ()> {
    if url.scheme() != "http"
        || url.host_str().is_none()
        || has_explicit_url_userinfo(raw)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err(());
    }
    Ok(())
}

fn parse_connect_target(authority: &str) -> Result<Target, ()> {
    if authority.is_empty()
        || !authority.is_ascii()
        || authority
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
        || authority
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'?' | b'#'))
        || authority.contains('@')
    {
        return Err(());
    }
    let url = Url::parse(&format!("http://{authority}")).map_err(|_| ())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err(());
    }
    let host = unbracket_host(url.host_str().ok_or(())?);
    let port = url.port().unwrap_or(443);
    Ok(Target { host, port })
}

fn build_outgoing_head(request: &RequestHead, request_target: &str) -> Vec<u8> {
    let mut outgoing = Vec::with_capacity(
        request_target.len()
            + request
                .headers
                .iter()
                .map(|header| header.raw.len() + 2)
                .sum::<usize>()
            + 64,
    );
    outgoing.extend_from_slice(request.method.as_bytes());
    outgoing.push(b' ');
    outgoing.extend_from_slice(request_target.as_bytes());
    outgoing.push(b' ');
    outgoing.extend_from_slice(request.version.as_bytes());
    outgoing.extend_from_slice(b"\r\n");
    for header in &request.headers {
        if header.name.eq_ignore_ascii_case(b"proxy-authorization")
            || header.name.eq_ignore_ascii_case(b"proxy-connection")
            || header.name.eq_ignore_ascii_case(b"connection")
        {
            continue;
        }
        outgoing.extend_from_slice(&header.raw);
        outgoing.extend_from_slice(b"\r\n");
    }
    outgoing.extend_from_slice(b"Connection: close\r\n\r\n");
    outgoing
}

fn parse_response_status(header: &[u8]) -> Option<u16> {
    let line_end = find_crlf(header)?;
    let line = std::str::from_utf8(&header[..line_end]).ok()?;
    let mut parts = line.split(' ');
    let version = parts.next()?;
    let status = parts.next()?;
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || status.len() != 3
        || !status.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    status.parse().ok()
}

async fn write_error<S>(stream: &mut S, status: u16, reason: &str) -> io::Result<()>
where
    S: AsyncWrite + Unpin + ?Sized,
{
    let response =
        format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
    stream.write_all(response.as_bytes()).await
}

fn is_http_token(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.iter().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\r\n")
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while bytes
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        bytes = &bytes[1..];
    }
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn unbracket_host(host: &str) -> String {
    host.strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .to_owned()
}

fn has_explicit_url_userinfo(value: &str) -> bool {
    let Some((_, remainder)) = value.split_once("://") else {
        return false;
    };
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    remainder[..authority_end].contains('@')
}

pub fn is_loopback_proxy_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let host = host.strip_suffix('.').unwrap_or(host);
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::net::SocketAddr;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        task::JoinHandle,
    };
    use tokio_rustls::TlsAcceptor;

    const HOST: &str = "127.0.0.1";
    const PROXY_CERTIFICATE: &str = include_str!("../../../test/fixtures/proxy-ca.pem");
    const PROXY_SERVER_CERTIFICATE: &str = r#"-----BEGIN CERTIFICATE-----
MIIDCTCCAfGgAwIBAgIBAjANBgkqhkiG9w0BAQsFADAUMRIwEAYDVQQDDAlsb2Nh
bGhvc3QwHhcNMjYwODExMDEzMjQ2WhcNMzYwODA4MDEzMjQ2WjAUMRIwEAYDVQQD
DAlsb2NhbGhvc3QwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQClGgH9
HIpTijPTmWGNQc3viXEky3my4lNPE/zaUJY/Yx1Wm1vyvTKuwqFFkfcIoYM0rr4i
CIDjiNnSgOZTn7RmzMlTtO+lvStH704keNTc9th/z1MBa+8pzOkLtiDzAQrIfCPa
zdimU9Gb1w8s0L+orzR87+W+Q4y+0HIdqaJA7hZ9hp2j01miVy8EwXkHyZzBX72I
GAtoCrUlnodMFYizVF9qDdQeC+eKgZMEOKfayBbntpRQkh7PJkb9/C4gHa0PZ778
bbWMukw+jytPygcm90pYqqFJpXoqo6vCsUmg3v/Od50Hcrtw+Tk/IVZgUEE9eu3a
PhDlQ5OIIYFoqUZ3AgMBAAGjZjBkMBQGA1UdEQQNMAuCCWxvY2FsaG9zdDAMBgNV
HRMBAf8EAjAAMB0GA1UdDgQWBBQR42EEr6kPCjz9qgbmAhxhBM/qOjAfBgNVHSME
GDAWgBRYKkFTq+uwOojs/PZMcmm0WsAdBzANBgkqhkiG9w0BAQsFAAOCAQEAH3MP
gWgGhnJsNpO7wmaEUe6K4yVwA0jlFm6STUsxVJ5uARIf2AQLHlBHNJlqhWjGwnpv
jPwaKpyQPXe+Qu6/EpYZQFDqzI0Qi5uEst5Ijci6yonCbvHH6mwOYS4zTbDxb7LM
DmY9LXWiLMqF9UElGz7d2AIEay1J8u6xyNZ+hS4cue3WEuQ26vWosB5wLgzK+vPa
ng6EqkaH6drQ1yAKbEtk0SJ6BJ96Y2AoFhRUtiP0JvdlpTf5/ilV/uiYc1uLOzsZ
b6od1H5XRLVwBx1m5PchcaZIV1z9H7k20aLZWUjyMGSJXFdG69kA5wJIgDwwFtFa
yBRD5BH+wXyyc/iKHg==
-----END CERTIFICATE-----"#;
    const PROXY_SERVER_PRIVATE_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQClGgH9HIpTijPT
mWGNQc3viXEky3my4lNPE/zaUJY/Yx1Wm1vyvTKuwqFFkfcIoYM0rr4iCIDjiNnS
gOZTn7RmzMlTtO+lvStH704keNTc9th/z1MBa+8pzOkLtiDzAQrIfCPazdimU9Gb
1w8s0L+orzR87+W+Q4y+0HIdqaJA7hZ9hp2j01miVy8EwXkHyZzBX72IGAtoCrUl
nodMFYizVF9qDdQeC+eKgZMEOKfayBbntpRQkh7PJkb9/C4gHa0PZ778bbWMukw+
jytPygcm90pYqqFJpXoqo6vCsUmg3v/Od50Hcrtw+Tk/IVZgUEE9eu3aPhDlQ5OI
IYFoqUZ3AgMBAAECggEAC0WvzhK1RQfiCvflZ6oO/+Q/49W6nOKD8pm4QUWQRIql
VaTszbFqNPFX5nKVTbMNTLsDgzpWZnKRAKTJVPtJ61zfAAaR+BxmyJYHnUjcq6t0
06EZlzbB0PZLEQAh1sWC8XY8UnOMb6q4XG39/L7R3xf7Ou58HpXpFZwQtoK+77TV
oEA9Er2b52G+euvd2We1TAML4PLJBigb1P3DFiCh7FJAqfP5vWFS86XE3eH3FTig
JwEI+hDu57okoQGug9HgfNA+fEWsz5UCyR2tmCHVq1ukggJvkz/ZqeWStYWUCqdO
oeqrIV6TSAHCZllXiExkurtbLrgrthz27nGxvjdF7QKBgQDU0wQylsp4TTLDoE1D
nRQHDTwT9LuJ7TGMnXiHt8cJffX51XuL56bjjXmcsHf4I4bLqdqEwYzmFCBo+qYD
N1RuUGt/U0g6Pe6tPooeR5g8ij4kYwO+sfvnfSjStaRR2Z7m6lrN3qUvgH9EPUNI
wefXsmK5SGn8mMTf8NHB6qc0PQKBgQDGmIYPrstW2UvKPgP4PZCdElIu00JNwzca
VlbL82kvFYxW+rBWJvGDD3DrOBmKUQDZwIYKvzpCgTwkYGePJYYlKTAuWlOBPkVg
zVwPXCimEhP7U733PFrEUxHTwNrfyvMDy5RacUe3cfnVa2IURq0CWjN46AdCp14s
X4WEgroswwKBgQC/MYNH32etg8zjKhO/dlITs7QRSX9hfZFR/fXWFyfcQyjDdSI0
obuwGdzzAyYD6gai2MjTEv59g/9J0ENsCDz1jZHFJRByIklVoiV65l7BpIAHOFyY
6FShtjMCeORSE+tJD6jb4fUMI6gxqcfUiT667Cr8wS1WG/hiJnqKd0AXEQKBgF0J
8ayBODP84xvhh9yRgyGDBst3H5XswfDtyAYOiBWN48yP73K9FeJPppgcFSMOpfZd
0q5QPkwP3YwxOfL/ImRgcnkUyhA/iyM5skpj44tB5uiUp+ee9+sH+88Xh7LWkpkR
k/P3JCEWHXRVtYJIRh9XAMxA773TSTRCn/uffvcfAoGAX2AExVPPwBPXJdRL/9ii
3fd/FQrmP48N/F4ZzSiEAhUEVvejCbs02uUZJd6i/Q69KpbNMgItyzfmuRPHxHAV
abb9bVp/t3PVdgPThrllLe2Ww2i4p/sBBWvArJiePu9FCczEV79VHiNRfM2cltuJ
NSmusQ/TUGcH28k4d4xY8n8=
-----END PRIVATE KEY-----"#;

    struct RunningRelay {
        address: SocketAddr,
        shutdown: CancellationToken,
        task: Option<JoinHandle<Result<(), ProxyRelayError>>>,
    }

    impl RunningRelay {
        async fn stop(mut self) {
            self.shutdown.cancel();
            self.task.take().unwrap().await.unwrap().unwrap();
        }
    }

    impl Drop for RunningRelay {
        fn drop(&mut self) {
            self.shutdown.cancel();
            if let Some(task) = &self.task {
                task.abort();
            }
        }
    }

    async fn start_relay(upstream: UpstreamProxy) -> RunningRelay {
        start_relay_with_limits(upstream, ProxyRelayLimits::default()).await
    }

    async fn start_relay_with_limits(
        upstream: UpstreamProxy,
        limits: ProxyRelayLimits,
    ) -> RunningRelay {
        let listener = TcpListener::bind((HOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            serve_with_limits(listener, upstream, task_shutdown, limits).await
        });
        RunningRelay {
            address,
            shutdown,
            task: Some(task),
        }
    }

    async fn read_test_header<S>(stream: &mut S) -> (Vec<u8>, usize)
    where
        S: AsyncRead + Unpin,
    {
        let mut buffer = Vec::new();
        let end = read_header(stream, &mut buffer).await.unwrap();
        (buffer, end)
    }

    async fn request(address: SocketAddr, head: &[u8]) -> Vec<u8> {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(head).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        response
    }

    async fn start_http_upstream() -> (SocketAddr, JoinHandle<(String, bool)>) {
        let listener = TcpListener::bind((HOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (request, header_end) = read_test_header(&mut stream).await;
            let head = String::from_utf8_lossy(&request[..header_end]);
            let target = head
                .lines()
                .next()
                .unwrap()
                .split(' ')
                .nth(1)
                .unwrap()
                .to_owned();
            let leaked_auth = head.to_ascii_lowercase().contains("proxy-authorization:");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nvia-upstream",
                )
                .await
                .unwrap();
            (target, leaked_auth)
        });
        (address, task)
    }

    async fn start_echo() -> (SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind((HOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 32];
            loop {
                let read = stream.read(&mut bytes).await.unwrap();
                if read == 0 {
                    break;
                }
                stream.write_all(&bytes[..read]).await.unwrap();
            }
        });
        (address, task)
    }

    async fn start_http_target() -> (SocketAddr, JoinHandle<String>) {
        let listener = TcpListener::bind((HOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (request, header_end) = read_test_header(&mut stream).await;
            let request_target = String::from_utf8_lossy(&request[..header_end])
                .lines()
                .next()
                .unwrap()
                .split(' ')
                .nth(1)
                .unwrap()
                .to_owned();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{request_target}",
                request_target.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request_target
        });
        (address, task)
    }

    async fn open_tunnel(address: SocketAddr, authority: &str) -> TcpStream {
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(
                format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let (response, header_end) = read_test_header(&mut client).await;
        assert!(String::from_utf8_lossy(&response[..header_end]).contains(" 200 "));
        assert_eq!(response.len(), header_end);
        client
    }

    #[test]
    fn validates_upstream_urls_and_loopback_hosts() {
        assert!(!UpstreamProxy::parse("http://proxy.example:8080")
            .unwrap()
            .is_direct());
        assert!(!UpstreamProxy::parse("https://proxy.example:8443")
            .unwrap()
            .is_direct());
        for value in [
            "http://user@proxy.example:8080",
            "http://@proxy.example:8080",
            "http://proxy.example:8080/path",
            "http://proxy.example:8080?mode=1",
            "http://proxy.example:8080?",
            "http://proxy.example:8080#",
            "http://proxy.example:0",
            " http://proxy.example:8080",
            "socks5://proxy.example:1080",
        ] {
            assert!(matches!(
                UpstreamProxy::parse(value),
                Err(ProxyRelayError::InvalidUpstreamProxy)
            ));
        }

        assert!(is_loopback_proxy_host("127.0.0.1"));
        assert!(is_loopback_proxy_host("localhost"));
        assert!(is_loopback_proxy_host("[::1]"));
        assert!(!is_loopback_proxy_host("127.0.0.2"));
        assert!(!is_loopback_proxy_host("localhost.example"));
    }

    #[tokio::test]
    async fn external_http_uses_upstream_and_strips_proxy_credentials() {
        let (upstream_address, upstream_task) = start_http_upstream().await;
        let upstream = UpstreamProxy::parse(&format!(
            "http://{}:{}",
            upstream_address.ip(),
            upstream_address.port()
        ))
        .unwrap();
        let relay = start_relay(upstream).await;
        let target = "http://outside.invalid:8123/music?id=7%2F8";
        let response = request(
            relay.address,
            format!(
                "GET {target} HTTP/1.1\r\nHost: outside.invalid:8123\r\nProxy-Authorization: Basic secret\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await;
        assert!(String::from_utf8_lossy(&response).ends_with("via-upstream"));
        let (received_target, leaked_auth) = upstream_task.await.unwrap();
        assert_eq!(received_target, target);
        assert!(!leaked_auth);
        relay.stop().await;
    }

    #[tokio::test]
    async fn loopback_http_and_connect_bypass_upstream_and_shutdown_closes_tunnel() {
        let (upstream_address, upstream_task) = start_http_upstream().await;
        let (http_address, http_task) = start_http_target().await;
        let (echo_address, echo_task) = start_echo().await;
        let upstream = UpstreamProxy::parse(&format!(
            "http://{}:{}",
            upstream_address.ip(),
            upstream_address.port()
        ))
        .unwrap();
        let relay = start_relay(upstream).await;

        let response = request(
            relay.address,
            format!(
                "GET http://127.0.0.1:{}/one?q=1 HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                http_address.port(),
                http_address.port()
            )
            .as_bytes(),
        )
        .await;
        assert!(String::from_utf8_lossy(&response).ends_with("/one?q=1"));
        assert_eq!(http_task.await.unwrap(), "/one?q=1");

        let mut tunnel =
            open_tunnel(relay.address, &format!("127.0.0.1:{}", echo_address.port())).await;
        tunnel.write_all(b"direct").await.unwrap();
        let mut echoed = [0_u8; 6];
        tunnel.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"direct");

        relay.stop().await;
        let mut after_shutdown = [0_u8; 1];
        assert_eq!(tunnel.read(&mut after_shutdown).await.unwrap(), 0);
        upstream_task.abort();
        echo_task.abort();
    }

    #[tokio::test]
    async fn external_connect_uses_http_upstream() {
        let listener = TcpListener::bind((HOST, 0)).await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (request, header_end) = read_test_header(&mut stream).await;
            let first_line = String::from_utf8_lossy(&request[..header_end])
                .lines()
                .next()
                .unwrap()
                .to_owned();
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            let mut payload = [0_u8; 5];
            stream.read_exact(&mut payload).await.unwrap();
            stream.write_all(&payload).await.unwrap();
            first_line
        });
        let relay = start_relay(
            UpstreamProxy::parse(&format!(
                "http://{}:{}",
                upstream_address.ip(),
                upstream_address.port()
            ))
            .unwrap(),
        )
        .await;
        let mut tunnel = open_tunnel(relay.address, "outside.invalid:443").await;
        tunnel.write_all(b"hello").await.unwrap();
        let mut echoed = [0_u8; 5];
        tunnel.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"hello");
        assert_eq!(
            upstream_task.await.unwrap(),
            "CONNECT outside.invalid:443 HTTP/1.1"
        );
        relay.stop().await;
    }

    #[tokio::test]
    async fn https_upstream_forwards_http_and_connect_over_tls() {
        let certificate = CertificateDer::pem_slice_iter(PROXY_SERVER_CERTIFICATE.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        let private_key =
            PrivatePkcs8KeyDer::from_pem_slice(PROXY_SERVER_PRIVATE_KEY.as_bytes()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls_config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], PrivateKeyDer::Pkcs8(private_key))
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(tls_config));
        let listener = TcpListener::bind((HOST, 0)).await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let mut observations = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = acceptor.accept(stream).await.unwrap();
                let (request, header_end) = read_test_header(&mut stream).await;
                let first_line = String::from_utf8_lossy(&request[..header_end])
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned();
                observations.push(first_line.clone());
                if first_line.starts_with("CONNECT ") {
                    stream
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await
                        .unwrap();
                    let mut payload = [0_u8; 6];
                    stream.read_exact(&mut payload).await.unwrap();
                    stream.write_all(&payload).await.unwrap();
                } else {
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecure",
                        )
                        .await
                        .unwrap();
                }
            }
            observations
        });
        let upstream =
            UpstreamProxy::parse(&format!("https://localhost:{}", upstream_address.port()))
                .unwrap()
                .with_tls_ca_pem(PROXY_CERTIFICATE.as_bytes())
                .unwrap();
        let relay = start_relay(upstream).await;

        let response = request(
            relay.address,
            b"GET http://outside.invalid:8123/secure?id=7 HTTP/1.1\r\nHost: outside.invalid:8123\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(
            String::from_utf8_lossy(&response).ends_with("secure"),
            "unexpected relay response: {}",
            String::from_utf8_lossy(&response)
        );
        let mut tunnel = open_tunnel(relay.address, "outside.invalid:443").await;
        tunnel.write_all(b"secure").await.unwrap();
        let mut echoed = [0_u8; 6];
        tunnel.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"secure");
        tunnel.shutdown().await.unwrap();

        let observations = upstream_task.await.unwrap();
        assert_eq!(
            observations,
            [
                "GET http://outside.invalid:8123/secure?id=7 HTTP/1.1",
                "CONNECT outside.invalid:443 HTTP/1.1",
            ]
        );
        relay.stop().await;
    }

    #[tokio::test]
    async fn enforces_header_request_and_connection_limits() {
        let limits = ProxyRelayLimits {
            max_connections: 1,
            connect_timeout: Duration::from_millis(80),
            header_timeout: Duration::from_millis(80),
            io_idle_timeout: Duration::from_millis(80),
            loopback_idle_timeout: Duration::from_millis(80),
        };
        let relay = start_relay_with_limits(UpstreamProxy::direct(), limits).await;

        let (echo_address, echo_task) = start_echo().await;
        let mut tunnel =
            open_tunnel(relay.address, &format!("127.0.0.1:{}", echo_address.port())).await;
        let mut queued = TcpStream::connect(relay.address).await.unwrap();
        queued
            .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n")
            .await
            .unwrap();
        let (queued_response, queued_header_end) =
            timeout(Duration::from_secs(1), read_test_header(&mut queued))
                .await
                .unwrap();
        assert!(String::from_utf8_lossy(&queued_response[..queued_header_end]).contains(" 503 "));

        tunnel.shutdown().await.unwrap();
        drop(tunnel);
        echo_task.await.unwrap();
        tokio::task::yield_now().await;

        let mut idle = TcpStream::connect(relay.address).await.unwrap();
        let mut idle_response = Vec::new();
        timeout(Duration::from_secs(1), idle.read_to_end(&mut idle_response))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&idle_response).contains(" 408 "));

        let response = request(
            relay.address,
            b"GET / HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n",
        )
        .await;
        assert!(String::from_utf8_lossy(&response).contains(" 502 "));

        let prefix = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Large: ";
        let oversized = vec![b'a'; MAX_HEADER_BYTES - prefix.len()];
        let mut client = TcpStream::connect(relay.address).await.unwrap();
        client
            .write_all(
                [prefix.as_slice(), oversized.as_slice()]
                    .concat()
                    .as_slice(),
            )
            .await
            .unwrap();
        let (response, header_end) = read_test_header(&mut client).await;
        assert!(String::from_utf8_lossy(&response[..header_end]).contains(" 431 "));

        let mut timed_out = TcpStream::connect(relay.address).await.unwrap();
        let (response, header_end) =
            timeout(Duration::from_secs(1), read_test_header(&mut timed_out))
                .await
                .unwrap();
        assert!(String::from_utf8_lossy(&response[..header_end]).contains(" 408 "));
        relay.stop().await;
    }

    #[tokio::test]
    async fn active_streams_outlive_the_idle_budget_but_stalls_are_reclaimed() {
        let (mut client, mut relay_client) = tokio::io::duplex(64);
        let (mut relay_server, mut server) = tokio::io::duplex(64);
        let relay = tokio::spawn(async move {
            relay_with_idle_timeout(
                &mut relay_client,
                &mut relay_server,
                Duration::from_millis(200),
            )
            .await
        });

        for byte in b"active" {
            client.write_all(&[*byte]).await.unwrap();
            let mut received = [0_u8; 1];
            server.read_exact(&mut received).await.unwrap();
            assert_eq!(received[0], *byte);
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        assert!(!relay.is_finished());
        timeout(Duration::from_secs(1), relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
