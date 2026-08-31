//! Remote control socket: `ypm pause` etc. drive a running player.
//!
//! JSON lines over a Unix socket. The TUI serves the full protocol here;
//! the GUI serves the command subset inline in src-tauri/src/main.rs
//! (keep the wire words in sync). The client half lives in ctl.rs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, watch, Notify};
use tokio::time::{sleep_until, Duration, Instant};

use crate::action::Action;
use crate::spectrum::{REMOTE_SPECTRUM_BINS, REMOTE_SPECTRUM_MAX_FPS};

/// Now-playing state published by the app loop for `status` replies.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub playing: bool,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    #[serde(default)]
    pub cover_url: Option<String>,
    #[serde(default)]
    pub seekable: bool,
    #[serde(default)]
    pub icon_style: crate::config::IconStyle,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
}

pub const SPECTRUM_PROTOCOL_VERSION: u8 = 1;
#[cfg(not(test))]
const SPECTRUM_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const SPECTRUM_WRITE_TIMEOUT: Duration = Duration::from_millis(50);

/// Bounded, versioned analyzer projection for public NDJSON consumers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpectrumFrame {
    pub version: u8,
    pub style: crate::spectrum::SpectrumKind,
    pub playing: bool,
    pub bins: [u8; REMOTE_SPECTRUM_BINS],
}

impl Default for SpectrumFrame {
    fn default() -> Self {
        Self {
            version: SPECTRUM_PROTOCOL_VERSION,
            style: crate::spectrum::SpectrumKind::default(),
            playing: false,
            bins: [0; REMOTE_SPECTRUM_BINS],
        }
    }
}

/// The app only pays the FFT cost while at least one public stream is alive.
#[derive(Clone, Default)]
pub struct SpectrumSubscribers(Arc<SpectrumSubscriberState>);

#[derive(Default)]
struct SpectrumSubscriberState {
    count: AtomicUsize,
    changed: Notify,
}

impl SpectrumSubscribers {
    pub fn is_active(&self) -> bool {
        self.0.count.load(Ordering::Acquire) > 0
    }

    /// Wake the app loop when the active state changes in either direction.
    pub async fn changed(&self) {
        self.0.changed.notified().await;
    }

    fn subscribe(&self) -> SpectrumSubscription {
        if self.0.count.fetch_add(1, Ordering::AcqRel) == 0 {
            self.0.changed.notify_one();
        }
        SpectrumSubscription(self.clone())
    }

    fn unsubscribe(&self) {
        if self.0.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.changed.notify_one();
        }
    }
}

struct SpectrumSubscription(SpectrumSubscribers);

impl Drop for SpectrumSubscription {
    fn drop(&mut self) {
        self.0.unsubscribe();
    }
}

const STATUS_COVER_EDGE: u16 = 64;

/// Keep status artwork on the official CDN, small, and free of upstream query
/// credentials before it crosses the public CLI boundary.
pub(crate) fn status_cover_url(raw: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(raw.trim()).ok()?;
    let default_port = match url.scheme() {
        "https" => 443,
        "http" => 80,
        _ => return None,
    };
    if url.port().is_some_and(|port| port != default_port) {
        return None;
    }
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    let host = url.host_str()?;
    let official_host = host == "music.126.net" || host.ends_with(".music.126.net");
    if !official_host || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    url.query_pairs_mut()
        .append_pair("param", &format!("{STATUS_COVER_EDGE}y{STATUS_COVER_EDGE}"));
    Some(url.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum Command {
    Status,
    Spectrum { fps: u8 },
    Pause,
    Resume,
    Toggle,
    Next,
    Prev,
    Seek { position_ms: u64 },
}

pub fn socket_path() -> PathBuf {
    crate::config::state_dir().join("ctl.sock")
}

/// Bind the control socket, replacing a stale file left by a dead process.
/// A live owner rejects a second TUI so remote control cannot target the wrong one.
pub fn bind(path: &PathBuf) -> std::io::Result<Option<UnixListener>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match UnixListener::bind(path) {
        Ok(listener) => Ok(Some(listener)),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(error);
            }
            std::fs::remove_file(path)?;
            Ok(Some(UnixListener::bind(path)?))
        }
        Err(error) => Err(error),
    }
}

