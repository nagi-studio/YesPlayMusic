use std::{io, sync::Arc, time::Duration};

use axum::{middleware, routing::any, Router};
use tokio::{
    io::AsyncRead,
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

use crate::{
    config::SidecarConfig,
    health::{self, HealthState},
    ncm::{self, NcmState},
    player_api::{self, PlayerApiState},
    precise_wav::{self, PreciseWavState},
    proxy_relay::{self, UpstreamProxy},
    renderer::{self, ApiProxy},
    session::{self, RequestBoundary},
    unm::{self, UnmState},
};

pub const LEGACY_PLAYER_PORT: u16 = 27_232;
const LOOPBACK_HOST: &str = "127.0.0.1";
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("invalid upstream proxy: {0}")]
    InvalidProxy(#[from] proxy_relay::ProxyRelayError),
    #[error("failed to build NCM routes: {0}")]
    NcmRouter(#[from] ncm::NcmRouterError),
    #[error("failed to initialize the renderer API proxy: {0}")]
    ApiProxy(#[from] reqwest::Error),
    #[error("failed to initialize precise WAV storage: {0}")]
    PreciseWav(#[source] io::Error),
    #[error("renderer directory is not a directory: {0}")]
    InvalidRendererDirectory(String),
    #[error("failed to bind {listener} on 127.0.0.1:{port}: {source}")]
    Bind {
        listener: &'static str,
        port: u16,
        #[source]
        source: io::Error,
    },
    #[error("{listener} listener stopped unexpectedly")]
    ListenerStopped { listener: &'static str },
    #[error("{listener} listener failed: {detail}")]
    ListenerFailed {
        listener: &'static str,
        detail: String,
    },
    #[error("listener task failed: {0}")]
    ListenerTask(#[from] tokio::task::JoinError),
    #[error("local listeners did not stop within five seconds")]
    ShutdownTimeout,
    #[error("failed to generate the native API token: {0}")]
    NativeToken(String),
}

struct BoundListeners {
    api: TcpListener,
    player: TcpListener,
    renderer: Option<TcpListener>,
    proxy: Option<TcpListener>,
}

async fn bind_listener(name: &'static str, port: u16) -> Result<TcpListener, ServerError> {
    TcpListener::bind((LOOPBACK_HOST, port))
        .await
        .map_err(|source| ServerError::Bind {
            listener: name,
            port,
            source,
        })
}

impl BoundListeners {
    async fn bind(config: &SidecarConfig) -> Result<Self, ServerError> {
        let api = bind_listener("API", config.api_port).await?;
        let player = bind_listener("player compatibility API", LEGACY_PLAYER_PORT).await?;
        let renderer = if config.api_only {
            None
        } else {
            Some(bind_listener("renderer", config.web_port).await?)
        };
        let proxy = if config.upstream_proxy.is_some() {
            Some(bind_listener("WebView proxy relay", config.proxy_relay_port).await?)
        } else {
            None
        };
        Ok(Self {
            api,
            player,
            renderer,
            proxy,
        })
    }
}

fn random_hex_token() -> Result<String, ServerError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| ServerError::NativeToken(error.to_string()))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(64);
    for byte in bytes {
        token.push(HEX[(byte >> 4) as usize] as char);
        token.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(token)
}

fn with_boundary(router: Router, boundary: RequestBoundary) -> Router {
    router.layer(middleware::from_fn_with_state(
        boundary,
        session::enforce_local_boundary,
    ))
}

async fn build_api_router(
    config: &SidecarConfig,
    health_state: HealthState,
    native_token: Option<String>,
    player_state: PlayerApiState,
) -> Result<Router, ServerError> {
    let client = Arc::new(ncm_api_rs::ApiClient::new(None));
    let mut router = ncm::router(NcmState::production(client))?
        .merge(unm::router(UnmState::new()))
        .merge(player_api::update_router(player_state));
    if config.api_only {
        router = router.merge(health::router(health_state));
    }
    let origin_port = if config.api_only {
        1_420
    } else {
        config.web_port
    };
    Ok(with_boundary(
        router,
        RequestBoundary::new(
            [format!("http://{LOOPBACK_HOST}:{origin_port}")],
            native_token,
        ),
    ))
}

async fn build_renderer_router(
    config: &SidecarConfig,
    health_state: HealthState,
    native_token: String,
    player_state: PlayerApiState,
) -> Result<Router, ServerError> {
    let renderer_dir = config
        .renderer_dir
        .as_ref()
        .expect("validated non-API-only configuration")
        .clone();
    if !renderer_dir.is_dir() {
        return Err(ServerError::InvalidRendererDirectory(
            renderer_dir.display().to_string(),
        ));
    }
    let proxy = ApiProxy::new(config.api_port, native_token)?;
    let precise_wav = PreciseWavState::production()
        .await
        .map_err(ServerError::PreciseWav)?;
    let fallback_dir = renderer_dir.clone();
    let router = health::router(health_state)
        .merge(player_api::player_router(player_state))
        .merge(precise_wav::router(precise_wav))
        .merge(
            Router::new()
                .route("/api", any(renderer::proxy_api))
                .route("/api/{*path}", any(renderer::proxy_api))
                .with_state(proxy),
        )
        .fallback(move |request| renderer::serve_renderer(fallback_dir.clone(), request))
        .layer(middleware::from_fn(renderer::apply_security_headers));
    Ok(with_boundary(
        router,
        RequestBoundary::new(
            [format!("http://{LOOPBACK_HOST}:{}", config.web_port)],
            None,
        ),
    ))
}

fn spawn_http(
    tasks: &mut JoinSet<Result<&'static str, (&'static str, String)>>,
    name: &'static str,
    listener: TcpListener,
    router: Router,
    shutdown: CancellationToken,
) {
    tasks.spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .map(|()| name)
            .map_err(|error| (name, error.to_string()))
    });
}

async fn cancel_on_signal(shutdown: CancellationToken) {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        if let Ok(mut terminate) = terminate {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            }
            shutdown.cancel();
            return;
        }
    }
    tokio::select! {
        _ = shutdown.cancelled() => {},
        _ = tokio::signal::ctrl_c() => shutdown.cancel(),
    }
}

