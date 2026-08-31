//! Player actor: rodio lives on one dedicated thread; the UI only talks
//! Command in / Event out. Generation stamps let the reducer drop stale
//! events after a track switch.
//!
//! Spike-proven rules (see design charter appendix):
//! - every Decoder must know its byte length, or backward seeks fail;
//! - rodio is pinned to 0.22.x (0.20 panics opening M4A).

mod cache_stream;

use std::io::{Cursor, Read, Seek};
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::{Duration, Instant};

use rodio::{Decoder, Player};
use stream_download::{Settings, StreamDownload, StreamHandle, StreamPhase};
use tokio::sync::{mpsc, oneshot};
use yesplaymusic_core::cache::{CacheLease, CacheMetadata};

use crate::spectrum::{SampleBuffer, SampleTap};
pub use cache_stream::CacheWritePlan;
use cache_stream::{CacheImportReader, CacheStreamProvider};

pub enum PlayerCommand {
    PlayCached {
        generation: u64,
        lease: CacheLease,
    },
    PlayUrl {
        generation: u64,
        url: String,
        cache: Option<CacheWritePlan>,
        unm_source: bool,
    },
    PlayBytes {
        generation: u64,
        bytes: Vec<u8>,
        unm_source: bool,
    },
    Play,
    TogglePause,
    SeekTo(Duration),
    SetVolume(f32),
    Stop,
}

#[derive(Debug)]
pub enum PlayerEvent {
    Started {
        generation: u64,
        total: Option<Duration>,
    },
    Position {
        generation: u64,
        position: Duration,
    },
    Paused {
        generation: u64,
        paused: bool,
    },
    Ended {
        generation: u64,
    },
    SeekFailed {
        generation: u64,
        message: String,
    },
    Failed {
        generation: u64,
        message: String,
        cached: Option<CacheMetadata>,
        unm_source: bool,
    },
}

#[derive(Clone)]
pub struct PlayerHandle {
    commands: std_mpsc::Sender<PlayerCommand>,
    wake: std_mpsc::Sender<()>,
    samples: Arc<SampleBuffer>,
}

impl PlayerHandle {
    pub fn send(&self, command: PlayerCommand) {
        if self.commands.send(command).is_ok() {
            let _ = self.wake.send(());
        }
    }

    pub fn samples(&self) -> &SampleBuffer {
        &self.samples
    }
}

pub fn spawn(
    runtime: tokio::runtime::Handle,
) -> (PlayerHandle, mpsc::UnboundedReceiver<PlayerEvent>) {
    spawn_with_engine(runtime, open_engine)
}

type EngineFactory = fn(f32) -> anyhow::Result<Engine>;

fn spawn_with_engine(
    runtime: tokio::runtime::Handle,
    engine_factory: EngineFactory,
) -> (PlayerHandle, mpsc::UnboundedReceiver<PlayerEvent>) {
    let samples = SampleBuffer::shared();
    let (command_tx, command_rx) = std_mpsc::channel();
    let (wake_tx, wake_rx) = std_mpsc::channel();
    let (work_tx, work_rx) = std_mpsc::channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let actor_wake = wake_tx.clone();
    let actor_samples = samples.clone();
    std::thread::Builder::new()
        .name("ypm-player".into())
        .spawn(move || {
            actor(ActorContext {
                runtime,
                commands: command_rx,
                wake: wake_rx,
                wake_tx: actor_wake,
                work_tx,
                work_rx,
                events: event_tx,
                engine_factory,
                samples: actor_samples,
            })
        })
        .expect("spawn player thread");
    (
        PlayerHandle {
            commands: command_tx,
            wake: wake_tx,
            samples,
        },
        event_rx,
    )
}

/// Anything rodio can decode from: a file or a stream-download reader.
trait MediaSource: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> MediaSource for T {}

/// Reader plus its byte length — carrying the length is what makes
/// backward seeks work, so the two travel together by construction.
struct Media {
    reader: Box<dyn MediaSource>,
    byte_len: Option<u64>,
    cached: Option<CacheMetadata>,
    cancel: Option<StreamCancel>,
    unm_source: bool,
}

struct UrlMedia {
    url: String,
    cache: Option<CacheWritePlan>,
    unm_source: bool,
}

