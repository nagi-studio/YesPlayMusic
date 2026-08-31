//! Client half of remote.rs: find a running player and send it one command.
//!
//! The TUI answers everything over its control socket. The GUI splits the
//! job: commands go to its control socket (src-tauri/src/main.rs), status
//! comes from the sidecar's legacy player endpoint.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::remote::{self, Command, Snapshot};

const GUI_SOCKET: &str = "/tmp/com_electron_yesplaymusic_ctl.sock";
const GUI_PLAYER_URL: &str = "http://127.0.0.1:27232/player";
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Gui,
    Tui,
}

impl Target {
    fn label(self) -> &'static str {
        match self {
            Target::Gui => "GUI",
            Target::Tui => "TUI",
        }
    }
}

pub async fn run(command: Command, forced: Option<Target>, json: bool) -> Result<()> {
    let gui = probe(status_gui()).await;
    let tui = probe(status_tui()).await;
    let target = resolve(forced, &gui, &tui)?;
    let snapshot = match target {
        Target::Gui => gui.as_ref().expect("resolve only returns live targets"),
        Target::Tui => tui.as_ref().expect("resolve only returns live targets"),
    };

    if command == Command::Status {
        report_status(target, snapshot, json);
        return Ok(());
    }
    ensure_command_supported(command, target, snapshot)?;

    match target {
        Target::Gui => {
            request(GUI_SOCKET, command)
                .await
                .context("GUI 播放器未响应控制命令")?;
        }
        Target::Tui => {
            request(&remote::socket_path().to_string_lossy(), command)
                .await
                .context("TUI 播放器未响应控制命令")?;
        }
    }
    if json {
        println!(
            "{}",
            serde_json::json!({ "ok": true, "source": target.label().to_lowercase() })
        );
    } else {
        let done = match command {
            Command::Pause => "已暂停".to_owned(),
            Command::Resume => "已继续播放".to_owned(),
            Command::Toggle => "已切换播放/暂停".to_owned(),
            Command::Next => "已切到下一首".to_owned(),
            Command::Prev => "已切到上一首".to_owned(),
            Command::Seek { position_ms } => format!("已跳转到 {}", clock(position_ms)),
            Command::Status => unreachable!(),
        };
        println!("{done}（{}）", target.label());
    }
    Ok(())
}

fn ensure_command_supported(command: Command, target: Target, snapshot: &Snapshot) -> Result<()> {
    if matches!(command, Command::Seek { .. }) && !snapshot.seekable {
        bail!("{} 当前曲目不能跳转", target.label());
    }
    Ok(())
}

async fn probe(status: impl std::future::Future<Output = Result<Snapshot>>) -> Option<Snapshot> {
    tokio::time::timeout(PROBE_TIMEOUT, status).await.ok()?.ok()
}

fn resolve(
    forced: Option<Target>,
    gui: &Option<Snapshot>,
    tui: &Option<Snapshot>,
) -> Result<Target> {
    if let Some(target) = forced {
        let alive = match target {
            Target::Gui => gui.is_some(),
            Target::Tui => tui.is_some(),
        };
        if !alive {
            bail!("{} 播放器没有在运行", target.label());
        }
        return Ok(target);
    }
    let gui_playing = gui.as_ref().is_some_and(|s| s.playing);
    let tui_playing = tui.as_ref().is_some_and(|s| s.playing);
    match (gui_playing, tui_playing) {
        (true, true) => bail!("GUI 和 TUI 都在播放，请用 --gui 或 --tui 指定"),
        (true, false) => Ok(Target::Gui),
        (false, true) => Ok(Target::Tui),
        (false, false) => match (gui.is_some(), tui.is_some()) {
            (true, true) => bail!("GUI 和 TUI 都在运行且都未播放，请用 --gui 或 --tui 指定"),
            (true, false) => Ok(Target::Gui),
            (false, true) => Ok(Target::Tui),
            (false, false) => bail!("没有运行中的播放器（GUI 或 TUI 都不在线）"),
        },
    }
}

fn report_status(target: Target, snapshot: &Snapshot, json: bool) {
    if json {
        let mut value = serde_json::to_value(snapshot).expect("snapshot serializes");
        value["source"] = Value::from(target.label().to_lowercase());
        println!("{value}");
        return;
    }
    let Some(title) = &snapshot.title else {
        println!("没有正在播放的曲目（{}）", target.label());
        return;
    };
    let mark = if snapshot.playing { "▶" } else { "⏸" };
    let artist = snapshot.artist.as_deref().unwrap_or("未知歌手");
    let position = clock(snapshot.position_ms);
    match snapshot.duration_ms {
        Some(duration) => println!(
            "{mark} {title} — {artist} [{position}/{}]（{}）",
            clock(duration),
            target.label()
        ),
        None => println!(
            "{mark} {title} — {artist} [{position}]（{}）",
            target.label()
        ),
    }
}

fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

async fn request(path: &str, command: Command) -> Result<Value> {
    let stream = UnixStream::connect(path).await?;
    let (reader, mut writer) = stream.into_split();
    let mut payload = serde_json::to_string(&command)?;
    payload.push('\n');
    writer.write_all(payload.as_bytes()).await?;
    let mut reply = String::new();
    BufReader::new(reader).read_line(&mut reply).await?;
    let value: Value = serde_json::from_str(reply.trim())?;
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        bail!("播放器拒绝了命令：{reply}");
    }
    Ok(value)
}

