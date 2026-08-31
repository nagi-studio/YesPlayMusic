//! Remote control socket: `ypm pause` etc. drive a running player.
//!
//! JSON lines over a Unix socket. The TUI serves the full protocol here;
//! the GUI serves the command subset inline in src-tauri/src/main.rs
//! (keep the wire words in sync). The client half lives in ctl.rs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};

use crate::action::Action;

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
/// If another live TUI already listens, that one keeps the socket.
pub fn bind(path: &PathBuf) -> std::io::Result<Option<UnixListener>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match UnixListener::bind(path) {
        Ok(listener) => Ok(Some(listener)),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Ok(None);
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
) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let actions = actions.clone();
        let snapshots = snapshots.clone();
        tokio::spawn(async move {
            let _ = handle(stream, actions, snapshots).await;
        });
    }
}

async fn handle(
    stream: UnixStream,
    actions: mpsc::UnboundedSender<Action>,
    snapshots: watch::Receiver<Snapshot>,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut line = String::new();
    BufReader::new(reader).read_line(&mut line).await?;
    let reply = match serde_json::from_str::<Command>(&line) {
        Ok(command) => {
            let snapshot = snapshots.borrow().clone();
            match command {
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
                        Command::Status | Command::Seek { .. } => unreachable!(),
                    };
                    if let Some(action) = action {
                        let _ = actions.send(action);
                    }
                    r#"{"ok":true}"#.to_owned()
                }
            }
        }
        Err(error) => format!(r#"{{"ok":false,"error":"{error}"}}"#),
    };
    writer.write_all(reply.as_bytes()).await?;
    writer.write_all(b"\n").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_use_stable_wire_words() {
        // The GUI parses these words independently; renaming breaks it.
        for (command, wire) in [
            (Command::Status, r#"{"cmd":"status"}"#),
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
    async fn pause_only_fires_while_playing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ctl.sock");
        let listener = bind(&path).unwrap().unwrap();
        let (actions, mut received) = mpsc::unbounded_channel();
        let (publish, snapshots) = watch::channel(Snapshot {
            playing: true,
            ..Snapshot::default()
        });
        tokio::spawn(serve(listener, actions, snapshots));

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
}