type StreamCancel = Arc<dyn Fn() + Send + Sync>;
type AudioDecoder = Decoder<Box<dyn MediaSource>>;

struct Decoded {
    source: AudioDecoder,
    total: Option<Duration>,
    cancel: Option<StreamCancel>,
}

struct StartedPlayback {
    cancel: Option<StreamCancel>,
}

struct DecodeFailure {
    message: String,
    cached: Option<CacheMetadata>,
    unm_source: bool,
}

enum WorkResult {
    Opened(Result<Media, DecodeFailure>),
    Decoded(Result<Decoded, DecodeFailure>),
}

struct CompletedWork {
    request: u64,
    generation: u64,
    result: WorkResult,
}

enum PendingTask {
    Open(tokio::task::JoinHandle<()>),
    Decode { _task: std::thread::JoinHandle<()> },
}

struct PendingWork {
    request: u64,
    generation: u64,
    cancel: Option<StreamCancel>,
    task: PendingTask,
}

impl PendingWork {
    fn finish(mut self) {
        self.cancel = None;
    }
}

impl Drop for PendingWork {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
        if let PendingTask::Open(task) = &self.task {
            task.abort();
        }
    }
}

// Word-synchronised lyrics need a responsive clock without busy-redrawing.
const TICK: Duration = Duration::from_millis(50);

struct Engine {
    _output: EngineOutput,
    player: Player,
}

enum EngineOutput {
    Device {
        _sink: rodio::MixerDeviceSink,
    },
    #[cfg(test)]
    Silent {
        _source: rodio::queue::SourcesQueueOutput,
    },
}

struct ActorContext {
    runtime: tokio::runtime::Handle,
    commands: std_mpsc::Receiver<PlayerCommand>,
    wake: std_mpsc::Receiver<()>,
    wake_tx: std_mpsc::Sender<()>,
    work_tx: std_mpsc::Sender<CompletedWork>,
    work_rx: std_mpsc::Receiver<CompletedWork>,
    events: mpsc::UnboundedSender<PlayerEvent>,
    engine_factory: EngineFactory,
    samples: Arc<SampleBuffer>,
}