async fn stop_monitors(monitors: Vec<JoinHandle<()>>) {
    for monitor in monitors {
        let _ = monitor.await;
    }
}

async fn drain_listeners(
    tasks: &mut JoinSet<Result<&'static str, (&'static str, String)>>,
) -> Result<(), ServerError> {
    let drain = async {
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(_)) => {}
                Ok(Err((listener, detail))) => {
                    tracing::warn!(listener, "listener failed during shutdown: {detail}");
                }
                Err(error) if error.is_cancelled() => {}
                Err(error) => return Err(ServerError::ListenerTask(error)),
            }
        }
        Ok(())
    };
    tokio::time::timeout(SHUTDOWN_TIMEOUT, drain)
        .await
        .map_err(|_| {
            tasks.abort_all();
            ServerError::ShutdownTimeout
        })?
}

pub async fn run<R>(
    config: SidecarConfig,
    health_state: HealthState,
    input: R,
) -> Result<(), ServerError>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let upstream = config
        .upstream_proxy
        .as_deref()
        .map(UpstreamProxy::parse)
        .transpose()?;
    let native_token = (!config.api_only).then(random_hex_token).transpose()?;
    let listeners = BoundListeners::bind(&config).await?;
    let player_state = PlayerApiState::new();
    let api_router = build_api_router(
        &config,
        health_state.clone(),
        native_token.clone(),
        player_state.clone(),
    )
    .await?;
    let renderer_router = match native_token {
        Some(token) => {
            Some(build_renderer_router(&config, health_state, token, player_state.clone()).await?)
        }
        None => None,
    };

    let shutdown = CancellationToken::new();
    let mut monitors = vec![tokio::spawn(health::cancel_on_parent_input(
        input,
        shutdown.clone(),
    ))];
    if let Some(parent_pid) = config.parent_pid {
        monitors.push(tokio::spawn(health::cancel_when_parent_exits(
            parent_pid,
            shutdown.clone(),
        )));
    }
    monitors.push(tokio::spawn(cancel_on_signal(shutdown.clone())));

    let mut tasks = JoinSet::new();
    spawn_http(
        &mut tasks,
        "API",
        listeners.api,
        api_router,
        shutdown.clone(),
    );
    spawn_http(
        &mut tasks,
        "player compatibility API",
        listeners.player,
        player_api::player_router(player_state),
        shutdown.clone(),
    );
    if let (Some(listener), Some(router)) = (listeners.renderer, renderer_router) {
        spawn_http(&mut tasks, "renderer", listener, router, shutdown.clone());
    }
    if let (Some(listener), Some(upstream)) = (listeners.proxy, upstream) {
        let relay_shutdown = shutdown.clone();
        tasks.spawn(async move {
            proxy_relay::serve(listener, upstream, relay_shutdown)
                .await
                .map(|()| "WebView proxy relay")
                .map_err(|error| ("WebView proxy relay", error.to_string()))
        });
    }

    tracing::info!(
        api_port = config.api_port,
        web_port = if config.api_only {
            None
        } else {
            Some(config.web_port)
        },
        player_port = LEGACY_PLAYER_PORT,
        proxy_port = config
            .upstream_proxy
            .as_ref()
            .map(|_| config.proxy_relay_port),
        "sidecar listeners are ready"
    );

    let unexpected = tokio::select! {
        _ = shutdown.cancelled() => None,
        result = tasks.join_next() => result,
    };
    let was_cancelled = shutdown.is_cancelled();
    shutdown.cancel();

    let primary_error = match unexpected {
        Some(Ok(Ok(listener))) if !was_cancelled => Some(ServerError::ListenerStopped { listener }),
        Some(Ok(Err((listener, detail)))) if !was_cancelled => {
            Some(ServerError::ListenerFailed { listener, detail })
        }
        Some(Err(error)) if !was_cancelled => Some(ServerError::ListenerTask(error)),
        _ => None,
    };
    let drain_result = drain_listeners(&mut tasks).await;
    stop_monitors(monitors).await;
    if let Some(error) = primary_error {
        Err(error)
    } else {
        drain_result
    }
}