pub async fn serve(
    listener: UnixListener,
    actions: mpsc::UnboundedSender<Action>,
    snapshots: watch::Receiver<Snapshot>,
    spectrum_frames: broadcast::Sender<SpectrumFrame>,
    spectrum_subscribers: SpectrumSubscribers,
) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let actions = actions.clone();
        let snapshots = snapshots.clone();
        let spectrum_frames = spectrum_frames.clone();
        let spectrum_subscribers = spectrum_subscribers.clone();
        tokio::spawn(async move {
            let _ = handle(
                stream,
                actions,
                snapshots,
                spectrum_frames,
                spectrum_subscribers,
            )
            .await;
        });
    }
}

async fn handle(
    stream: UnixStream,
    actions: mpsc::UnboundedSender<Action>,
    snapshots: watch::Receiver<Snapshot>,
    spectrum_frames: broadcast::Sender<SpectrumFrame>,
    spectrum_subscribers: SpectrumSubscribers,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let command = match serde_json::from_str::<Command>(&line) {
        Ok(command) => command,
        Err(error) => {
            let reply = serde_json::json!({ "ok": false, "error": error.to_string() });
            writer.write_all(reply.to_string().as_bytes()).await?;
            return writer.write_all(b"\n").await;
        }
    };
    if let Command::Spectrum { fps } = command {
        if !(1..=REMOTE_SPECTRUM_MAX_FPS).contains(&fps) {
            writer
                .write_all(b"{\"ok\":false,\"error\":\"spectrum fps must be between 1 and 20\"}\n")
                .await?;
            return Ok(());
        }
        let subscription = spectrum_subscribers.subscribe();
        let frames = spectrum_frames.subscribe();
        return stream_spectrum(&mut reader, &mut writer, frames, subscription, fps).await;
    }

    let snapshot = snapshots.borrow().clone();
    let reply = match command {
        Command::Status => serde_json::to_string(&snapshot).expect("snapshot serializes"),
        Command::Seek { position_ms } => match snapshot.duration_ms {
            Some(duration_ms) if snapshot.seekable && duration_ms > 0 => {
                let ratio = position_ms as f64 / duration_ms as f64;
                let _ = actions.send(Action::SeekToRatio(ratio.clamp(0.0, 1.0)));
                r#"{"ok":true}"#.to_owned()
            }
            _ => r#"{"ok":false,"error":"current track is not seekable"}"#.to_owned(),
        },
        other => {
            let action = match other {
                Command::Toggle => Some(Action::TogglePlay),
                Command::Pause => snapshot.playing.then_some(Action::TogglePlay),
                Command::Resume => (!snapshot.playing).then_some(Action::TogglePlay),
                Command::Next => Some(Action::NextTrack),
                Command::Prev => Some(Action::PrevTrack),
                Command::Status | Command::Spectrum { .. } | Command::Seek { .. } => {
                    unreachable!()
                }
            };
            if let Some(action) = action {
                let _ = actions.send(action);
            }
            r#"{"ok":true}"#.to_owned()
        }
    };
    writer.write_all(reply.as_bytes()).await?;
    writer.write_all(b"\n").await
}