fn actor(context: ActorContext) {
    let ActorContext {
        runtime,
        commands,
        wake,
        wake_tx,
        work_tx,
        work_rx,
        events,
        engine_factory,
        samples,
    } = context;
    let mut engine: Option<Engine> = None;
    let mut active_generation: Option<u64> = None;
    let mut volume = 1.0_f32;
    let mut pending: Option<PendingWork> = None;
    let mut active_cancel: Option<StreamCancel> = None;
    let mut next_request = 1_u64;
    let mut last_tick = Instant::now();

    loop {
        let wait = TICK.saturating_sub(last_tick.elapsed());
        let _ = wake.recv_timeout(wait);

        let mut disconnected = false;
        loop {
            let command = match commands.try_recv() {
                Ok(command) => command,
                Err(std_mpsc::TryRecvError::Empty) => break,
                Err(std_mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            };

            match command {
                PlayerCommand::PlayCached {
                    generation: g,
                    lease,
                } => {
                    drop(pending.take());
                    stop(&engine, &mut active_cancel);
                    samples.clear();
                    active_generation = None;
                    let request = next_request;
                    next_request += 1;
                    pending = Some(spawn_decode(
                        request,
                        g,
                        open_cached(lease),
                        work_tx.clone(),
                        wake_tx.clone(),
                    ));
                }
                PlayerCommand::PlayUrl {
                    generation: g,
                    url,
                    cache,
                    unm_source,
                } => {
                    drop(pending.take());
                    stop(&engine, &mut active_cancel);
                    samples.clear();
                    active_generation = None;
                    let request = next_request;
                    next_request += 1;
                    pending = Some(spawn_open(
                        &runtime,
                        request,
                        g,
                        UrlMedia {
                            url,
                            cache,
                            unm_source,
                        },
                        work_tx.clone(),
                        wake_tx.clone(),
                    ));
                }
                PlayerCommand::PlayBytes {
                    generation: g,
                    bytes,
                    unm_source,
                } => {
                    drop(pending.take());
                    stop(&engine, &mut active_cancel);
                    samples.clear();
                    active_generation = None;
                    let request = next_request;
                    next_request += 1;
                    pending = Some(spawn_decode(
                        request,
                        g,
                        open_bytes(bytes, unm_source),
                        work_tx.clone(),
                        wake_tx.clone(),
                    ));
                }
                PlayerCommand::Play => {
                    if let (Some(engine), Some(_)) = (&engine, active_generation) {
                        engine.player.play();
                    }
                }
                PlayerCommand::TogglePause => {
                    if let (Some(engine), Some(generation)) = (&engine, active_generation) {
                        let paused = !engine.player.is_paused();
                        if paused {
                            engine.player.pause();
                        } else {
                            engine.player.play();
                        }
                        let _ = events.send(PlayerEvent::Paused { generation, paused });
                    }
                }
                PlayerCommand::SeekTo(position) => {
                    if let (Some(engine), Some(generation)) = (&engine, active_generation) {
                        if let Err(error) = engine.player.try_seek(position) {
                            let _ = events.send(PlayerEvent::SeekFailed {
                                generation,
                                message: format!("seek failed: {error}"),
                            });
                        }
                    }
                }
                PlayerCommand::SetVolume(value) => {
                    volume = value.clamp(0.0, 1.5);
                    if let Some(engine) = &engine {
                        engine.player.set_volume(volume);
                    }
                }
                PlayerCommand::Stop => {
                    drop(pending.take());
                    stop(&engine, &mut active_cancel);
                    samples.clear();
                    active_generation = None;
                }
            }
        }

        if disconnected {
            drop(pending.take());
            stop(&engine, &mut active_cancel);
            break;
        }

        while let Ok(completed) = work_rx.try_recv() {
            let is_current = pending.as_ref().is_some_and(|candidate| {
                candidate.request == completed.request
                    && candidate.generation == completed.generation
            });
            if !is_current {
                continue;
            }

            pending.take().expect("current player work").finish();
            match completed.result {
                WorkResult::Opened(Ok(media)) => {
                    pending = Some(spawn_decode(
                        completed.request,
                        completed.generation,
                        media,
                        work_tx.clone(),
                        wake_tx.clone(),
                    ));
                }
                WorkResult::Opened(Err(error)) => {
                    report_failure(&events, completed.generation, error);
                }
                WorkResult::Decoded(Ok(decoded)) => {
                    if let Some(started) = start_decoded(
                        &mut engine,
                        volume,
                        completed.generation,
                        &events,
                        decoded,
                        engine_factory,
                        samples.clone(),
                    ) {
                        active_generation = Some(completed.generation);
                        active_cancel = started.cancel;
                    }
                }
                WorkResult::Decoded(Err(error)) => {
                    report_failure(&events, completed.generation, error);
                }
            }
        }

        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            if let (Some(engine), Some(generation)) = (&engine, active_generation) {
                if engine.player.empty() {
                    active_generation = None;
                    active_cancel = None;
                    let _ = events.send(PlayerEvent::Ended { generation });
                } else if !engine.player.is_paused() {
                    let _ = events.send(PlayerEvent::Position {
                        generation,
                        position: engine.player.get_pos(),
                    });
                }
            }
        }
    }
}

fn stop(engine: &Option<Engine>, active_cancel: &mut Option<StreamCancel>) {
    if let Some(cancel) = active_cancel.take() {
        cancel();
    }
    if let Some(engine) = engine {
        engine.player.stop();
    }
}

fn spawn_open(
    runtime: &tokio::runtime::Handle,
    request: u64,
    generation: u64,
    media: UrlMedia,
    completed: std_mpsc::Sender<CompletedWork>,
    wake: std_mpsc::Sender<()>,
) -> PendingWork {
    let task = runtime.spawn(async move {
        let result = open_url(&media.url, media.cache, media.unm_source)
            .await
            .map_err(|error| DecodeFailure {
                message: error.to_string(),
                cached: None,
                unm_source: media.unm_source,
            });
        let _ = completed.send(CompletedWork {
            request,
            generation,
            result: WorkResult::Opened(result),
        });
        let _ = wake.send(());
    });
    PendingWork {
        request,
        generation,
        cancel: None,
        task: PendingTask::Open(task),
    }
}