async fn status_tui() -> Result<Snapshot> {
    let value = request(&remote::socket_path().to_string_lossy(), Command::Status).await?;
    Ok(serde_json::from_value(value)?)
}

async fn status_gui() -> Result<Snapshot> {
    let response = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()?
        .get(GUI_PLAYER_URL)
        .send()
        .await?
        .error_for_status()?;
    let player: Value = response.json().await?;
    Ok(snapshot_from_gui(&player))
}

/// Map the sidecar's `/player` payload onto the shared snapshot shape.
/// Personal-FM tracks use `artists`/`album`/`duration` where playlist
/// tracks use `ar`/`al`/`dt`.
fn snapshot_from_gui(player: &Value) -> Snapshot {
    let track = player.get("currentTrack").and_then(Value::as_object);
    let field = |names: [&str; 2]| track.and_then(|t| names.iter().find_map(|name| t.get(*name)));
    let artist = field(["ar", "artists"])
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .filter(|joined| !joined.is_empty());
    let album = field(["al", "album"]);
    let duration_ms = field(["dt", "duration"]).and_then(Value::as_u64);
    Snapshot {
        playing: player
            .get("playing")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        title: track
            .and_then(|t| t.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        artist,
        album: album
            .and_then(|album| album.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        cover_url: album
            .and_then(|album| album.get("picUrl"))
            .and_then(Value::as_str)
            .and_then(remote::status_cover_url),
        seekable: track.is_some() && duration_ms.is_some(),
        icon_style: crate::config::IconStyle::Unicode,
        position_ms: (player
            .get("progress")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            * 1000.0) as u64,
        duration_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_requires_the_selected_player_to_advertise_readiness() {
        let seek = Command::Seek {
            position_ms: 30_000,
        };
        let mut snapshot = Snapshot::default();
        assert!(ensure_command_supported(seek, Target::Gui, &snapshot).is_err());
        assert!(ensure_command_supported(seek, Target::Tui, &snapshot).is_err());

        snapshot.seekable = true;
        assert!(ensure_command_supported(seek, Target::Gui, &snapshot).is_ok());
        assert!(ensure_command_supported(seek, Target::Tui, &snapshot).is_ok());
    }

    #[test]
    fn gui_playlist_track_maps_to_snapshot() {
        let player = serde_json::json!({
            "currentTrack": {
                "name": "海阔天空",
                "ar": [{ "name": "Beyond" }],
                "al": {
                    "name": "乐与怒",
                    "picUrl": "http://p1.music.126.net/cover.jpg?token=secret"
                },
                "dt": 326000
            },
            "progress": 12.5,
            "playing": true
        });
        let snapshot = snapshot_from_gui(&player);
        assert_eq!(snapshot.title.as_deref(), Some("海阔天空"));
        assert_eq!(snapshot.artist.as_deref(), Some("Beyond"));
        assert_eq!(snapshot.album.as_deref(), Some("乐与怒"));
        assert_eq!(
            snapshot.cover_url.as_deref(),
            Some("https://p1.music.126.net/cover.jpg?param=64y64")
        );
        assert_eq!(snapshot.position_ms, 12500);
        assert_eq!(snapshot.duration_ms, Some(326000));
        assert!(snapshot.playing);
    }

    #[test]
    fn gui_personal_fm_cover_maps_to_the_same_snapshot_shape() {
        let player = serde_json::json!({
            "currentTrack": {
                "name": "夜曲",
                "artists": [{ "name": "周杰伦" }],
                "album": {
                    "name": "十一月的萧邦",
                    "picUrl": "https://p2.music.126.net/fm.jpg?param=512y512"
                },
                "duration": 226000
            },
            "progress": 8.25,
            "playing": false
        });

        let snapshot = snapshot_from_gui(&player);
        assert_eq!(snapshot.title.as_deref(), Some("夜曲"));
        assert_eq!(snapshot.artist.as_deref(), Some("周杰伦"));
        assert_eq!(snapshot.album.as_deref(), Some("十一月的萧邦"));
        assert_eq!(
            snapshot.cover_url.as_deref(),
            Some("https://p2.music.126.net/fm.jpg?param=64y64")
        );
        assert_eq!(snapshot.position_ms, 8250);
        assert_eq!(snapshot.duration_ms, Some(226000));
        assert!(!snapshot.playing);
    }

    #[test]
    fn gui_idle_player_maps_to_empty_snapshot() {
        let player = serde_json::json!({ "currentTrack": null, "progress": 0.0 });
        let snapshot = snapshot_from_gui(&player);
        assert_eq!(snapshot, Snapshot::default());
    }

    #[test]
    fn resolve_prefers_the_playing_instance() {
        let playing = Some(Snapshot {
            playing: true,
            ..Snapshot::default()
        });
        let idle = Some(Snapshot::default());
        assert_eq!(resolve(None, &idle, &playing).unwrap(), Target::Tui);
        assert_eq!(resolve(None, &playing, &idle).unwrap(), Target::Gui);
        assert_eq!(resolve(None, &idle, &None).unwrap(), Target::Gui);
        assert!(resolve(None, &None, &None).is_err());
        assert!(resolve(None, &idle, &idle.clone()).is_err());
        assert!(resolve(Some(Target::Tui), &idle, &None).is_err());
    }
}