async fn stream_spectrum<R, W>(
    reader: &mut R,
    writer: &mut W,
    mut frames: broadcast::Receiver<SpectrumFrame>,
    _subscription: SpectrumSubscription,
    fps: u8,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
    let mut next_send = Instant::now();
    loop {
        let mut frame = loop {
            let received = tokio::select! {
                received = frames.recv() => received,
                disconnected = reader.read_u8() => {
                    let _ = disconnected;
                    return Ok(());
                }
            };
            match received {
                Ok(frame) => break frame,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            }
        };

        let now = Instant::now();
        if now < next_send {
            tokio::select! {
                _ = sleep_until(next_send) => {}
                disconnected = reader.read_u8() => {
                    let _ = disconnected;
                    return Ok(());
                }
            }
        }
        loop {
            match frames.try_recv() {
                Ok(latest) => frame = latest,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => return Ok(()),
            }
        }
        let mut encoded = serde_json::to_vec(&frame).expect("spectrum frame serializes");
        encoded.push(b'\n');
        tokio::select! {
            result = writer.write_all(&encoded) => result?,
            disconnected = reader.read_u8() => {
                let _ = disconnected;
                return Ok(());
            }
            _ = tokio::time::sleep(SPECTRUM_WRITE_TIMEOUT) => return Ok(()),
        }
        next_send = Instant::now() + interval;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_use_stable_wire_words() {
        // The GUI parses these words independently; renaming breaks it.
        for (command, wire) in [
            (Command::Status, r#"{"cmd":"status"}"#),
            (
                Command::Spectrum { fps: 12 },
                r#"{"cmd":"spectrum","fps":12}"#,
            ),
            (Command::Pause, r#"{"cmd":"pause"}"#),
            (Command::Resume, r#"{"cmd":"resume"}"#),
            (Command::Toggle, r#"{"cmd":"toggle"}"#),
            (Command::Next, r#"{"cmd":"next"}"#),
            (Command::Prev, r#"{"cmd":"prev"}"#),
            (
                Command::Seek {
                    position_ms: 90_500,
                },
                r#"{"cmd":"seek","positionMs":90500}"#,
            ),
        ] {
            assert_eq!(serde_json::to_string(&command).unwrap(), wire);
        }
    }

    #[test]
    fn snapshot_cover_url_is_additive_and_camel_case() {
        let legacy = r#"{
            "playing":true,
            "title":"Track",
            "artist":"Artist",
            "album":"Album",
            "positionMs":1200,
            "durationMs":180000
        }"#;
        let snapshot: Snapshot = serde_json::from_str(legacy).unwrap();
        assert_eq!(snapshot.cover_url, None);
        assert!(!snapshot.seekable);
        assert_eq!(snapshot.icon_style, crate::config::IconStyle::Unicode);

        let value = serde_json::to_value(Snapshot {
            cover_url: Some("https://p1.music.126.net/cover.jpg?param=64y64".into()),
            seekable: true,
            icon_style: crate::config::IconStyle::Nerd,
            ..Snapshot::default()
        })
        .unwrap();
        assert_eq!(
            value["coverUrl"],
            "https://p1.music.126.net/cover.jpg?param=64y64"
        );
        assert!(value.get("cover_url").is_none());
        assert_eq!(value["seekable"], true);
        assert_eq!(value["iconStyle"], "nerd");
    }

    #[test]
    fn status_cover_is_https_small_and_credential_free() {
        assert_eq!(
            status_cover_url(
                "http://p1.music.126.net/cover.jpg?token=secret&param=1024y1024#private"
            )
            .as_deref(),
            Some("https://p1.music.126.net/cover.jpg?param=64y64")
        );
        assert_eq!(
            status_cover_url("https://p2.music.126.net:443/cover.jpg").as_deref(),
            Some("https://p2.music.126.net/cover.jpg?param=64y64")
        );

        for raw in [
            "",
            "not a URL",
            "file:///tmp/cover.jpg",
            "data:image/png;base64,secret",
            "https://example.test/cover.jpg",
            "https://music.126.net.evil.test/cover.jpg",
            "https://p1.music.126.net:8443/cover.jpg",
            "https://user:password@p1.music.126.net/cover.jpg",
        ] {
            assert_eq!(status_cover_url(raw), None, "{raw}");
        }
    }

    #[tokio::test]
    async fn a_live_control_socket_rejects_a_second_tui_but_stale_files_recover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ctl.sock");
        let listener = bind(&path).unwrap().expect("first TUI owns the socket");

        let error = bind(&path).expect_err("a live owner must reject the second TUI");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);

        drop(listener);
        assert!(bind(&path).unwrap().is_some());
    }

    #[tokio::test]
    async fn pause_only_fires_while_playing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ctl.sock");
        let listener = bind(&path).unwrap().unwrap();
        let (actions, mut received) = mpsc::unbounded_channel();
        let (publish, snapshots) = watch::channel(Snapshot {
            playing: true,
            ..Snapshot::default()
        });
        let (spectrum_publish, _) = broadcast::channel(4);
        let spectrum_subscribers = SpectrumSubscribers::default();
        tokio::spawn(serve(
            listener,
            actions,
            snapshots,
            spectrum_publish,
            spectrum_subscribers,
        ));

        let roundtrip = |cmd: Command| {
            let path = path.clone();
            async move {
                let mut stream = UnixStream::connect(&path).await.unwrap();
                let mut payload = serde_json::to_string(&cmd).unwrap();
                payload.push('\n');
                stream.write_all(payload.as_bytes()).await.unwrap();
                let mut reply = String::new();
                BufReader::new(stream).read_line(&mut reply).await.unwrap();
                reply
            }
        };

        assert_eq!(roundtrip(Command::Pause).await, "{\"ok\":true}\n");
        assert!(matches!(received.recv().await, Some(Action::TogglePlay)));

        publish.send_replace(Snapshot::default());
        assert_eq!(roundtrip(Command::Pause).await, "{\"ok\":true}\n");
        assert_eq!(roundtrip(Command::Next).await, "{\"ok\":true}\n");
        // Pause while paused sent nothing; Next arrived instead.
        assert!(matches!(received.recv().await, Some(Action::NextTrack)));

        publish.send_replace(Snapshot {
            duration_ms: Some(200_000),
            seekable: false,
            ..Snapshot::default()
        });
        assert_eq!(
            roundtrip(Command::Seek {
                position_ms: 50_000,
            })
            .await,
            "{\"ok\":false,\"error\":\"current track is not seekable\"}\n"
        );
        assert_eq!(roundtrip(Command::Next).await, "{\"ok\":true}\n");
        assert!(matches!(received.recv().await, Some(Action::NextTrack)));

        publish.send_replace(Snapshot {
            duration_ms: Some(200_000),
            seekable: true,
            ..Snapshot::default()
        });
        assert_eq!(
            roundtrip(Command::Seek {
                position_ms: 50_000,
            })
            .await,
            "{\"ok\":true}\n"
        );
        assert!(matches!(
            received.recv().await,
            Some(Action::SeekToRatio(ratio)) if (ratio - 0.25).abs() < f64::EPSILON
        ));

        let status: Snapshot =
            serde_json::from_str(roundtrip(Command::Status).await.trim()).unwrap();
        assert!(!status.playing);
    }

    #[tokio::test]
    async fn spectrum_stream_projects_bounded_frames_and_releases_its_subscription() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ctl.sock");
        let listener = bind(&path).unwrap().unwrap();
        let (actions, _) = mpsc::unbounded_channel();
        let (_, snapshots) = watch::channel(Snapshot::default());
        let (publish, _) = broadcast::channel(4);
        let subscribers = SpectrumSubscribers::default();
        tokio::spawn(serve(
            listener,
            actions,
            snapshots,
            publish.clone(),
            subscribers.clone(),
        ));

        let mut stream = UnixStream::connect(&path).await.unwrap();
        stream
            .write_all(b"{\"cmd\":\"spectrum\",\"fps\":20}\n")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_millis(250), async {
            while !subscribers.is_active() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(subscribers.is_active());

        let mut expected = SpectrumFrame {
            style: crate::spectrum::SpectrumKind::Braille,
            playing: true,
            ..SpectrumFrame::default()
        };
        expected.bins[0] = 255;
        expected.bins[31] = 128;
        publish.send(expected.clone()).unwrap();
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_millis(250),
            BufReader::new(&mut stream).read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            line.len() < 512,
            "public frame grew unexpectedly: {}",
            line.len()
        );
        assert_eq!(
            serde_json::from_str::<SpectrumFrame>(line.trim()).unwrap(),
            expected
        );

        drop(stream);
        tokio::time::timeout(Duration::from_millis(250), async {
            while subscribers.is_active() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!subscribers.is_active());
    }

    #[tokio::test]
    async fn spectrum_subscription_transitions_are_wakeable() {
        let subscribers = SpectrumSubscribers::default();
        let waiting = subscribers.clone();
        let wake_on_subscribe = tokio::spawn(async move {
            waiting.changed().await;
            waiting.is_active()
        });

        let subscription = subscribers.subscribe();
        assert!(
            tokio::time::timeout(Duration::from_millis(250), wake_on_subscribe)
                .await
                .unwrap()
                .unwrap()
        );

        let waiting = subscribers.clone();
        let wake_on_unsubscribe = tokio::spawn(async move {
            waiting.changed().await;
            waiting.is_active()
        });
        drop(subscription);
        assert!(
            !tokio::time::timeout(Duration::from_millis(250), wake_on_unsubscribe)
                .await
                .unwrap()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn stalled_spectrum_consumers_release_the_analyzer() {
        let subscribers = SpectrumSubscribers::default();
        let subscription = subscribers.subscribe();
        let (publish, frames) = broadcast::channel(1);
        let (server, _client) = tokio::io::duplex(1);
        let (mut reader, mut writer) = tokio::io::split(server);
        let task = tokio::spawn(async move {
            stream_spectrum(
                &mut reader,
                &mut writer,
                frames,
                subscription,
                REMOTE_SPECTRUM_MAX_FPS,
            )
            .await
        });
        publish.send(SpectrumFrame::default()).unwrap();

        tokio::time::timeout(Duration::from_millis(250), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!subscribers.is_active());
    }
}