fn spawn_decode(
    request: u64,
    generation: u64,
    media: Media,
    completed: std_mpsc::Sender<CompletedWork>,
    wake: std_mpsc::Sender<()>,
) -> PendingWork {
    let cancel = media.cancel.clone();
    let task = std::thread::Builder::new()
        .name("ypm-decoder".into())
        .spawn(move || {
            let result = decode(media);
            let _ = completed.send(CompletedWork {
                request,
                generation,
                result: WorkResult::Decoded(result),
            });
            let _ = wake.send(());
        })
        .expect("spawn decoder thread");
    PendingWork {
        request,
        generation,
        cancel,
        task: PendingTask::Decode { _task: task },
    }
}

fn open_cached(lease: CacheLease) -> Media {
    let metadata = *lease.metadata();
    Media {
        reader: Box::new(lease),
        byte_len: Some(metadata.bytes),
        cached: Some(metadata),
        cancel: None,
        unm_source: false,
    }
}

fn open_bytes(bytes: Vec<u8>, unm_source: bool) -> Media {
    Media {
        byte_len: Some(bytes.len() as u64),
        reader: Box::new(Cursor::new(bytes)),
        cached: None,
        cancel: None,
        unm_source,
    }
}

/// stream-download's reqwest is built with `rustls-no-provider` so the whole
/// binary links a single crypto backend; that variant requires installing the
/// process default provider before the first client is built.
fn install_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

async fn open_url(
    url: &str,
    cache: Option<CacheWritePlan>,
    unm_source: bool,
) -> anyhow::Result<Media> {
    install_crypto_provider();
    let (provider, import) = CacheStreamProvider::new()?;
    let (complete_tx, complete_rx) = oneshot::channel();
    let mut complete_tx = cache.as_ref().map(|_| complete_tx);
    let settings = Settings::default().on_progress(move |_, state, _| {
        if matches!(state.phase, StreamPhase::Complete) {
            if let Some(complete) = complete_tx.take() {
                let _ = complete.send(());
            }
        }
    });
    let reader = StreamDownload::new_http(url.parse()?, provider, settings).await?;
    let byte_len = reader.content_length();
    let cancellation = reader.cancellation_token();
    let cancel: StreamCancel = Arc::new(move || cancellation.cancel());
    if let Some(plan) = cache {
        spawn_cache_publish(reader.handle(), complete_rx, import, plan);
    }
    Ok(Media {
        reader: Box::new(reader),
        byte_len,
        cached: None,
        cancel: Some(cancel),
        unm_source,
    })
}

fn spawn_cache_publish(
    handle: StreamHandle,
    complete: oneshot::Receiver<()>,
    import: CacheImportReader,
    plan: CacheWritePlan,
) {
    tokio::spawn(async move {
        if complete.await.is_err() {
            return;
        }
        handle.wait_for_completion().await;
        match tokio::task::spawn_blocking(move || cache_stream::publish(import, plan)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "audio cache write failed"),
            Err(error) => tracing::warn!(%error, "audio cache worker failed"),
        }
    });
}

fn decode(media: Media) -> Result<Decoded, DecodeFailure> {
    let Media {
        reader,
        byte_len,
        cached,
        cancel,
        unm_source,
    } = media;
    let mut builder = Decoder::builder().with_data(reader).with_seekable(true);
    if let Some(byte_len) = byte_len {
        builder = builder.with_byte_len(byte_len);
    }
    let source = builder.build().map_err(|error| DecodeFailure {
        message: error.to_string(),
        cached,
        unm_source,
    })?;
    let total = rodio::Source::total_duration(&source);
    Ok(Decoded {
        source,
        total,
        cancel,
    })
}

fn start_decoded(
    engine: &mut Option<Engine>,
    volume: f32,
    generation: u64,
    events: &mpsc::UnboundedSender<PlayerEvent>,
    decoded: Decoded,
    engine_factory: EngineFactory,
    samples: Arc<SampleBuffer>,
) -> Option<StartedPlayback> {
    let engine = match engine {
        Some(engine) => engine,
        None => match engine_factory(volume) {
            Ok(opened) => engine.insert(opened),
            Err(error) => {
                let _ = events.send(PlayerEvent::Failed {
                    generation,
                    message: format!("audio device unavailable: {error}"),
                    cached: None,
                    unm_source: false,
                });
                return None;
            }
        },
    };

    engine.player.stop();
    engine.player.pause();
    engine
        .player
        .append(SampleTap::new(decoded.source, samples));
    let _ = events.send(PlayerEvent::Started {
        generation,
        total: decoded.total,
    });
    Some(StartedPlayback {
        cancel: decoded.cancel,
    })
}

fn report_failure(
    events: &mpsc::UnboundedSender<PlayerEvent>,
    generation: u64,
    failure: DecodeFailure,
) {
    let _ = events.send(PlayerEvent::Failed {
        generation,
        message: failure.message,
        cached: failure.cached,
        unm_source: failure.unm_source,
    });
}

#[cfg(test)]
fn start<F>(
    engine: &mut Option<Engine>,
    volume: f32,
    generation: u64,
    events: &mpsc::UnboundedSender<PlayerEvent>,
    open: F,
) -> bool
where
    F: FnOnce() -> anyhow::Result<Media>,
{
    let media = match open() {
        Ok(media) => media,
        Err(error) => {
            report_failure(
                events,
                generation,
                DecodeFailure {
                    message: error.to_string(),
                    cached: None,
                    unm_source: false,
                },
            );
            return false;
        }
    };
    let decoded = match decode(media) {
        Ok(decoded) => decoded,
        Err(error) => {
            report_failure(events, generation, error);
            return false;
        }
    };
    start_decoded(
        engine,
        volume,
        generation,
        events,
        decoded,
        open_engine,
        SampleBuffer::shared(),
    )
    .is_some()
}

fn open_engine(volume: f32) -> anyhow::Result<Engine> {
    let device = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let player = Player::connect_new(device.mixer());
    player.set_volume(volume);
    Ok(Engine {
        _output: EngineOutput::Device { _sink: device },
        player,
    })
}

#[cfg(test)]
fn open_silent_engine(volume: f32) -> anyhow::Result<Engine> {
    let (player, source) = Player::new();
    player.set_volume(volume);
    Ok(Engine {
        _output: EngineOutput::Silent { _source: source },
        player,
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::mpsc as std_mpsc;
    use std::thread;
    use std::time::Duration;

    use tokio::sync::{mpsc, oneshot};
    use yesplaymusic_core::cache::{
        AudioCodec, AudioQuality, CacheKey, CacheWriteRequest, TrackCache,
    };

    use super::{
        open_cached, open_silent_engine, open_url, spawn, spawn_with_engine, start, CacheWritePlan,
        PlayerCommand, PlayerEvent,
    };

    const AUDIO_BODY: &[u8] = b"complete cache body";
    const WAV_BODY: &[u8] = b"RIFF,\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0@\x1f\0\0\x80>\0\0\x02\0\x10\0data\x08\0\0\0\0\0\0\0\0\0\0\0";

    #[test]
    fn position_updates_are_frequent_enough_for_word_synced_lyrics() {
        assert!(super::TICK <= Duration::from_millis(100));
    }

    fn cache_request(track_id: i64, expected_bytes: u64) -> CacheWriteRequest {
        CacheWriteRequest::new(
            CacheKey::new(track_id, AudioQuality::High320),
            AudioCodec::Mp3,
            320_000,
        )
        .with_expected_bytes(expected_bytes)
        .with_expected_md5([
            0xe8, 0xa9, 0x92, 0x1b, 0xe8, 0x6b, 0xc2, 0x3f, 0x73, 0x2f, 0xa2, 0x62, 0x13, 0xec,
            0x6e, 0x05,
        ])
    }

    struct HttpServer {
        url: String,
        thread: thread::JoinHandle<()>,
    }

    impl HttpServer {
        fn complete(body: &'static [u8]) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind complete server");
            let address = listener.local_addr().expect("server address");
            let thread = thread::spawn(move || {
                let (mut socket, _) = listener.accept().expect("accept player request");
                read_request(&mut socket);
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .expect("write response headers");
                socket.write_all(body).expect("write response body");
            });
            Self {
                url: format!("http://{address}/audio"),
                thread,
            }
        }

        fn stalled_prefix(
            prefix: &'static [u8],
            content_length: usize,
        ) -> (Self, oneshot::Receiver<()>, std_mpsc::Receiver<()>) {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind partial server");
            let address = listener.local_addr().expect("server address");
            let (prefix_tx, prefix_sent) = oneshot::channel();
            let (closed_tx, closed) = std_mpsc::channel();
            let thread = thread::spawn(move || {
                let (mut socket, _) = listener.accept().expect("accept player request");
                read_request(&mut socket);
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
                )
                .expect("write response headers");
                socket.write_all(prefix).expect("write response prefix");
                socket.flush().expect("flush response prefix");
                let _ = prefix_tx.send(());
                let mut buffer = [0_u8; 256];
                while socket.read(&mut buffer).is_ok_and(|read| read != 0) {}
                let _ = closed_tx.send(());
            });
            (
                Self {
                    url: format!("http://{address}/audio"),
                    thread,
                },
                prefix_sent,
                closed,
            )
        }

        fn join(self) {
            self.thread.join().expect("join HTTP server");
        }
    }

    fn read_request(socket: &mut std::net::TcpStream) {
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("set request timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 256];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = socket.read(&mut buffer).expect("read request");
            assert_ne!(read, 0, "request ended before headers");
            request.extend_from_slice(&buffer[..read]);
        }
    }

    async fn wait_for_cache(root: &Path, key: CacheKey) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if TrackCache::open(root)
                    .expect("open cache")
                    .lookup(key)
                    .expect("lookup cache")
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("completed stream should be cached");
    }

    struct StalledServer {
        url: String,
        accepted: Option<oneshot::Receiver<()>>,
        closed: std_mpsc::Receiver<()>,
        thread: thread::JoinHandle<()>,
    }

    impl StalledServer {
        fn spawn() -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind stalled server");
            let address = listener.local_addr().expect("server address");
            let (accepted_tx, accepted) = oneshot::channel();
            let (closed_tx, closed) = std_mpsc::channel();
            let thread = thread::spawn(move || {
                let (mut socket, _) = listener.accept().expect("accept player request");
                read_request(&mut socket);
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    WAV_BODY.len()
                )
                .expect("write stalled response headers");
                socket
                    .write_all(&WAV_BODY[..16])
                    .expect("write stalled response prefix");
                socket.flush().expect("flush stalled response prefix");
                socket
                    .set_read_timeout(Some(Duration::from_millis(20)))
                    .expect("set socket timeout");
                let _ = accepted_tx.send(());

                let mut buffer = [0_u8; 1024];
                loop {
                    match socket.read(&mut buffer) {
                        Ok(0) => {
                            let _ = closed_tx.send(());
                            break;
                        }
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) => {}
                        Err(_) => {
                            let _ = closed_tx.send(());
                            break;
                        }
                    }
                }
            });
            Self {
                url: format!("http://{address}/audio"),
                accepted: Some(accepted),
                closed,
                thread,
            }
        }

        async fn wait_until_requested(&mut self) {
            let accepted = self.accepted.take().expect("server request awaited once");
            tokio::time::timeout(Duration::from_secs(1), accepted)
                .await
                .expect("player should connect to local server")
                .expect("stalled server should report the connection");
        }

        fn wait_until_closed(&self, timeout: Duration) {
            self.closed
                .recv_timeout(timeout)
                .expect("cancelling the open should close its HTTP connection");
        }

        fn join(self) {
            self.thread.join().expect("join stalled server");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_cancels_a_stalled_decoder() {
        let mut server = StalledServer::spawn();
        let (player, mut events) = spawn(tokio::runtime::Handle::current());
        player.send(PlayerCommand::PlayUrl {
            generation: 1,
            url: server.url.clone(),
            cache: None,
            unm_source: false,
        });
        server.wait_until_requested().await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        player.send(PlayerCommand::Stop);
        server.wait_until_closed(Duration::from_millis(500));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), events.recv())
                .await
                .is_err(),
            "the cancelled track must not emit a playback event"
        );

        drop(player);
        server.join();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_play_replaces_a_stalled_decoder_without_starting_it() {
        let mut stalled = StalledServer::spawn();
        let replacement = HttpServer::complete(WAV_BODY);
        let (player, mut events) =
            spawn_with_engine(tokio::runtime::Handle::current(), open_silent_engine);
        player.send(PlayerCommand::PlayUrl {
            generation: 1,
            url: stalled.url.clone(),
            cache: None,
            unm_source: false,
        });
        stalled.wait_until_requested().await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        player.send(PlayerCommand::PlayUrl {
            generation: 2,
            url: replacement.url.clone(),
            cache: None,
            unm_source: false,
        });
        let event = tokio::time::timeout(
            deadline.saturating_duration_since(std::time::Instant::now()),
            events.recv(),
        )
        .await
        .expect("replacement should start without waiting for the first connection")
        .expect("player event channel should remain open");
        assert!(matches!(event, PlayerEvent::Started { generation: 2, .. }));
        stalled.wait_until_closed(deadline.saturating_duration_since(std::time::Instant::now()));
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(100), events.recv()).await
        {
            assert!(
                !matches!(
                    event,
                    PlayerEvent::Started { generation: 1, .. }
                        | PlayerEvent::Position { generation: 1, .. }
                        | PlayerEvent::Paused { generation: 1, .. }
                        | PlayerEvent::Ended { generation: 1 }
                        | PlayerEvent::Failed { generation: 1, .. }
                ),
                "the replaced track must not emit a playback event"
            );
        }

        drop(player);
        stalled.join();
        replacement.join();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn decoded_track_waits_for_explicit_play_before_reporting_position() {
        let server = HttpServer::complete(WAV_BODY);
        let (player, mut events) =
            spawn_with_engine(tokio::runtime::Handle::current(), open_silent_engine);
        player.send(PlayerCommand::PlayUrl {
            generation: 7,
            url: server.url.clone(),
            cache: None,
            unm_source: false,
        });

        let started = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("decoded track should become ready")
            .expect("player event channel should remain open");
        assert!(matches!(
            started,
            PlayerEvent::Started { generation: 7, .. }
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(350), events.recv())
                .await
                .is_err(),
            "decoded audio must remain paused until the app finishes any restore seek"
        );

        player.send(PlayerCommand::Play);
        let position = tokio::time::timeout(Duration::from_millis(500), events.recv())
            .await
            .expect("explicit play should start position updates")
            .expect("player event channel should remain open");
        assert!(matches!(
            position,
            PlayerEvent::Position { generation: 7, .. }
        ));

        drop(player);
        server.join();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_audio_uses_the_same_generation_stamped_decode_path() {
        let (player, mut events) =
            spawn_with_engine(tokio::runtime::Handle::current(), open_silent_engine);

        player.send(PlayerCommand::PlayBytes {
            generation: 11,
            bytes: WAV_BODY.to_vec(),
            unm_source: true,
        });

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("in-memory audio should decode")
            .expect("player event channel should remain open");
        assert!(matches!(event, PlayerEvent::Started { generation: 11, .. }));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unm_url_open_failure_keeps_its_origin() {
        let (player, mut events) =
            spawn_with_engine(tokio::runtime::Handle::current(), open_silent_engine);

        player.send(PlayerCommand::PlayUrl {
            generation: 12,
            url: "not a url".into(),
            cache: None,
            unm_source: true,
        });

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("invalid UNM URL should fail")
            .expect("player event channel should remain open");
        assert!(matches!(
            event,
            PlayerEvent::Failed {
                generation: 12,
                cached: None,
                unm_source: true,
                ..
            }
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unm_bytes_decode_failure_keeps_its_origin() {
        let (player, mut events) =
            spawn_with_engine(tokio::runtime::Handle::current(), open_silent_engine);

        player.send(PlayerCommand::PlayBytes {
            generation: 13,
            bytes: Vec::new(),
            unm_source: true,
        });

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("invalid UNM bytes should fail")
            .expect("player event channel should remain open");
        assert!(matches!(
            event,
            PlayerEvent::Failed {
                generation: 13,
                cached: None,
                unm_source: true,
                ..
            }
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn naturally_completed_stream_is_published_with_validated_bytes_and_md5() {
        let cache_dir = tempfile::tempdir().expect("cache directory");
        let server = HttpServer::complete(AUDIO_BODY);
        let request = cache_request(31, AUDIO_BODY.len() as u64);
        let key = request.key;
        let media = open_url(
            &server.url,
            Some(CacheWritePlan {
                root: cache_dir.path().to_path_buf(),
                request,
            }),
            false,
        )
        .await
        .expect("open complete stream");

        let downloaded = tokio::task::spawn_blocking(move || {
            let mut reader = media.reader;
            let mut downloaded = Vec::new();
            reader.read_to_end(&mut downloaded).expect("read stream");
            downloaded
        })
        .await
        .expect("join stream reader");
        assert_eq!(downloaded, AUDIO_BODY);
        wait_for_cache(cache_dir.path(), key).await;

        let cache = TrackCache::open(cache_dir.path()).expect("open published cache");
        let mut lease = cache
            .lookup(key)
            .expect("lookup published cache")
            .expect("published cache entry");
        assert_eq!(lease.metadata().bytes, AUDIO_BODY.len() as u64);
        let mut cached = Vec::new();
        lease.read_to_end(&mut cached).expect("read cached bytes");
        assert_eq!(cached, AUDIO_BODY);
        server.join();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stopping_a_partial_stream_does_not_publish_it() {
        let cache_dir = tempfile::tempdir().expect("cache directory");
        let (server, prefix_sent, closed) = HttpServer::stalled_prefix(b"part", AUDIO_BODY.len());
        let request = cache_request(32, AUDIO_BODY.len() as u64);
        let key = request.key;
        let media = open_url(
            &server.url,
            Some(CacheWritePlan {
                root: cache_dir.path().to_path_buf(),
                request,
            }),
            false,
        )
        .await
        .expect("open partial stream");
        tokio::time::timeout(Duration::from_secs(1), prefix_sent)
            .await
            .expect("server should send a prefix")
            .expect("prefix signal should arrive");

        drop(media);
        closed
            .recv_timeout(Duration::from_millis(500))
            .expect("stopping the stream should close the response");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let cache = TrackCache::open(cache_dir.path()).expect("open cache");
        assert!(cache.lookup(key).expect("lookup cache").is_none());
        server.join();
    }

    #[test]
    fn cached_media_keeps_its_lease_until_the_decoder_source_is_dropped() {
        let cache_dir = tempfile::tempdir().expect("cache directory");
        let key = CacheKey::new(33, AudioQuality::High320);
        let cache = TrackCache::open(cache_dir.path()).expect("open cache");
        let mut writer = cache
            .begin_write(
                CacheWriteRequest::new(key, AudioCodec::Mp3, 320_000)
                    .with_expected_bytes(AUDIO_BODY.len() as u64),
            )
            .expect("begin cache write");
        writer.write_all(AUDIO_BODY).expect("write cache entry");
        writer.finish().expect("finish cache entry");
        let lease = cache
            .lookup(key)
            .expect("lookup cache")
            .expect("cache lease");
        let media = open_cached(lease);

        cache.set_max_bytes(0).expect("evict leased cache");
        assert_eq!(cache.total_bytes().expect("leased cache size"), 19);

        drop(media);
        cache.set_max_bytes(0).expect("evict released cache");
        assert_eq!(cache.total_bytes().expect("released cache size"), 0);
    }

    #[test]
    fn cached_decoder_failure_reports_the_entry_that_must_be_invalidated() {
        let cache_dir = tempfile::tempdir().expect("cache directory");
        let key = CacheKey::new(34, AudioQuality::High320);
        let cache = TrackCache::open(cache_dir.path()).expect("open cache");
        let mut writer = cache
            .begin_write(CacheWriteRequest::new(key, AudioCodec::Mp3, 320_000))
            .expect("begin cache write");
        writer.write_all(b"not audio").expect("write cache entry");
        let metadata = writer.finish().expect("finish cache entry");
        let lease = cache
            .lookup(key)
            .expect("lookup cache")
            .expect("cache lease");
        let (events, mut received) = mpsc::unbounded_channel();
        let mut engine = None;

        assert!(!start(&mut engine, 1.0, 8, &events, || {
            Ok(open_cached(lease))
        }));
        let event = received.try_recv().expect("decoder failure event");
        assert!(matches!(
            event,
            PlayerEvent::Failed {
                generation: 8,
                cached: Some(failed),
                ..
            } if failed == metadata
        ));
    }
}
