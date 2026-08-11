#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod desktop_preferences;
mod discord_presence;
mod legacy_renderer_data;
#[cfg(target_os = "linux")]
mod linux_media;
#[cfg(target_os = "macos")]
mod macos_media_controls;
mod window_preferences;

use std::{
    collections::HashMap,
    env, fs,
    io::{Cursor, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use desktop_preferences::{
    close_decision, load_webview_proxy, migrate_legacy_webview_proxy, parse_close_choice,
    parse_desktop_preferences, parse_player_state_from_media_state, remove_webview_proxy,
    save_webview_proxy, tray_icon_asset, tray_menu_text, CloseAppOption, CloseChoiceAction,
    CloseDecision, DesktopPreferences, PlayerStatePayload, TrayIconTheme,
};
use discord_presence::{DiscordPresenceHandle, DiscordPresencePayload};
#[cfg(target_os = "linux")]
use linux_media::{LinuxMedia, MediaControl, MediaMetadata, MediaState, RepeatMode};
use window_preferences::{
    load as load_window_preferences, load_legacy_preferences, read_legacy_electron_config,
    save as save_window_preferences, with_legacy_fallback, WindowPosition, WindowPreferences,
    WindowSize,
};

use tauri::{
    image::Image as TauriImage,
    menu::{AboutMetadata, AboutMetadataBuilder, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::PageLoadEvent,
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, RunEvent, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_shell::{
    process::{CommandChild, CommandEvent},
    ShellExt,
};

#[cfg(target_os = "macos")]
use objc2_app_kit::{NSWindow, NSWindowButton, NSWindowCollectionBehavior};
#[cfg(target_os = "macos")]
use std::{
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::Path,
};

const API_PORT: u16 = 12_754;
const DEV_WEB_PORT: u16 = 1_420;
const RELEASE_WEB_PORT: u16 = 28_232;
const WEBVIEW_PROXY_RELAY_PORT: u16 = 27_233;
const SIDECAR_HEALTH_PATH: &str = "/__yesplaymusic/health";
const SIDECAR_HEALTH_BODY: &str = r#"{"service":"yesplaymusic-sidecar","protocol":1}"#;
const SIDECAR_HEALTH_TOKEN_HEADER: &str = "X-YesPlayMusic-Health-Token";
const WINDOW_MOVE_SETTLE_TIME: Duration = Duration::from_millis(250);
const STARTUP_SHOW_TIMEOUT: Duration = Duration::from_secs(5);
const SIDECAR_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(7);
const SIDECAR_FORCED_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const SIDECAR_SHUTDOWN_SIGNAL: &[u8] = &[0];
#[cfg(target_os = "macos")]
const SINGLE_INSTANCE_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(target_os = "macos")]
struct MacosStartupGate {
    file: fs::File,
}

#[cfg(target_os = "macos")]
impl Drop for MacosStartupGate {
    fn drop(&mut self) {
        // SAFETY: the descriptor remains owned by self.file for the duration of this call.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(target_os = "macos")]
fn single_instance_identifier(identifier: &str) -> String {
    identifier.replace(['.', '-'], "_")
}

#[cfg(target_os = "macos")]
fn single_instance_socket_path(identifier: &str) -> PathBuf {
    PathBuf::from(format!(
        "/tmp/{}_si.sock",
        single_instance_identifier(identifier)
    ))
}

#[cfg(target_os = "macos")]
fn startup_gate_path(identifier: &str) -> PathBuf {
    env::temp_dir().join(format!(
        "{}_startup.lock",
        single_instance_identifier(identifier)
    ))
}

#[cfg(target_os = "macos")]
fn acquire_macos_startup_gate(path: &Path, timeout: Duration) -> Result<MacosStartupGate, String> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("无法打开 single-instance 启动锁：{error}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: flock only borrows this valid descriptor and does not outlive file.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(MacosStartupGate { file });
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(format!("无法取得 single-instance 启动锁：{error}"));
        }
        if Instant::now() >= deadline {
            return Err("等待现有 YesPlayMusic 实例完成冷启动超时".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "macos")]
fn wait_for_single_instance_listener(path: &Path, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match UnixStream::connect(path) {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) => {}
            Err(error) => return Err(format!("single-instance listener 检查失败：{error}")),
        }
        if Instant::now() >= deadline {
            return Err("single-instance listener 未在启动锁释放前就绪".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn updater_public_key() -> Option<&'static str> {
    option_env!("TAURI_UPDATER_PUBKEY").filter(|key| !key.trim().is_empty())
}

fn updater_target_for(
    os: &str,
    arch: &str,
    bundle_type: Option<tauri::utils::config::BundleType>,
) -> &'static str {
    use tauri::utils::config::BundleType;

    match (os, arch, bundle_type) {
        ("macos", "aarch64", _) => "darwin-aarch64",
        ("windows", "x86_64", _) => "windows-x86_64",
        ("linux", "x86_64", Some(BundleType::AppImage)) => "linux-x86_64-appimage",
        ("linux", "x86_64", Some(BundleType::Deb)) => "linux-x86_64-deb",
        _ => "unsupported",
    }
}

fn updater_target() -> &'static str {
    updater_target_for(
        std::env::consts::OS,
        std::env::consts::ARCH,
        tauri::utils::platform::bundle_type(),
    )
}

#[cfg(test)]
mod updater_target_tests {
    use super::{
        single_instance_notification_is_probe, updater_requires_explicit_sidecar_shutdown,
        updater_target_for,
    };
    use tauri::utils::config::BundleType;

    #[test]
    fn linux_bundle_types_use_distinct_manifest_targets() {
        assert_eq!(
            updater_target_for("linux", "x86_64", Some(BundleType::AppImage)),
            "linux-x86_64-appimage"
        );
        assert_eq!(
            updater_target_for("linux", "x86_64", Some(BundleType::Deb)),
            "linux-x86_64-deb"
        );
        assert_eq!(updater_target_for("linux", "x86_64", None), "unsupported");
    }

    #[test]
    fn windows_installer_requires_confirmed_sidecar_shutdown() {
        assert!(updater_requires_explicit_sidecar_shutdown("windows"));
        assert!(!updater_requires_explicit_sidecar_shutdown("macos"));
        assert!(!updater_requires_explicit_sidecar_shutdown("linux"));
    }

    #[test]
    fn only_an_empty_internal_single_instance_connection_is_a_readiness_probe() {
        assert!(single_instance_notification_is_probe(&[String::new()], ""));
        assert!(!single_instance_notification_is_probe(
            &["/Applications/YesPlayMusic.app".into()],
            "/tmp"
        ));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_startup_gate_tests {
    use super::{acquire_macos_startup_gate, single_instance_socket_path, startup_gate_path};
    use std::{sync::mpsc, thread, time::Duration};

    #[test]
    fn startup_gate_serializes_cold_processes_before_the_plugin_listener() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("startup.lock");
        let first = acquire_macos_startup_gate(&path, Duration::from_secs(1)).unwrap();
        let (acquired, receiver) = mpsc::channel();
        let waiter = thread::spawn(move || {
            let _second = acquire_macos_startup_gate(&path, Duration::from_secs(1)).unwrap();
            acquired.send(()).unwrap();
        });

        assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
        drop(first);
        receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        waiter.join().unwrap();
    }

    #[test]
    fn paths_match_the_locked_single_instance_plugin_contract() {
        assert_eq!(
            single_instance_socket_path("com.electron.yesplaymusic"),
            std::path::PathBuf::from("/tmp/com_electron_yesplaymusic_si.sock")
        );
        assert!(startup_gate_path("com.electron.yesplaymusic")
            .ends_with("com_electron_yesplaymusic_startup.lock"));
    }
}

#[tauri::command]
fn updater_configured() -> bool {
    updater_public_key().is_some()
}

fn updater_requires_explicit_sidecar_shutdown(os: &str) -> bool {
    os == "windows"
}

fn single_instance_notification_is_probe(args: &[String], cwd: &str) -> bool {
    cwd.is_empty() && args == [""]
}

#[tauri::command]
fn prepare_for_update(app: AppHandle) -> Result<bool, String> {
    if !updater_requires_explicit_sidecar_shutdown(std::env::consts::OS) {
        return Ok(false);
    }
    let Some(state) = app.try_state::<SidecarState>() else {
        return Ok(false);
    };
    state.shutdown_requested.store(true, Ordering::Release);
    if stop_sidecar_gracefully(&app) {
        Ok(true)
    } else {
        Err("无法确认旧版后台服务已退出，已取消安装".into())
    }
}

fn app_about_metadata(version: &str) -> AboutMetadata<'static> {
    AboutMetadataBuilder::new()
        .name(Some("YesPlayMusic"))
        // macOS reads the build number from Info.plist, so provide only the short version.
        .short_version(Some(version.to_string()))
        .credits(Some("Tauri 2 跨平台版\n由 Nagi Studio 独立维护"))
        .copyright(Some("基于 qier222/YesPlayMusic 的开源工作重构"))
        .build()
}

fn create_app_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let preferences = MenuItem::with_id(
        app,
        "app.preferences",
        "Preferences…",
        true,
        Some("CmdOrCtrl+,"),
    )?;
    let search = MenuItem::with_id(app, "app.search", "Search", true, Some("CmdOrCtrl+F"))?;
    #[cfg(target_os = "macos")]
    let speech = Submenu::with_items(
        app,
        "Speech",
        true,
        &[
            &MenuItem::with_id(
                app,
                "app.startSpeaking",
                "Start Speaking",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(app, "app.stopSpeaking", "Stop Speaking", true, None::<&str>)?,
        ],
    )?;
    let controls = Submenu::with_items(
        app,
        "Controls",
        true,
        &[
            &MenuItem::with_id(app, "app.play", "Play / Pause", true, None::<&str>)?,
            &MenuItem::with_id(app, "app.next", "Next", true, None::<&str>)?,
            &MenuItem::with_id(app, "app.previous", "Previous", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                "app.increaseVolume",
                "Increase Volume",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                "app.decreaseVolume",
                "Decrease Volume",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(app, "app.like", "Like", true, None::<&str>)?,
            &MenuItem::with_id(app, "app.repeat", "Repeat", true, Some("Alt+R"))?,
            &MenuItem::with_id(app, "app.shuffle", "Shuffle", true, Some("Alt+S"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                "app.minimizeToTray",
                "Minimize to Tray",
                true,
                None::<&str>,
            )?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            #[cfg(target_os = "macos")]
            &MenuItem::with_id(app, "app.delete", "Delete", true, None::<&str>)?,
            &PredefinedMenuItem::select_all(app, None)?,
            #[cfg(target_os = "macos")]
            &speech,
            &PredefinedMenuItem::separator(app)?,
            &search,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &MenuItem::with_id(
                app,
                "app.toggleFullscreen",
                "Toggle Full Screen",
                true,
                None::<&str>,
            )?,
            #[cfg(debug_assertions)]
            &PredefinedMenuItem::separator(app)?,
            #[cfg(debug_assertions)]
            &MenuItem::with_id(app, "app.reload", "Reload", true, Some("CmdOrCtrl+R"))?,
            #[cfg(debug_assertions)]
            &MenuItem::with_id(
                app,
                "app.forceReload",
                "Force Reload",
                true,
                Some("CmdOrCtrl+Shift+R"),
            )?,
            #[cfg(debug_assertions)]
            &MenuItem::with_id(
                app,
                "app.toggleDevtools",
                "Toggle Developer Tools",
                true,
                Some("F12"),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    let help = Submenu::with_items(
        app,
        "Help",
        true,
        &[
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::about(
                app,
                Some("About YesPlayMusic"),
                Some(app_about_metadata(&app.package_info().version.to_string())),
            )?,
            &MenuItem::with_id(app, "app.github", "GitHub", true, None::<&str>)?,
            &MenuItem::with_id(app, "app.tauri", "Tauri", true, None::<&str>)?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    let app_submenu = Submenu::with_items(
        app,
        "YesPlayMusic",
        true,
        &[
            &PredefinedMenuItem::about(
                app,
                Some("About YesPlayMusic"),
                Some(app_about_metadata(&app.package_info().version.to_string())),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &preferences,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    #[cfg(not(target_os = "macos"))]
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &preferences,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    Menu::with_items(
        app,
        &[
            #[cfg(target_os = "macos")]
            &app_submenu,
            #[cfg(not(target_os = "macos"))]
            &file,
            &edit,
            &controls,
            &window,
            &help,
        ],
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SidecarProcessIdentity {
    pid: u32,
    generation: usize,
}

#[derive(Default)]
struct SidecarProcessSlot {
    child: Option<CommandChild>,
    current: Option<SidecarProcessIdentity>,
}

#[derive(Default)]
struct SidecarTerminationWait {
    expected: Option<SidecarProcessIdentity>,
    terminated: bool,
}

struct SidecarState {
    process: Mutex<SidecarProcessSlot>,
    next_generation: AtomicUsize,
    termination_wait: Mutex<SidecarTerminationWait>,
    termination_changed: Condvar,
    shutdown_requested: AtomicBool,
    restart_attempts: AtomicUsize,
    replacement_ready: AtomicBool,
    permanently_unavailable: AtomicBool,
}

#[derive(Clone)]
struct SidecarLaunchConfig {
    health_token: String,
    upstream_proxy: Option<String>,
    ready_port: u16,
}

#[derive(Debug, PartialEq, Eq)]
enum SidecarExitAction {
    Stop,
    Restart(Duration),
    NotifyFailure,
}

fn sidecar_exit_action(shutdown_requested: bool, completed_restarts: usize) -> SidecarExitAction {
    if shutdown_requested {
        return SidecarExitAction::Stop;
    }
    match completed_restarts {
        0 => SidecarExitAction::Restart(Duration::from_millis(500)),
        1 => SidecarExitAction::Restart(Duration::from_secs(1)),
        2 => SidecarExitAction::Restart(Duration::from_secs(2)),
        _ => SidecarExitAction::NotifyFailure,
    }
}

fn sidecar_startup_error_message(port: u16, detail: &str) -> String {
    format!(
        "后台服务无法启动。端口 {port} 可能已被其他程序占用。\n\n请退出其他 YesPlayMusic 实例或释放该端口后重试。\n\n技术信息：{detail}"
    )
}

fn handle_sidecar_startup_failure(
    app: &AppHandle,
    port: u16,
    error: impl Into<String>,
    show_dialog: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let error = error.into();
    if !show_dialog {
        return Err(std::io::Error::other(error).into());
    }
    let message = sidecar_startup_error_message(port, &error);
    eprintln!("[tauri] {message}");
    let exit_handle = app.clone();
    app.dialog()
        .message(message)
        .title("YesPlayMusic 无法启动")
        .kind(MessageDialogKind::Error)
        .show(move |_| exit_handle.exit(1));
    Ok(())
}

struct DesktopPreferencesState(Mutex<DesktopPreferences>);

struct ClosePromptState(AtomicBool);

struct WindowPreferencesState {
    preferences: Mutex<WindowPreferences>,
    movement: Mutex<PendingWindowMovement>,
}

struct StartupWindowState(AtomicBool);

#[derive(Default)]
struct PendingWindowMovement {
    position: Option<WindowPosition>,
    generation: u64,
    worker_running: bool,
}

#[derive(Default)]
struct TrayMenuRegistration {
    player_state: PlayerStatePayload,
    play: Option<MenuItem<tauri::Wry>>,
    like: Option<MenuItem<tauri::Wry>>,
}

struct TrayMenuState(Mutex<TrayMenuRegistration>);

struct TrayAvailabilityState(AtomicBool);

#[cfg(target_os = "linux")]
struct LinuxMediaState(Option<LinuxMedia>);

#[derive(Default)]
struct TrayCoverState(Mutex<Option<String>>);

#[derive(Default)]
struct TrayTitleRegistration {
    title: String,
    rendered: Option<String>,
}

#[derive(Default)]
struct TrayTitleState(Mutex<TrayTitleRegistration>);

#[derive(Default)]
struct GlobalShortcutRegistration {
    actions: HashMap<u32, String>,
    settings: Option<serde_json::Value>,
    temporarily_disabled: bool,
}

struct GlobalShortcutRegistrationState(Mutex<GlobalShortcutRegistration>);

const MAX_TRAY_COVER_BYTES: u64 = 2 * 1024 * 1024;

fn tray_cover_url(payload: &serde_json::Value) -> Option<&str> {
    payload
        .get("coverUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
}

fn character_display_width(character: char) -> usize {
    if matches!(
        character,
        '\u{1100}'..='\u{115f}'
            | '\u{2e80}'..='\u{a4cf}'
            | '\u{ac00}'..='\u{d7a3}'
            | '\u{f900}'..='\u{faff}'
            | '\u{fe30}'..='\u{fe6f}'
            | '\u{ff00}'..='\u{ffa0}'
            | '\u{ffe0}'..='\u{ffe6}'
    ) {
        2
    } else {
        1
    }
}

fn truncate_by_display_width(title: &str, max_width: usize) -> String {
    let mut width = 0;
    for (index, character) in title.char_indices() {
        width += character_display_width(character);
        if width > max_width {
            return format!("{}…", &title[..index]);
        }
    }
    title.to_string()
}

fn tray_title_for_visibility(title: &str, window_visible: bool) -> String {
    if window_visible {
        String::new()
    } else {
        truncate_by_display_width(title.trim(), 44)
    }
}

#[cfg(target_os = "macos")]
fn render_tray_title(app: &AppHandle) -> Result<(), String> {
    let title = app
        .state::<TrayTitleState>()
        .0
        .lock()
        .map_err(|_| "菜单栏标题状态锁已损坏".to_string())?
        .title
        .clone();
    let window_visible = app
        .get_webview_window("main")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    if let Some(tray) = app.tray_by_id("main-tray") {
        let title = tray_title_for_visibility(&title, window_visible);
        let already_rendered = app
            .state::<TrayTitleState>()
            .0
            .lock()
            .map_err(|_| "菜单栏标题状态锁已损坏".to_string())?
            .rendered
            .as_deref()
            == Some(title.as_str());
        if already_rendered {
            return Ok(());
        }
        tray.set_title(Some(&title))
            .map_err(|error| error.to_string())?;
        app.state::<TrayTitleState>()
            .0
            .lock()
            .map_err(|_| "菜单栏标题状态锁已损坏".to_string())?
            .rendered = Some(title);
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn render_tray_title(_app: &AppHandle) -> Result<(), String> {
    // Tray titles are meaningful only in the macOS menu bar.
    Ok(())
}

#[cfg(target_os = "macos")]
const TRAY_TITLE_RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

#[cfg(target_os = "macos")]
fn spawn_tray_title_reconciler(app: &AppHandle) {
    // Tauri emits no visibility event for Cmd+H or the miniaturize button, so a
    // slow reconcile pass catches transitions the explicit render calls miss.
    // A fixed-interval thread cannot re-enter the busy-loop that per-iteration
    // MainEventsCleared polling caused: each tick wakes the main thread once.
    let handle = app.clone();
    thread::spawn(move || loop {
        thread::sleep(TRAY_TITLE_RECONCILE_INTERVAL);
        let render_handle = handle.clone();
        let dispatched = handle.run_on_main_thread(move || {
            let _ = render_tray_title(&render_handle);
        });
        if dispatched.is_err() {
            // The event loop is gone; the app is shutting down.
            break;
        }
    });
}

fn decode_tray_cover(bytes: &[u8]) -> Result<TauriImage<'static>, String> {
    decode_tray_image(bytes, 64)
}

fn decode_tray_icon(bytes: &[u8]) -> Result<TauriImage<'static>, String> {
    decode_tray_image(bytes, 20)
}

fn decode_tray_image(bytes: &[u8], size: u32) -> Result<TauriImage<'static>, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let cover = reader
        .decode()
        .map_err(|error| error.to_string())?
        .resize_exact(size, size, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    Ok(TauriImage::new_owned(cover.into_raw(), size, size))
}

async fn download_tray_cover(url: &str) -> Result<TauriImage<'static>, String> {
    let parsed = reqwest::Url::parse(url).map_err(|error| error.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("封面地址只允许 HTTP(S)".to_string());
    }

    let mut response = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|error| error.to_string())?
        .get(parsed)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    if response.content_length().unwrap_or(0) > MAX_TRAY_COVER_BYTES {
        return Err("菜单栏封面响应过大".to_string());
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(64 * 1024)
            .min(MAX_TRAY_COVER_BYTES) as usize,
    );
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if bytes.len() as u64 + chunk.len() as u64 > MAX_TRAY_COVER_BYTES {
            return Err("菜单栏封面响应过大".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    decode_tray_cover(&bytes)
}

fn update_tray_cover(app: &AppHandle, payload: &serde_json::Value) {
    let Some(cover_url) = tray_cover_url(payload) else {
        return;
    };
    let state = app.state::<TrayCoverState>();
    let mut current = match state.0.lock() {
        Ok(current) => current,
        Err(error) => {
            eprintln!("[tauri] 无法锁定菜单栏封面状态：{error}");
            return;
        }
    };
    if current.as_deref() == Some(cover_url) {
        return;
    }
    let cover_url = cover_url.to_string();
    *current = Some(cover_url.clone());
    drop(current);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match download_tray_cover(&cover_url).await {
            Ok(icon) => {
                let main_app = app.clone();
                if let Err(error) = app.run_on_main_thread(move || {
                    let is_current = main_app
                        .state::<TrayCoverState>()
                        .0
                        .lock()
                        .map(|current| current.as_deref() == Some(cover_url.as_str()))
                        .unwrap_or(false);
                    if is_current {
                        if let Some(tray) = main_app.tray_by_id("main-tray") {
                            if let Err(error) = tray.set_icon(Some(icon)) {
                                eprintln!("[tauri] 无法更新菜单栏封面：{error}");
                            }
                        }
                    }
                }) {
                    eprintln!("[tauri] 无法调度菜单栏封面更新：{error}");
                }
            }
            Err(error) => {
                // Clear deduplication state so the next lyrics update retries.
                let mut cleared = false;
                if let Ok(mut current) = app.state::<TrayCoverState>().0.lock() {
                    if current.as_deref() == Some(cover_url.as_str()) {
                        *current = None;
                        cleared = true;
                    }
                }
                if cleared {
                    let main_app = app.clone();
                    if let Err(error) = app.run_on_main_thread(move || {
                        if let Some(window) = main_app.get_webview_window("main") {
                            if let Ok(theme) = window.theme() {
                                let _ = update_tray_icon(&main_app, theme);
                            }
                        }
                    }) {
                        eprintln!("[tauri] 无法调度菜单栏图标恢复：{error}");
                    }
                }
                eprintln!("[tauri] 无法下载菜单栏封面：{error}");
            }
        }
    });
}

fn update_tray_menu(app: &AppHandle, player_state: PlayerStatePayload) -> Result<(), String> {
    let state = app.state::<TrayMenuState>();
    let mut registration = state.0.lock().map_err(|error| error.to_string())?;
    registration.player_state = player_state;
    let text = tray_menu_text(player_state);
    if let Some(play) = &registration.play {
        play.set_text(text.playback)
            .map_err(|error| error.to_string())?;
    }
    if let Some(like) = &registration.like {
        like.set_text(text.like)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn tray_icon_path(
    app: &AppHandle,
    theme: TrayIconTheme,
    system_theme: tauri::Theme,
) -> Result<PathBuf, String> {
    let relative = tray_icon_asset(theme, system_theme).relative_path();
    let bundled = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?
        .join("renderer")
        .join(relative);
    if bundled.is_file() {
        return Ok(bundled);
    }

    #[cfg(debug_assertions)]
    {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../public")
            .join(relative);
        if source.is_file() {
            return Ok(source);
        }
    }

    Err(format!("tray icon asset is missing: {relative}"))
}

fn update_tray_icon(app: &AppHandle, system_theme: tauri::Theme) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if app
        .state::<TrayCoverState>()
        .0
        .lock()
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Ok(());
    }

    let preference = app
        .state::<DesktopPreferencesState>()
        .0
        .lock()
        .map_err(|error| error.to_string())?
        .tray_icon_theme;
    let path = tray_icon_path(app, preference, system_theme)?;
    let icon = decode_tray_icon(&fs::read(path).map_err(|error| error.to_string())?)?;
    if let Some(tray) = app.tray_by_id("main-tray") {
        tray.set_icon(Some(icon))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn update_desktop_preferences(app: &AppHandle, payload: &serde_json::Value) -> Result<(), String> {
    let preferences = parse_desktop_preferences(payload).map_err(|error| error.to_string())?;
    *app.state::<DesktopPreferencesState>()
        .0
        .lock()
        .map_err(|error| error.to_string())? = preferences;

    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "linux")]
        window
            .set_decorations(!preferences.linux_enable_custom_titlebar)
            .map_err(|error| error.to_string())?;
        let system_theme = window.theme().map_err(|error| error.to_string())?;
        update_tray_icon(app, system_theme)?;
    }
    Ok(())
}

fn hide_main_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.hide().map_err(|error| error.to_string())?;
        render_tray_title(app)?;
    }
    Ok(())
}

fn tray_recovery_available(is_macos: bool, tray_available: bool) -> bool {
    is_macos || tray_available
}

fn can_hide_main_window(app: &AppHandle) -> bool {
    let tray_available = app
        .try_state::<TrayAvailabilityState>()
        .is_some_and(|state| state.0.load(Ordering::Acquire));
    tray_recovery_available(cfg!(target_os = "macos"), tray_available)
}

fn hide_main_window_or_exit(app: &AppHandle) -> Result<(), String> {
    if can_hide_main_window(app) {
        hide_main_window(app)
    } else {
        app.exit(0);
        Ok(())
    }
}

fn show_main_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_minimized().map_err(|error| error.to_string())? {
            window.unminimize().map_err(|error| error.to_string())?;
        }
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        render_tray_title(app)?;
    }
    Ok(())
}

fn show_pending_startup_window(app: &AppHandle) -> Result<bool, String> {
    let pending = &app.state::<StartupWindowState>().0;
    if !claim_startup_show(pending) {
        return Ok(false);
    }
    if let Err(error) = show_main_window(app) {
        pending.store(true, Ordering::Release);
        return Err(error);
    }
    Ok(true)
}

fn claim_startup_show(pending: &AtomicBool) -> bool {
    pending.swap(false, Ordering::AcqRel)
}

fn schedule_startup_show_fallback(app: &AppHandle) {
    let handle = app.clone();
    thread::spawn(move || {
        thread::sleep(STARTUP_SHOW_TIMEOUT);
        match show_pending_startup_window(&handle) {
            Ok(true) => eprintln!("[tauri] renderer readiness timed out; showing the window"),
            Ok(false) => {}
            Err(error) => eprintln!("[tauri] failed to show the startup window: {error}"),
        }
    });
}

fn resolve_close_choice(app: &AppHandle, payload: serde_json::Value) -> Result<(), String> {
    if !app
        .state::<ClosePromptState>()
        .0
        .swap(false, Ordering::AcqRel)
    {
        return Err("no close choice is pending".to_string());
    }
    let choice = parse_close_choice(payload).map_err(|error| error.to_string())?;
    if choice.remember {
        let option = match choice.action {
            CloseChoiceAction::Exit => CloseAppOption::Exit,
            CloseChoiceAction::MinimizeToTray => CloseAppOption::MinimizeToTray,
        };
        app.state::<DesktopPreferencesState>()
            .0
            .lock()
            .map_err(|error| error.to_string())?
            .close_app_option = option;
        let value = match option {
            CloseAppOption::Exit => "exit",
            CloseAppOption::MinimizeToTray => "minimizeToTray",
            CloseAppOption::Ask => unreachable!(),
        };
        if let Err(error) = app.emit("desktop://rememberCloseAppOption", value) {
            eprintln!("[tauri] failed to persist the close choice in the renderer: {error}");
        }
    }

    match choice.action {
        CloseChoiceAction::Exit => app.exit(0),
        CloseChoiceAction::MinimizeToTray => hide_main_window_or_exit(app)?,
    }
    Ok(())
}

fn emit_desktop_event(app: &AppHandle, event: &str) {
    let _ = app.emit(&format!("desktop://{event}"), ());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppMenuAction {
    Emit(&'static str),
    Navigate(&'static str),
    Hide,
    OpenUrl(&'static str),
    DeleteSelection,
    StartSpeaking,
    StopSpeaking,
    Reload,
    ToggleFullscreen,
    ToggleDevtools,
}

fn app_menu_action(id: &str) -> Option<AppMenuAction> {
    match id.strip_prefix("app.")? {
        "preferences" => Some(AppMenuAction::Navigate("/settings")),
        "search" => Some(AppMenuAction::Emit("search")),
        "play" => Some(AppMenuAction::Emit("play")),
        "next" => Some(AppMenuAction::Emit("next")),
        "previous" => Some(AppMenuAction::Emit("previous")),
        "increaseVolume" => Some(AppMenuAction::Emit("increaseVolume")),
        "decreaseVolume" => Some(AppMenuAction::Emit("decreaseVolume")),
        "like" => Some(AppMenuAction::Emit("like")),
        "repeat" => Some(AppMenuAction::Emit("repeat")),
        "shuffle" => Some(AppMenuAction::Emit("shuffle")),
        "minimizeToTray" => Some(AppMenuAction::Hide),
        "github" => Some(AppMenuAction::OpenUrl(
            "https://github.com/nagi-studio/YesPlayMusic",
        )),
        "tauri" => Some(AppMenuAction::OpenUrl("https://tauri.app")),
        "delete" => Some(AppMenuAction::DeleteSelection),
        "startSpeaking" => Some(AppMenuAction::StartSpeaking),
        "stopSpeaking" => Some(AppMenuAction::StopSpeaking),
        "reload" | "forceReload" => Some(AppMenuAction::Reload),
        "toggleFullscreen" => Some(AppMenuAction::ToggleFullscreen),
        "toggleDevtools" => Some(AppMenuAction::ToggleDevtools),
        _ => None,
    }
}

fn handle_app_menu_event(app: &AppHandle, id: &str) {
    let Some(action) = app_menu_action(id) else {
        return;
    };
    match action {
        AppMenuAction::Emit(event) => emit_desktop_event(app, event),
        AppMenuAction::Navigate(path) => {
            let _ = app.emit("desktop://changeRouteTo", path);
        }
        AppMenuAction::Hide => {
            if let Err(error) = hide_main_window_or_exit(app) {
                eprintln!("[tauri] failed to hide the main window: {error}");
            }
        }
        AppMenuAction::OpenUrl(url) => {
            if let Err(error) = app.opener().open_url(url, None::<&str>) {
                eprintln!("[tauri] failed to open menu URL: {error}");
            }
        }
        AppMenuAction::DeleteSelection => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.eval("document.execCommand('delete');");
            }
        }
        AppMenuAction::StartSpeaking => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.eval(
                    "(() => { const text = String(window.getSelection?.() ?? '').trim(); if (text) { speechSynthesis.cancel(); speechSynthesis.speak(new SpeechSynthesisUtterance(text)); } })();",
                );
            }
        }
        AppMenuAction::StopSpeaking => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.eval("speechSynthesis.cancel();");
            }
        }
        AppMenuAction::Reload => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.reload();
            }
        }
        AppMenuAction::ToggleFullscreen => {
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(fullscreen) = window.is_fullscreen() {
                    let _ = window.set_fullscreen(!fullscreen);
                }
            }
        }
        AppMenuAction::ToggleDevtools =>
        {
            #[cfg(debug_assertions)]
            if let Some(window) = app.get_webview_window("main") {
                if window.is_devtools_open() {
                    window.close_devtools();
                } else {
                    window.open_devtools();
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn handle_linux_media_control(app: &AppHandle, control: MediaControl) {
    match control {
        MediaControl::Quit => app.exit(0),
        MediaControl::Next => emit_desktop_event(app, "next"),
        MediaControl::Previous => emit_desktop_event(app, "previous"),
        MediaControl::Play => emit_desktop_event(app, "resume"),
        MediaControl::Pause => emit_desktop_event(app, "pause"),
        MediaControl::PlayPause => emit_desktop_event(app, "play"),
        MediaControl::SeekBy(seconds) => {
            let _ = app.emit("desktop://seekBy", seconds);
        }
        MediaControl::SeekTo(seconds) => {
            let _ = app.emit("desktop://setPosition", seconds);
        }
        MediaControl::SetRepeat(mode) => {
            let mode = match mode {
                RepeatMode::Off => "off",
                RepeatMode::Track => "one",
                RepeatMode::Playlist => "on",
            };
            let _ = app.emit("desktop://setRepeat", mode);
        }
        MediaControl::SetShuffle(enabled) => {
            let _ = app.emit("desktop://setShuffle", enabled);
        }
    }
}

#[cfg(target_os = "macos")]
fn set_window_button_visibility(window: WebviewWindow, visible: bool) -> Result<(), String> {
    let window_on_main = window.clone();
    window
        .run_on_main_thread(move || match window_on_main.ns_window() {
            Ok(pointer) => {
                let ns_window = unsafe { &*(pointer.cast::<NSWindow>()) };
                for kind in [
                    NSWindowButton::CloseButton,
                    NSWindowButton::MiniaturizeButton,
                    NSWindowButton::ZoomButton,
                ] {
                    if let Some(button) = ns_window.standardWindowButton(kind) {
                        button.setHidden(!visible);
                    }
                }
            }
            Err(error) => eprintln!("[tauri] 无法取得 macOS 窗口按钮：{error}"),
        })
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
fn set_window_button_visibility(_window: WebviewWindow, _visible: bool) -> Result<(), String> {
    Ok(())
}

fn normalize_global_shortcut(shortcut: &str) -> String {
    shortcut
        .split('+')
        .map(|part| match part {
            "CommandOrControl" => "CmdOrCtrl",
            "Right" => "ArrowRight",
            "Left" => "ArrowLeft",
            "Up" => "ArrowUp",
            "Down" => "ArrowDown",
            _ => part,
        })
        .collect::<Vec<_>>()
        .join("+")
}

fn register_global_shortcuts(app: &AppHandle) -> Result<(), String> {
    app.global_shortcut()
        .unregister_all()
        .map_err(|error| error.to_string())?;

    let state = app.state::<GlobalShortcutRegistrationState>();
    let (settings, temporarily_disabled) = {
        let mut registration = state.0.lock().map_err(|error| error.to_string())?;
        registration.actions.clear();
        (
            registration.settings.clone(),
            registration.temporarily_disabled,
        )
    };
    let Some(settings) = settings else {
        return Ok(());
    };
    if temporarily_disabled
        || !settings
            .get("enableGlobalShortcut")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
    {
        return Ok(());
    }

    let shortcuts = settings
        .get("shortcuts")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "快捷键设置缺少 shortcuts 数组".to_string())?;
    let mut actions = HashMap::new();
    for item in shortcuts {
        let Some(action) = item.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(accelerator) = item
            .get("globalShortcut")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Ok(shortcut) = normalize_global_shortcut(accelerator).parse::<Shortcut>() else {
            eprintln!("[tauri] 忽略无法解析的快捷键：{accelerator}");
            continue;
        };
        let shortcut_id = shortcut.id();
        if let Err(error) = app.global_shortcut().register(shortcut) {
            // One unavailable shortcut must not block the remaining bindings.
            eprintln!("[tauri] 无法注册快捷键 {accelerator}: {error}");
            continue;
        }
        actions.insert(shortcut_id, action.to_string());
    }

    state.0.lock().map_err(|error| error.to_string())?.actions = actions;
    Ok(())
}

fn update_shortcut_settings(
    app: &AppHandle,
    channel: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    let state = app.state::<GlobalShortcutRegistrationState>();
    {
        let mut registration = state.0.lock().map_err(|error| error.to_string())?;
        match channel {
            "settings" => registration.settings = Some(payload),
            "switchGlobalShortcutStatusTemporary" => {
                registration.temporarily_disabled = payload.as_str() == Some("disable");
            }
            "updateShortcut" => {
                if let (Some(settings), Some(id), Some(shortcut)) = (
                    registration.settings.as_mut(),
                    payload.get("id").and_then(serde_json::Value::as_str),
                    payload.get("shortcut").and_then(serde_json::Value::as_str),
                ) {
                    if payload.get("type").and_then(serde_json::Value::as_str)
                        == Some("globalShortcut")
                    {
                        if let Some(item) = settings
                            .get_mut("shortcuts")
                            .and_then(serde_json::Value::as_array_mut)
                            .and_then(|items| {
                                items.iter_mut().find(|item| {
                                    item.get("id").and_then(serde_json::Value::as_str) == Some(id)
                                })
                            })
                        {
                            item["globalShortcut"] =
                                serde_json::Value::String(shortcut.to_string());
                        }
                    }
                }
            }
            "restoreDefaultShortcuts" => {
                registration.settings = Some(payload);
            }
            _ => {}
        }
    }
    register_global_shortcuts(app)
}

fn parse_legacy_settings(config: &str) -> Result<Option<serde_json::Value>, String> {
    let value: serde_json::Value =
        serde_json::from_str(config).map_err(|error| error.to_string())?;
    let root = value
        .as_object()
        .ok_or_else(|| "旧版配置根节点必须是对象".to_string())?;
    match root.get("settings") {
        None => Ok(None),
        Some(settings) if settings.is_object() => Ok(Some(settings.clone())),
        Some(_) => Err("旧版 settings 必须是对象".to_string()),
    }
}

#[tauri::command]
fn read_legacy_settings(app: AppHandle) -> Result<Option<serde_json::Value>, String> {
    let Some(bytes) = read_legacy_electron_config(&app)? else {
        return Ok(None);
    };
    let config = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    parse_legacy_settings(&config)
}

#[tauri::command]
fn desktop_event(
    app: AppHandle,
    channel: String,
    payload: serde_json::Value,
) -> Result<(), String> {
    match channel.as_str() {
        "settings"
        | "switchGlobalShortcutStatusTemporary"
        | "updateShortcut"
        | "restoreDefaultShortcuts" => {
            if channel == "settings" {
                let enabled = payload
                    .get("enableDiscordRichPresence")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                app.state::<DiscordPresenceHandle>().configure(enabled)?;
                update_desktop_preferences(&app, &payload)?;
            }
            update_shortcut_settings(&app, &channel, payload)?;
        }
        "discordPresence" => {
            let presence: DiscordPresencePayload =
                serde_json::from_value(payload).map_err(|error| error.to_string())?;
            app.state::<DiscordPresenceHandle>().update(presence)?;
        }
        "mediaState" => {
            let tray_state =
                parse_player_state_from_media_state(&payload).map_err(|error| error.to_string())?;
            update_tray_menu(&app, tray_state)?;

            #[cfg(target_os = "macos")]
            macos_media_controls::update_player_state(&app, payload)?;

            #[cfg(target_os = "linux")]
            {
                let state: MediaState =
                    serde_json::from_value(payload).map_err(|error| error.to_string())?;
                if let Some(media) = &app.state::<LinuxMediaState>().0 {
                    media.update_state(state);
                }
            }

            #[cfg(target_os = "windows")]
            let _ = payload;
        }
        "mediaMetadata" => {
            #[cfg(target_os = "linux")]
            {
                let metadata: MediaMetadata =
                    serde_json::from_value(payload).map_err(|error| error.to_string())?;
                if let Some(media) = &app.state::<LinuxMediaState>().0 {
                    media.set_metadata(metadata);
                }
            }

            #[cfg(not(target_os = "linux"))]
            let _ = payload;
        }
        "mediaSeeked" => {
            #[cfg(target_os = "linux")]
            {
                let seconds = payload
                    .as_f64()
                    .ok_or_else(|| "media seek position must be numeric".to_string())?;
                if let Some(media) = &app.state::<LinuxMediaState>().0 {
                    media.set_position(seconds, true);
                }
            }

            #[cfg(not(target_os = "linux"))]
            let _ = payload;
        }
        "updateTrayTooltip" => {
            if let (Some(tray), Some(title)) = (app.tray_by_id("main-tray"), payload.as_str()) {
                tray.set_tooltip(Some(title))
                    .map_err(|error| error.to_string())?;
            }
        }
        "updateTrayNowPlaying" => {
            #[cfg(target_os = "macos")]
            {
                if let Some(title) = payload.get("title").and_then(serde_json::Value::as_str) {
                    app.state::<TrayTitleState>()
                        .0
                        .lock()
                        .map_err(|_| "菜单栏标题状态锁已损坏".to_string())?
                        .title = title.to_string();
                }
                render_tray_title(&app)?;
                update_tray_cover(&app, &payload);
            }

            #[cfg(not(target_os = "macos"))]
            let _ = payload;
        }
        "setProxy" => {
            save_webview_proxy(&app, payload).map_err(|error| error.to_string())?;
        }
        "removeProxy" => {
            remove_webview_proxy(&app, &payload).map_err(|error| error.to_string())?;
        }
        "resolveCloseChoice" => resolve_close_choice(&app, payload)?,
        "cancelCloseChoice" => {
            if !payload.is_null() {
                return Err("cancelCloseChoice payload must be null".to_string());
            }
            app.state::<ClosePromptState>()
                .0
                .store(false, Ordering::Release);
        }
        "setWindowButtonVisibility" => {
            let visible = payload
                .as_bool()
                .ok_or_else(|| "窗口按钮显隐参数必须是布尔值".to_string())?;
            if let Some(window) = app.get_webview_window("main") {
                set_window_button_visibility(window, visible)?;
            }
        }
        "minimize" => {
            if let Some(window) = app.get_webview_window("main") {
                window.minimize().map_err(|error| error.to_string())?;
            }
        }
        "maximizeOrUnmaximize" => {
            if let Some(window) = app.get_webview_window("main") {
                let maximized = window.is_maximized().map_err(|error| error.to_string())?;
                if maximized {
                    window.unmaximize().map_err(|error| error.to_string())?;
                } else {
                    window.maximize().map_err(|error| error.to_string())?;
                }
                app.emit("desktop://isMaximized", !maximized)
                    .map_err(|error| error.to_string())?;
            }
        }
        "close" => {
            if let Some(window) = app.get_webview_window("main") {
                window.close().map_err(|error| error.to_string())?;
            }
        }
        _ => return Err(format!("不允许的桌面事件：{channel}")),
    }
    Ok(())
}

#[tauri::command]
fn is_always_on_top(window: WebviewWindow) -> Result<bool, String> {
    window.is_always_on_top().map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn set_macos_fullscreen_workspace_visibility(
    window: &WebviewWindow,
    visible: bool,
) -> Result<(), String> {
    let window_on_main = window.clone();
    window
        .run_on_main_thread(move || match window_on_main.ns_window() {
            Ok(pointer) => {
                let ns_window = unsafe { &*(pointer.cast::<NSWindow>()) };
                let flags = NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary;
                let mut behavior = ns_window.collectionBehavior();
                if visible {
                    behavior |= flags;
                } else {
                    behavior &= !flags;
                }
                ns_window.setCollectionBehavior(behavior);
            }
            Err(error) => eprintln!("[tauri] failed to access the macOS window: {error}"),
        })
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
fn set_macos_fullscreen_workspace_visibility(
    _window: &WebviewWindow,
    _visible: bool,
) -> Result<(), String> {
    Ok(())
}

fn apply_always_on_top(window: &WebviewWindow, enabled: bool) -> Result<(), String> {
    window
        .set_always_on_top(enabled)
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    window
        .set_visible_on_all_workspaces(enabled)
        .map_err(|error| error.to_string())?;
    set_macos_fullscreen_workspace_visibility(window, enabled)
}

#[tauri::command]
fn toggle_always_on_top(window: WebviewWindow) -> Result<bool, String> {
    let state = window.app_handle().state::<WindowPreferencesState>();
    let mut preferences = state
        .preferences
        .lock()
        .map_err(|error| error.to_string())?;
    let previous = preferences.always_on_top;
    let next = !preferences.always_on_top;
    apply_always_on_top(&window, next)?;
    preferences.always_on_top = next;
    if let Err(error) = save_window_preferences(window.app_handle(), &preferences) {
        preferences.always_on_top = previous;
        let _ = apply_always_on_top(&window, previous);
        return Err(error);
    }
    Ok(next)
}

fn load_initial_window_preferences(app: &AppHandle) -> WindowPreferences {
    match load_window_preferences(app) {
        Ok(Some(preferences)) => preferences,
        Ok(None) => {
            let legacy = match load_legacy_preferences(app) {
                Ok(preferences) => preferences,
                Err(error) => {
                    eprintln!("[tauri] ignored invalid legacy window preferences: {error}");
                    None
                }
            };
            with_legacy_fallback(None, legacy)
        }
        Err(error) => {
            eprintln!("[tauri] ignored an invalid window preference: {error}");
            WindowPreferences::default()
        }
    }
}

fn persist_window_position(app: &AppHandle, position: WindowPosition) -> Result<(), String> {
    let state = app.state::<WindowPreferencesState>();
    let mut preferences = state
        .preferences
        .lock()
        .map_err(|error| error.to_string())?;
    if preferences.position == Some(position) {
        return Ok(());
    }
    let previous = preferences.position;
    preferences.position = Some(position);
    if let Err(error) = save_window_preferences(app, &preferences) {
        preferences.position = previous;
        return Err(error);
    }
    Ok(())
}

fn persist_current_window_position(app: &AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    if window.is_minimized().map_err(|error| error.to_string())?
        || window.is_maximized().map_err(|error| error.to_string())?
        || window.is_fullscreen().map_err(|error| error.to_string())?
    {
        return Ok(());
    }
    let position = window.outer_position().map_err(|error| error.to_string())?;
    persist_window_position(
        app,
        WindowPosition {
            x: position.x,
            y: position.y,
        },
    )
}

fn schedule_window_position_save(app: &AppHandle, position: WindowPosition) {
    let state = app.state::<WindowPreferencesState>();
    let mut movement = match state.movement.lock() {
        Ok(movement) => movement,
        Err(error) => {
            eprintln!("[tauri] failed to queue the window position: {error}");
            return;
        }
    };
    movement.position = Some(position);
    movement.generation = movement.generation.wrapping_add(1);
    if movement.worker_running {
        return;
    }
    movement.worker_running = true;
    drop(movement);

    let handle = app.clone();
    thread::spawn(move || loop {
        let generation = match handle.state::<WindowPreferencesState>().movement.lock() {
            Ok(movement) => movement.generation,
            Err(error) => {
                eprintln!("[tauri] failed to inspect the window position queue: {error}");
                return;
            }
        };
        thread::sleep(WINDOW_MOVE_SETTLE_TIME);
        let position = {
            let state = handle.state::<WindowPreferencesState>();
            let mut movement = match state.movement.lock() {
                Ok(movement) => movement,
                Err(error) => {
                    eprintln!("[tauri] failed to inspect the window position queue: {error}");
                    return;
                }
            };
            if movement.generation != generation {
                continue;
            }
            movement.worker_running = false;
            movement.position.take()
        };
        if let Some(position) = position {
            if let Err(error) = persist_window_position(&handle, position) {
                eprintln!("[tauri] failed to persist the window position: {error}");
            }
        }
        return;
    });
}

fn window_frame_has_reachable_area(
    frame: (i32, i32, u32, u32),
    monitors: &[(i32, i32, u32, u32)],
) -> bool {
    let (window_x, window_y, window_width, window_height) = frame;
    if window_width == 0 || window_height == 0 {
        return false;
    }
    // Keep enough of the player visible to discover and drag it back onscreen.
    let minimum_width = i64::from(window_width.min(160));
    let minimum_height = i64::from(window_height.min(80));
    monitors.iter().any(|&(x, y, width, height)| {
        let overlap_width = (i64::from(window_x) + i64::from(window_width))
            .min(i64::from(x) + i64::from(width))
            - i64::from(window_x).max(i64::from(x));
        let overlap_height = (i64::from(window_y) + i64::from(window_height))
            .min(i64::from(y) + i64::from(height))
            - i64::from(window_y).max(i64::from(y));
        overlap_width >= minimum_width && overlap_height >= minimum_height
    })
}

fn monitor_work_areas(window: &WebviewWindow) -> Result<Vec<(i32, i32, u32, u32)>, String> {
    window
        .available_monitors()
        .map_err(|error| error.to_string())
        .map(|monitors| {
            monitors
                .iter()
                .map(|monitor| {
                    let area = monitor.work_area();
                    (
                        area.position.x,
                        area.position.y,
                        area.size.width,
                        area.size.height,
                    )
                })
                .collect()
        })
}

fn ensure_main_window_reachable(window: &WebviewWindow) -> Result<(), String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let size = window.outer_size().map_err(|error| error.to_string())?;
    let frame = (position.x, position.y, size.width, size.height);
    if !window_frame_has_reachable_area(frame, &monitor_work_areas(window)?) {
        window.center().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn renderer_ready(window: WebviewWindow) -> Result<bool, String> {
    if window.label() != "main" {
        return Err("renderer_ready is restricted to the main window".to_string());
    }
    show_pending_startup_window(window.app_handle())
}

#[tauri::command]
fn restore_compact_window(
    window: WebviewWindow,
    x: Option<i32>,
    y: Option<i32>,
    width: f64,
    height: f64,
) -> Result<(), String> {
    if !width.is_finite()
        || !height.is_finite()
        || !(300.0..=8192.0).contains(&width)
        || !(48.0..=8192.0).contains(&height)
    {
        return Err("窗口尺寸超出安全范围".to_string());
    }
    if window.is_fullscreen().map_err(|error| error.to_string())? {
        window
            .set_fullscreen(false)
            .map_err(|error| error.to_string())?;
    }
    if window.is_maximized().map_err(|error| error.to_string())? {
        window.unmaximize().map_err(|error| error.to_string())?;
    }
    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|error| error.to_string())?;
    let restored_size = window.outer_size().map_err(|error| error.to_string())?;
    let monitors = monitor_work_areas(&window)?;
    let reachable = x.zip(y).map(|(x, y)| {
        window_frame_has_reachable_area(
            (x, y, restored_size.width, restored_size.height),
            &monitors,
        )
    });
    if let Some((x, y)) = x.zip(y).filter(|_| reachable == Some(true)) {
        window
            .set_position(PhysicalPosition::new(x, y))
            .map_err(|error| error.to_string())?;
    } else {
        window.center().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn create_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let player_state = app
        .state::<TrayMenuState>()
        .0
        .lock()
        .map_err(|error| error.to_string())?
        .player_state;
    let text = tray_menu_text(player_state);
    let play = MenuItem::with_id(app, "play", text.playback, true, None::<&str>)?;
    let previous = MenuItem::with_id(app, "previous", "上一首", true, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "下一首", true, None::<&str>)?;
    let like = MenuItem::with_id(app, "like", text.like, true, None::<&str>)?;
    let repeat = MenuItem::with_id(app, "repeat", "切换循环", true, None::<&str>)?;
    let shuffle = MenuItem::with_id(app, "shuffle", "切换随机播放", true, None::<&str>)?;
    #[cfg(target_os = "linux")]
    let show_main = MenuItem::with_id(app, "show-main", "显示主面板", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            #[cfg(target_os = "linux")]
            &show_main,
            &play,
            &previous,
            &next,
            &like,
            &repeat,
            &shuffle,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("YesPlayMusic")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "show-main" => {
                if let Err(error) = show_main_window(app) {
                    eprintln!("[tauri] failed to show the main window: {error}");
                }
            }
            "play" | "previous" | "next" | "like" | "repeat" | "shuffle" => {
                emit_desktop_event(app, event.id.as_ref());
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    if cfg!(target_os = "macos")
                        && window.is_visible().unwrap_or(false)
                        && window.is_focused().unwrap_or(false)
                    {
                        let _ = window.hide();
                    } else {
                        let _ = show_main_window(app);
                    }
                    let _ = render_tray_title(app);
                }
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.state::<TrayAvailabilityState>()
        .0
        .store(true, Ordering::Release);
    {
        let state = app.state::<TrayMenuState>();
        let mut registration = state.0.lock().map_err(|error| error.to_string())?;
        registration.play = Some(play);
        registration.like = Some(like);
    }
    let system_theme = app
        .get_webview_window("main")
        .map(|window| window.theme())
        .transpose()?
        .unwrap_or(tauri::Theme::Light);
    update_tray_icon(app.handle(), system_theme)?;
    Ok(())
}

fn is_smoke_test<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().any(|arg| arg.as_ref() == "--smoke-test")
}

fn is_webview_smoke_test<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .any(|arg| arg.as_ref() == "--webview-smoke-test")
}

fn response_has_sidecar_identity(response: &str, expected_token: &str) -> bool {
    let Some((headers, body)) = response.split_once("\r\n\r\n") else {
        return false;
    };
    let status_ok = headers
        .lines()
        .next()
        .map(|line| line.starts_with("HTTP/1.1 200 ") || line.starts_with("HTTP/1.0 200 "))
        .unwrap_or(false);
    let token_matches = headers.lines().skip(1).any(|line| {
        line.split_once(':')
            .map(|(name, value)| {
                name.eq_ignore_ascii_case(SIDECAR_HEALTH_TOKEN_HEADER)
                    && value.trim() == expected_token
            })
            .unwrap_or(false)
    });
    status_ok && token_matches && body.trim() == SIDECAR_HEALTH_BODY
}

fn sidecar_identity_matches(address: SocketAddr, expected_token: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(200)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let request = format!(
        "GET {SIDECAR_HEALTH_PATH} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut response = String::new();
    if stream.take(8 * 1024).read_to_string(&mut response).is_err() {
        return false;
    }
    response_has_sidecar_identity(&response, expected_token)
}

fn wait_for_sidecar(port: u16, expected_token: &str, timeout: Duration) -> Result<(), String> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if sidecar_identity_matches(address, expected_token) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }

    Err(format!(
        "等待 YesPlayMusic sidecar 身份握手超时（端口 {port}）"
    ))
}

fn wait_for_supervised_sidecar(
    port: u16,
    initial_token: &str,
    replacement_ready: &AtomicBool,
    permanently_unavailable: &AtomicBool,
    timeout: Duration,
) -> Result<(), String> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if permanently_unavailable.load(Ordering::Acquire) {
            return Err("YesPlayMusic sidecar exhausted its restart budget".into());
        }
        if replacement_ready.load(Ordering::Acquire) {
            return Ok(());
        }
        if sidecar_identity_matches(address, initial_token) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }

    Err(format!(
        "等待 YesPlayMusic sidecar 受监督启动超时（端口 {port}）"
    ))
}

fn generate_sidecar_health_token() -> Result<String, Box<dyn std::error::Error>> {
    let mut bytes = [0_u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| std::io::Error::other(format!("无法生成 Sidecar 健康令牌：{error}")))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(64);
    for byte in bytes {
        token.push(HEX[(byte >> 4) as usize] as char);
        token.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(token)
}

fn start_sidecar(
    app: &AppHandle,
    health_token: &str,
    upstream_proxy: Option<&str>,
) -> Result<(tauri::async_runtime::Receiver<CommandEvent>, CommandChild), String> {
    #[cfg(debug_assertions)]
    let mut args = vec![
        "--api-only".to_string(),
        "--api-port".to_string(),
        API_PORT.to_string(),
        "--parent-pid".to_string(),
        std::process::id().to_string(),
    ];

    #[cfg(not(debug_assertions))]
    let mut args = {
        let renderer_dir = app
            .path()
            .resource_dir()
            .map_err(|error| error.to_string())?
            .join("renderer");
        vec![
            "--api-port".to_string(),
            API_PORT.to_string(),
            "--web-port".to_string(),
            RELEASE_WEB_PORT.to_string(),
            "--renderer-dir".to_string(),
            renderer_dir.to_string_lossy().into_owned(),
            "--parent-pid".to_string(),
            std::process::id().to_string(),
        ]
    };

    if let Some(proxy) = upstream_proxy {
        args.extend([
            "--upstream-proxy".to_string(),
            proxy.to_string(),
            "--proxy-relay-port".to_string(),
            WEBVIEW_PROXY_RELAY_PORT.to_string(),
        ]);
    }
    let command = app
        .shell()
        .sidecar("yesplaymusic-sidecar")
        .map_err(|error| error.to_string())?
        .args(args);

    let (events, mut child) = command.spawn().map_err(|error| error.to_string())?;
    // An anonymous stdin pipe keeps the token out of process listings.
    if let Err(error) = child.write(format!("{health_token}\n").as_bytes()) {
        let _ = child.kill();
        return Err(error.to_string());
    }

    Ok((events, child))
}

fn kill_sidecar(app: &AppHandle) {
    let Some(state) = app.try_state::<SidecarState>() else {
        return;
    };
    let child = state
        .process
        .lock()
        .ok()
        .and_then(|mut process| process.child.take());
    if let Some(child) = child {
        let _ = child.kill();
    }
}

fn kill_sidecar_if_current(app: &AppHandle, expected: SidecarProcessIdentity) {
    let Some(state) = app.try_state::<SidecarState>() else {
        return;
    };
    let child = state.process.lock().ok().and_then(|mut process| {
        if process.current == Some(expected) {
            process.child.take()
        } else {
            None
        }
    });
    if let Some(child) = child {
        let _ = child.kill();
    }
}

fn mark_replacement_ready_if_current(
    state: &SidecarState,
    expected: SidecarProcessIdentity,
) -> bool {
    let Ok(process) = state.process.lock() else {
        return false;
    };
    if process.current != Some(expected)
        || state.shutdown_requested.load(Ordering::Acquire)
        || state.permanently_unavailable.load(Ordering::Acquire)
    {
        return false;
    }
    state.replacement_ready.store(true, Ordering::Release);
    true
}

fn prepare_sidecar_termination_wait(state: &SidecarState, expected: SidecarProcessIdentity) {
    if let Ok(mut termination_wait) = state.termination_wait.lock() {
        termination_wait.expected = Some(expected);
        termination_wait.terminated = false;
    }
}

fn record_sidecar_termination(
    state: &SidecarState,
    identity: SidecarProcessIdentity,
    received_termination: bool,
) {
    if !received_termination {
        return;
    }
    if let Ok(mut termination_wait) = state.termination_wait.lock() {
        if termination_wait.expected != Some(identity) {
            return;
        }
        termination_wait.terminated = true;
        state.termination_changed.notify_all();
    }
}

fn wait_for_sidecar_termination(
    state: &SidecarState,
    expected: SidecarProcessIdentity,
    timeout: Duration,
) -> bool {
    let Ok(termination_wait) = state.termination_wait.lock() else {
        return false;
    };
    let Ok((termination_wait, _)) =
        state
            .termination_changed
            .wait_timeout_while(termination_wait, timeout, |wait| {
                wait.expected == Some(expected) && !wait.terminated
            })
    else {
        return false;
    };
    termination_wait.expected == Some(expected) && termination_wait.terminated
}

fn clear_sidecar_termination_wait(state: &SidecarState, expected: SidecarProcessIdentity) {
    if let Ok(mut termination_wait) = state.termination_wait.lock() {
        if termination_wait.expected == Some(expected) {
            *termination_wait = SidecarTerminationWait::default();
        }
    }
}

fn stop_sidecar_gracefully(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<SidecarState>() else {
        return true;
    };
    let process = state.process.lock().ok().and_then(|mut process| {
        let identity = process.current?;
        process.child.take().map(|child| {
            prepare_sidecar_termination_wait(state.inner(), identity);
            (identity, child)
        })
    });
    let Some((identity, mut child)) = process else {
        return true;
    };

    // Keep CommandChild's process handle until shutdown is confirmed. This avoids ever sending a
    // fallback kill to a bare PID that the OS could already have reused.
    if let Err(error) = child.write(SIDECAR_SHUTDOWN_SIGNAL) {
        eprintln!("[sidecar] failed to request graceful shutdown: {error}");
    }
    if wait_for_sidecar_termination(state.inner(), identity, SIDECAR_GRACEFUL_SHUTDOWN_TIMEOUT) {
        clear_sidecar_termination_wait(state.inner(), identity);
        return true;
    }

    eprintln!(
        "[sidecar] graceful shutdown timed out for pid {}; forcing termination",
        identity.pid
    );
    if let Err(error) = child.kill() {
        eprintln!("[sidecar] failed to force termination: {error}");
        clear_sidecar_termination_wait(state.inner(), identity);
        return false;
    }
    let terminated =
        wait_for_sidecar_termination(state.inner(), identity, SIDECAR_FORCED_SHUTDOWN_TIMEOUT);
    if !terminated {
        eprintln!(
            "[sidecar] termination event timed out for pid {}",
            identity.pid
        );
    }
    clear_sidecar_termination_wait(state.inner(), identity);
    terminated
}

fn install_sidecar_process(
    app: &AppHandle,
    child: CommandChild,
    events: tauri::async_runtime::Receiver<CommandEvent>,
    config: SidecarLaunchConfig,
) -> Option<SidecarProcessIdentity> {
    let state = app.state::<SidecarState>();
    if state.shutdown_requested.load(Ordering::Acquire) {
        let _ = child.kill();
        return None;
    }
    let identity = SidecarProcessIdentity {
        pid: child.pid(),
        generation: state.next_generation.fetch_add(1, Ordering::AcqRel) + 1,
    };
    match state.process.lock() {
        Ok(mut process) => {
            if process.current.is_some() {
                eprintln!("[sidecar] refused to replace a running process");
                let _ = child.kill();
                return None;
            }
            process.current = Some(identity);
            process.child = Some(child);
        }
        Err(error) => {
            eprintln!("[sidecar] failed to store the process handle: {error}");
            let _ = child.kill();
            return None;
        }
    }
    if state.shutdown_requested.load(Ordering::Acquire) {
        kill_sidecar(app);
        return None;
    }
    monitor_sidecar_events(app.clone(), events, config, identity);
    Some(identity)
}

fn monitor_sidecar_events(
    app: AppHandle,
    mut events: tauri::async_runtime::Receiver<CommandEvent>,
    config: SidecarLaunchConfig,
    identity: SidecarProcessIdentity,
) {
    tauri::async_runtime::spawn(async move {
        let mut received_termination = false;
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stdout(line) => {
                    println!("[sidecar] {}", String::from_utf8_lossy(&line));
                }
                CommandEvent::Stderr(line) => {
                    eprintln!("[sidecar] {}", String::from_utf8_lossy(&line));
                }
                CommandEvent::Terminated(status) => {
                    println!("[sidecar] exited: {:?}", status.code);
                    received_termination = true;
                    break;
                }
                _ => {}
            }
        }

        let state = app.state::<SidecarState>();
        let (child, was_current) = state
            .process
            .lock()
            .map(|mut process| {
                if process.current == Some(identity) {
                    process.current = None;
                    state.replacement_ready.store(false, Ordering::Release);
                    (process.child.take(), true)
                } else {
                    (None, false)
                }
            })
            .unwrap_or((None, false));
        if !received_termination {
            if let Some(child) = child {
                let _ = child.kill();
            }
            eprintln!("[sidecar] event stream closed before termination");
        }
        record_sidecar_termination(state.inner(), identity, received_termination);
        if was_current {
            handle_sidecar_exit(app, config);
        }
    });
}

fn handle_sidecar_exit(app: AppHandle, config: SidecarLaunchConfig) {
    let state = app.state::<SidecarState>();
    state.replacement_ready.store(false, Ordering::Release);
    let shutdown_requested = state.shutdown_requested.load(Ordering::Acquire);
    let completed_restarts = state.restart_attempts.load(Ordering::Acquire);
    match sidecar_exit_action(shutdown_requested, completed_restarts) {
        SidecarExitAction::Stop => {}
        SidecarExitAction::NotifyFailure => {
            state.permanently_unavailable.store(true, Ordering::Release);
            let message = "后台服务已停止，自动重启失败。请重启应用。";
            eprintln!("[sidecar] {message}");
            if let Err(error) = show_main_window(&app) {
                eprintln!("[sidecar] failed to show the unavailable service: {error}");
            }
            if let Err(error) = app.emit("desktop://sidecarUnavailable", message) {
                eprintln!("[sidecar] failed to report the unavailable service: {error}");
            }
        }
        SidecarExitAction::Restart(delay) => {
            state.restart_attempts.fetch_add(1, Ordering::AcqRel);
            tauri::async_runtime::spawn(async move {
                let _ = tauri::async_runtime::spawn_blocking(move || thread::sleep(delay)).await;
                if app
                    .state::<SidecarState>()
                    .shutdown_requested
                    .load(Ordering::Acquire)
                {
                    return;
                }

                let mut restarted_config = config.clone();
                restarted_config.health_token = match generate_sidecar_health_token() {
                    Ok(token) => token,
                    Err(error) => {
                        eprintln!("[sidecar] restart token generation failed: {error}");
                        handle_sidecar_exit(app, config);
                        return;
                    }
                };
                let (events, child) = match start_sidecar(
                    &app,
                    &restarted_config.health_token,
                    restarted_config.upstream_proxy.as_deref(),
                ) {
                    Ok(process) => process,
                    Err(error) => {
                        eprintln!("[sidecar] restart failed to spawn: {error}");
                        handle_sidecar_exit(app, config);
                        return;
                    }
                };
                let Some(identity) =
                    install_sidecar_process(&app, child, events, restarted_config.clone())
                else {
                    return;
                };

                let ready_port = restarted_config.ready_port;
                let health_token = restarted_config.health_token.clone();
                let health = tauri::async_runtime::spawn_blocking(move || {
                    wait_for_sidecar(ready_port, &health_token, Duration::from_secs(15))
                })
                .await;
                match health {
                    Ok(Ok(())) => {
                        let state = app.state::<SidecarState>();
                        if mark_replacement_ready_if_current(state.inner(), identity) {
                            println!("[sidecar] restart passed the health check");
                            schedule_stable_restart_budget_reset(app.clone(), identity);
                        } else {
                            eprintln!(
                                "[sidecar] ignored a stale health result for pid {}",
                                identity.pid
                            );
                        }
                    }
                    Ok(Err(error)) => {
                        eprintln!("[sidecar] restarted process is unhealthy: {error}");
                        kill_sidecar_if_current(&app, identity);
                    }
                    Err(error) => {
                        eprintln!("[sidecar] restart health task failed: {error}");
                        kill_sidecar_if_current(&app, identity);
                    }
                }
            });
        }
    }
}

fn should_reset_restart_budget(
    ready: bool,
    current: Option<SidecarProcessIdentity>,
    expected: SidecarProcessIdentity,
) -> bool {
    ready && current == Some(expected)
}

fn schedule_stable_restart_budget_reset(app: AppHandle, expected: SidecarProcessIdentity) {
    tauri::async_runtime::spawn(async move {
        let _ =
            tauri::async_runtime::spawn_blocking(|| thread::sleep(Duration::from_secs(30))).await;
        let state = app.state::<SidecarState>();
        if state.shutdown_requested.load(Ordering::Acquire) {
            return;
        }
        let current = state
            .process
            .lock()
            .ok()
            .and_then(|process| process.current);
        if should_reset_restart_budget(
            state.replacement_ready.load(Ordering::Acquire),
            current,
            expected,
        ) {
            state.restart_attempts.store(0, Ordering::Release);
        }
    });
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn emit_maximized_state(app: &AppHandle, window: &WebviewWindow) {
    match window.is_maximized() {
        Ok(maximized) => {
            if let Err(error) = app.emit("desktop://isMaximized", maximized) {
                eprintln!("[tauri] failed to sync the maximized state: {error}");
            }
        }
        Err(error) => eprintln!("[tauri] failed to read the maximized state: {error}"),
    }
}

fn create_main_window(
    app: &tauri::App,
    proxy_relay_enabled: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let port = if cfg!(debug_assertions) {
        DEV_WEB_PORT
    } else {
        RELEASE_WEB_PORT
    };
    let preferences = app
        .state::<WindowPreferencesState>()
        .preferences
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    let size = preferences.size.unwrap_or(WindowSize {
        width: 1_440,
        height: 840,
    });
    let url = format!("http://127.0.0.1:{port}").parse()?;
    let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title("YesPlayMusic")
        .inner_size(f64::from(size.width), f64::from(size.height))
        .min_inner_size(300.0, 48.0)
        .visible(false);
    if proxy_relay_enabled {
        let proxy = format!("http://127.0.0.1:{WEBVIEW_PROXY_RELAY_PORT}").parse()?;
        builder = builder.proxy_url(proxy);
    }
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    #[cfg(target_os = "windows")]
    let builder = builder.decorations(false);
    let window = builder
        .on_page_load(|_, payload| {
            if payload.event() == PageLoadEvent::Finished {
                println!("[tauri] webview-ready:");
            }
        })
        .build()?;
    if let Some(position) = preferences.position {
        window.set_position(PhysicalPosition::new(position.x, position.y))?;
    }
    ensure_main_window_reachable(&window)?;
    persist_current_window_position(app.handle())?;
    apply_always_on_top(&window, preferences.always_on_top)?;

    let window_for_events = window.clone();
    window.on_window_event(move |event| {
        let app = window_for_events.app_handle();
        match event {
            WindowEvent::Resized(_) => {
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                emit_maximized_state(app, &window_for_events);
            }
            WindowEvent::Moved(position) => {
                let normal = !window_for_events.is_minimized().unwrap_or(true)
                    && !window_for_events.is_maximized().unwrap_or(true)
                    && !window_for_events.is_fullscreen().unwrap_or(true);
                if normal {
                    schedule_window_position_save(
                        app,
                        WindowPosition {
                            x: position.x,
                            y: position.y,
                        },
                    );
                }
            }
            WindowEvent::CloseRequested { api, .. } => {
                let option = app
                    .state::<DesktopPreferencesState>()
                    .0
                    .lock()
                    .map(|preferences| preferences.close_app_option)
                    .unwrap_or(CloseAppOption::MinimizeToTray);
                match close_decision(cfg!(target_os = "macos"), option) {
                    CloseDecision::Exit => app.exit(0),
                    CloseDecision::Hide | CloseDecision::MinimizeToTray => {
                        if can_hide_main_window(app) {
                            api.prevent_close();
                            if let Err(error) = hide_main_window(app) {
                                eprintln!("[tauri] failed to hide the main window: {error}");
                            }
                        } else {
                            app.exit(0);
                        }
                    }
                    CloseDecision::Ask => {
                        api.prevent_close();
                        app.state::<ClosePromptState>()
                            .0
                            .store(true, Ordering::Release);
                        if let Err(error) = app.emit("desktop://requestCloseChoice", ()) {
                            app.state::<ClosePromptState>()
                                .0
                                .store(false, Ordering::Release);
                            eprintln!("[tauri] failed to request a close choice: {error}");
                        }
                    }
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                let is_auto = app
                    .state::<DesktopPreferencesState>()
                    .0
                    .lock()
                    .map(|preferences| preferences.tray_icon_theme == TrayIconTheme::Auto)
                    .unwrap_or(false);
                if is_auto {
                    if let Err(error) = update_tray_icon(app, *theme) {
                        eprintln!("[tauri] failed to update the tray theme: {error}");
                    }
                }
            }
            _ => {}
        }
    });
    Ok(())
}

fn main() {
    let mut updater = tauri_plugin_updater::Builder::new().target(updater_target());
    if let Some(public_key) = updater_public_key() {
        updater = updater.pubkey(public_key);
    }

    let context = tauri::generate_context!();
    #[cfg(target_os = "macos")]
    let single_instance_socket = single_instance_socket_path(&context.config().identifier);
    #[cfg(target_os = "macos")]
    let startup_gate = acquire_macos_startup_gate(
        &startup_gate_path(&context.config().identifier),
        SINGLE_INSTANCE_STARTUP_TIMEOUT,
    )
    .expect("failed to serialize macOS cold startup");

    let app = tauri::Builder::default()
        .menu(create_app_menu)
        .on_menu_event(|app, event| handle_app_menu_event(app, event.id().as_ref()))
        // Register single-instance handling before a second process can bind sidecar ports.
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            if single_instance_notification_is_probe(&args, &cwd) {
                return;
            }
            if let Err(error) = show_main_window(app) {
                eprintln!("[tauri] failed to restore the main window: {error}");
            }
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let action = app
                        .state::<GlobalShortcutRegistrationState>()
                        .0
                        .lock()
                        .ok()
                        .and_then(|registration| registration.actions.get(&shortcut.id()).cloned());
                    match action.as_deref() {
                        Some("minimize") => {
                            if let Some(window) = app.get_webview_window("main") {
                                if window.is_visible().unwrap_or(false) {
                                    let _ = window.hide();
                                } else {
                                    let _ = show_main_window(app);
                                }
                                let _ = render_tray_title(app);
                            }
                        }
                        Some(action) => emit_desktop_event(app, action),
                        None => {}
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(updater.build())
        // Skip ambiguous physical-pixel restore across mixed-DPI displays.
        // The renderer restores logical size while the plugin still persists on exit.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .skip_initial_state("main")
                .build(),
        )
        .setup(|app| {
            let window_preferences = load_initial_window_preferences(app.handle());
            app.manage(WindowPreferencesState {
                preferences: Mutex::new(window_preferences),
                movement: Mutex::new(PendingWindowMovement::default()),
            });
            app.manage(StartupWindowState(AtomicBool::new(false)));
            app.manage(GlobalShortcutRegistrationState(Mutex::new(
                GlobalShortcutRegistration::default(),
            )));
            app.manage(DesktopPreferencesState(Mutex::new(
                DesktopPreferences::default(),
            )));
            app.manage(ClosePromptState(AtomicBool::new(false)));
            app.manage(TrayMenuState(Mutex::new(TrayMenuRegistration::default())));
            app.manage(TrayAvailabilityState(AtomicBool::new(false)));
            app.manage(TrayCoverState::default());
            app.manage(TrayTitleState::default());
            app.manage(DiscordPresenceHandle::default());
            #[cfg(target_os = "linux")]
            {
                let handle = app.handle().clone();
                let media =
                    LinuxMedia::start(move |control| handle_linux_media_control(&handle, control))
                        .map_err(|error| {
                            eprintln!("[tauri] Linux media integration unavailable: {error}");
                            error
                        })
                        .ok();
                app.manage(LinuxMediaState(media));
            }
            match read_legacy_electron_config(app.handle()) {
                Ok(Some(config)) => {
                    if let Err(error) = migrate_legacy_webview_proxy(app.handle(), &config) {
                        eprintln!("[tauri] ignored an invalid legacy proxy: {error}");
                    }
                }
                Ok(None) => {}
                Err(error) => eprintln!("[tauri] could not read the legacy proxy: {error}"),
            }
            let upstream_proxy = match load_webview_proxy(app.handle()) {
                Ok(proxy) => proxy,
                Err(error) => {
                    eprintln!("[tauri] ignored an invalid saved proxy: {error}");
                    None
                }
            };
            let proxy_relay_enabled = upstream_proxy.is_some();
            let health_token = generate_sidecar_health_token()?;
            let ready_port = if cfg!(debug_assertions) {
                API_PORT
            } else {
                RELEASE_WEB_PORT
            };
            let core_smoke_test = is_smoke_test(env::args());
            let webview_smoke_test = is_webview_smoke_test(env::args());
            let show_startup_error_dialog = !core_smoke_test && !webview_smoke_test;
            let sidecar_config = SidecarLaunchConfig {
                health_token,
                upstream_proxy: upstream_proxy.map(|proxy| proxy.to_string()),
                ready_port,
            };
            app.manage(SidecarState {
                process: Mutex::new(SidecarProcessSlot::default()),
                next_generation: AtomicUsize::new(0),
                termination_wait: Mutex::new(SidecarTerminationWait::default()),
                termination_changed: Condvar::new(),
                shutdown_requested: AtomicBool::new(false),
                restart_attempts: AtomicUsize::new(0),
                replacement_ready: AtomicBool::new(false),
                permanently_unavailable: AtomicBool::new(false),
            });
            let (events, child) = match start_sidecar(
                app.handle(),
                &sidecar_config.health_token,
                sidecar_config.upstream_proxy.as_deref(),
            ) {
                Ok(process) => process,
                Err(error) => {
                    return handle_sidecar_startup_failure(
                        app.handle(),
                        ready_port,
                        error,
                        show_startup_error_dialog,
                    );
                }
            };
            if install_sidecar_process(app.handle(), child, events, sidecar_config.clone())
                .is_none()
            {
                return handle_sidecar_startup_failure(
                    app.handle(),
                    ready_port,
                    "无法管理 Sidecar 进程",
                    show_startup_error_dialog,
                );
            }
            let sidecar_state = app.state::<SidecarState>();
            if let Err(error) = wait_for_supervised_sidecar(
                ready_port,
                &sidecar_config.health_token,
                &sidecar_state.replacement_ready,
                &sidecar_state.permanently_unavailable,
                Duration::from_secs(20),
            ) {
                sidecar_state
                    .shutdown_requested
                    .store(true, Ordering::Release);
                kill_sidecar(app.handle());
                return handle_sidecar_startup_failure(
                    app.handle(),
                    ready_port,
                    error,
                    show_startup_error_dialog,
                );
            }
            println!(
                "[tauri] ready: pid={}, port={ready_port}",
                std::process::id()
            );

            if core_smoke_test || webview_smoke_test {
                // Hidden smoke checks avoid the Dock and focus without affecting normal launches.
                #[cfg(target_os = "macos")]
                let _ = app
                    .handle()
                    .set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            if core_smoke_test {
                // Headless checks validate the backend without creating or focusing a window.
                let handle = app.handle().clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(12));
                    handle.exit(0);
                });
            } else if webview_smoke_test {
                create_main_window(app, proxy_relay_enabled)?;
                let handle = app.handle().clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(25));
                    handle.exit(0);
                });
            } else {
                if let Err(error) = create_tray(app) {
                    eprintln!("[tauri] tray integration unavailable; continuing: {error}");
                }
                #[cfg(target_os = "macos")]
                spawn_tray_title_reconciler(app.handle());
                app.state::<StartupWindowState>()
                    .0
                    .store(true, Ordering::Release);
                create_main_window(app, proxy_relay_enabled)?;
                schedule_startup_show_fallback(app.handle());
                #[cfg(target_os = "macos")]
                macos_media_controls::install(app.handle()).map_err(std::io::Error::other)?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            desktop_event,
            is_always_on_top,
            toggle_always_on_top,
            renderer_ready,
            restore_compact_window,
            read_legacy_settings,
            legacy_renderer_data::import_legacy_renderer_data,
            updater_configured,
            prepare_for_update
        ])
        .build(context)
        .expect("failed to build Tauri application");

    #[cfg(target_os = "macos")]
    {
        wait_for_single_instance_listener(&single_instance_socket, SINGLE_INSTANCE_STARTUP_TIMEOUT)
            .expect("single-instance listener did not become ready");
        drop(startup_gate);
    }

    app.run(|app, event| match event {
        RunEvent::Exit => {
            if let Err(error) = persist_current_window_position(app) {
                eprintln!("[tauri] failed to flush the window position: {error}");
            }
            #[cfg(target_os = "linux")]
            if let Some(media) = app
                .try_state::<LinuxMediaState>()
                .and_then(|state| state.0.clone())
            {
                media.shutdown();
            }
            if let Some(state) = app.try_state::<SidecarState>() {
                state.shutdown_requested.store(true, Ordering::Release);
            }
            let _ = stop_sidecar_gracefully(app);
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            if let Err(error) = show_main_window(app) {
                eprintln!("[tauri] failed to reopen the main window: {error}");
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::{
        app_about_metadata, app_menu_action, claim_startup_show, clear_sidecar_termination_wait,
        decode_tray_cover, is_smoke_test, is_webview_smoke_test, mark_replacement_ready_if_current,
        normalize_global_shortcut, parse_legacy_settings, prepare_sidecar_termination_wait,
        record_sidecar_termination, response_has_sidecar_identity, should_reset_restart_budget,
        sidecar_exit_action, sidecar_startup_error_message, tray_cover_url,
        tray_recovery_available, tray_title_for_visibility, wait_for_sidecar_termination,
        wait_for_supervised_sidecar, window_frame_has_reachable_area, AppMenuAction,
        SidecarExitAction, SidecarProcessIdentity, SidecarProcessSlot, SidecarState,
        SidecarTerminationWait,
    };
    use std::{
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Condvar, Mutex,
        },
        thread,
        time::Duration,
    };
    use tauri_plugin_global_shortcut::Shortcut;

    fn sidecar_state(current: Option<SidecarProcessIdentity>) -> SidecarState {
        SidecarState {
            process: Mutex::new(SidecarProcessSlot {
                child: None,
                current,
            }),
            next_generation: AtomicUsize::new(0),
            termination_wait: Mutex::new(SidecarTerminationWait::default()),
            termination_changed: Condvar::new(),
            shutdown_requested: AtomicBool::new(false),
            restart_attempts: AtomicUsize::new(0),
            replacement_ready: AtomicBool::new(false),
            permanently_unavailable: AtomicBool::new(false),
        }
    }

    #[test]
    fn smoke_test_must_be_explicit() {
        assert!(is_smoke_test(["yesplaymusic-tauri", "--smoke-test"]));
        assert!(!is_smoke_test(["yesplaymusic-tauri"]));
    }

    #[test]
    fn webview_smoke_test_must_be_explicit() {
        assert!(is_webview_smoke_test([
            "yesplaymusic-tauri",
            "--webview-smoke-test"
        ]));
        assert!(!is_webview_smoke_test([
            "yesplaymusic-tauri",
            "--smoke-test"
        ]));
    }

    #[test]
    fn startup_window_is_claimed_exactly_once() {
        let pending = AtomicBool::new(false);
        assert!(!claim_startup_show(&pending));
        pending.store(true, Ordering::Release);
        assert!(claim_startup_show(&pending));
        assert!(!claim_startup_show(&pending));
    }

    #[test]
    fn startup_error_explains_how_to_recover_from_an_occupied_port() {
        let message = sidecar_startup_error_message(28_232, "health check timed out");

        assert!(message.contains("28232"));
        assert!(message.contains("其他 YesPlayMusic 实例"));
        assert!(message.contains("health check timed out"));
    }

    #[test]
    fn only_macos_can_recover_a_hidden_window_without_a_tray() {
        assert!(tray_recovery_available(true, false));
        assert!(tray_recovery_available(false, true));
        assert!(!tray_recovery_available(false, false));
    }

    #[test]
    fn app_about_identifies_the_tauri_rebuild() {
        let version = env!("CARGO_PKG_VERSION");
        let metadata = app_about_metadata(version);

        assert_eq!(metadata.name.as_deref(), Some("YesPlayMusic"));
        assert_eq!(metadata.version, None);
        assert_eq!(metadata.short_version.as_deref(), Some(version));
        assert_eq!(
            metadata.credits.as_deref(),
            Some("Tauri 2 跨平台版\n由 Nagi Studio 独立维护")
        );
        assert_eq!(
            metadata.copyright.as_deref(),
            Some("基于 qier222/YesPlayMusic 的开源工作重构")
        );
    }

    #[test]
    fn application_menu_routes_every_custom_action() {
        for (id, action) in [
            ("app.preferences", AppMenuAction::Navigate("/settings")),
            ("app.search", AppMenuAction::Emit("search")),
            ("app.play", AppMenuAction::Emit("play")),
            ("app.next", AppMenuAction::Emit("next")),
            ("app.previous", AppMenuAction::Emit("previous")),
            ("app.increaseVolume", AppMenuAction::Emit("increaseVolume")),
            ("app.decreaseVolume", AppMenuAction::Emit("decreaseVolume")),
            ("app.like", AppMenuAction::Emit("like")),
            ("app.repeat", AppMenuAction::Emit("repeat")),
            ("app.shuffle", AppMenuAction::Emit("shuffle")),
            ("app.minimizeToTray", AppMenuAction::Hide),
            (
                "app.github",
                AppMenuAction::OpenUrl("https://github.com/nagi-studio/YesPlayMusic"),
            ),
            ("app.tauri", AppMenuAction::OpenUrl("https://tauri.app")),
            ("app.delete", AppMenuAction::DeleteSelection),
            ("app.startSpeaking", AppMenuAction::StartSpeaking),
            ("app.stopSpeaking", AppMenuAction::StopSpeaking),
            ("app.reload", AppMenuAction::Reload),
            ("app.forceReload", AppMenuAction::Reload),
            ("app.toggleFullscreen", AppMenuAction::ToggleFullscreen),
            ("app.toggleDevtools", AppMenuAction::ToggleDevtools),
        ] {
            assert_eq!(app_menu_action(id), Some(action), "unmapped menu ID: {id}");
        }
        assert_eq!(app_menu_action("unexpected"), None);
    }

    #[test]
    fn occupied_port_must_answer_with_the_sidecar_identity() {
        let expected_token = "a".repeat(64);
        let valid = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: application/json\r\n",
            "X-YesPlayMusic-Health-Token: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n\r\n",
            "{\"service\":\"yesplaymusic-sidecar\",\"protocol\":1}"
        );
        let replayed = concat!(
            "HTTP/1.1 200 OK\r\n",
            "X-YesPlayMusic-Health-Token: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\r\n\r\n",
            "{\"service\":\"yesplaymusic-sidecar\",\"protocol\":1}"
        );
        let unrelated = "HTTP/1.1 200 OK\r\n\r\n{\"service\":\"other-app\"}";

        assert!(response_has_sidecar_identity(valid, &expected_token));
        assert!(!response_has_sidecar_identity(replayed, &expected_token));
        assert!(!response_has_sidecar_identity(unrelated, &expected_token));
        assert!(!response_has_sidecar_identity(
            "HTTP/1.1 404 Not Found\r\n\r\n",
            &expected_token
        ));
    }

    #[test]
    fn sidecar_exit_sequence_is_bounded_and_shutdown_never_restarts() {
        assert_eq!(sidecar_exit_action(true, 0), SidecarExitAction::Stop);
        assert_eq!(
            (0..=3)
                .map(|completed| sidecar_exit_action(false, completed))
                .collect::<Vec<_>>(),
            vec![
                SidecarExitAction::Restart(std::time::Duration::from_millis(500)),
                SidecarExitAction::Restart(std::time::Duration::from_secs(1)),
                SidecarExitAction::Restart(std::time::Duration::from_secs(2)),
                SidecarExitAction::NotifyFailure,
            ]
        );
    }

    #[test]
    fn startup_accepts_a_healthy_replacement_sidecar() {
        let replacement_ready = AtomicBool::new(true);
        let permanently_unavailable = AtomicBool::new(false);

        assert!(wait_for_supervised_sidecar(
            0,
            "dead-initial-token",
            &replacement_ready,
            &permanently_unavailable,
            std::time::Duration::from_millis(50),
        )
        .is_ok());
    }

    #[test]
    fn permanent_failure_wins_over_a_previous_ready_signal() {
        let replacement_ready = AtomicBool::new(true);
        let permanently_unavailable = AtomicBool::new(true);

        assert!(wait_for_supervised_sidecar(
            0,
            "dead-initial-token",
            &replacement_ready,
            &permanently_unavailable,
            Duration::from_millis(50),
        )
        .is_err());
    }

    #[test]
    fn replacement_health_only_marks_the_current_live_generation_ready() {
        let current = SidecarProcessIdentity {
            pid: 42,
            generation: 2,
        };
        let stale = SidecarProcessIdentity {
            pid: 42,
            generation: 1,
        };
        let state = sidecar_state(Some(current));

        assert!(!mark_replacement_ready_if_current(&state, stale));
        assert!(!state.replacement_ready.load(Ordering::Acquire));
        assert!(mark_replacement_ready_if_current(&state, current));

        state.replacement_ready.store(false, Ordering::Release);
        state.permanently_unavailable.store(true, Ordering::Release);
        assert!(!mark_replacement_ready_if_current(&state, current));
        assert!(!state.replacement_ready.load(Ordering::Acquire));
    }

    #[test]
    fn graceful_shutdown_waits_only_for_a_confirmed_matching_termination() {
        let state = Arc::new(sidecar_state(None));
        let expected = SidecarProcessIdentity {
            pid: 42,
            generation: 3,
        };
        prepare_sidecar_termination_wait(&state, expected);

        record_sidecar_termination(&state, expected, false);
        assert!(!wait_for_sidecar_termination(
            &state,
            expected,
            Duration::from_millis(10)
        ));

        let notifier = Arc::clone(&state);
        let thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            record_sidecar_termination(&notifier, expected, true);
        });

        assert!(wait_for_sidecar_termination(
            &state,
            expected,
            Duration::from_secs(1)
        ));
        assert!(!wait_for_sidecar_termination(
            &state,
            SidecarProcessIdentity {
                pid: 42,
                generation: 4,
            },
            Duration::from_millis(10)
        ));
        thread.join().unwrap();

        clear_sidecar_termination_wait(&state, expected);
        assert!(state
            .termination_wait
            .lock()
            .is_ok_and(|wait| wait.expected.is_none() && !wait.terminated));
    }

    #[test]
    fn stable_generation_resets_only_its_own_restart_budget() {
        let expected = SidecarProcessIdentity {
            pid: 42,
            generation: 2,
        };
        assert!(should_reset_restart_budget(true, Some(expected), expected));
        assert!(!should_reset_restart_budget(
            false,
            Some(expected),
            expected
        ));
        assert!(!should_reset_restart_budget(
            true,
            Some(SidecarProcessIdentity {
                pid: 42,
                generation: 3
            }),
            expected
        ));
    }

    #[test]
    fn restored_window_must_have_a_reachable_area() {
        let monitors = [(0, 0, 2560, 1410), (5120, 0, 3024, 1900)];
        assert!(window_frame_has_reachable_area(
            (837, 30, 920, 620),
            &monitors
        ));
        assert!(!window_frame_has_reachable_area(
            (8064, 100, 3812, 268),
            &monitors
        ));
        assert!(!window_frame_has_reachable_area(
            (8080, 100, 500, 200),
            &monitors
        ));
    }

    #[test]
    fn default_shortcuts_can_be_parsed_by_tauri() {
        for accelerator in [
            "Alt+CommandOrControl+P",
            "Alt+CommandOrControl+Right",
            "Alt+CommandOrControl+Left",
            "Alt+CommandOrControl+Up",
            "Alt+CommandOrControl+Down",
            "Alt+CommandOrControl+L",
            "Alt+CommandOrControl+M",
        ] {
            let normalized = normalize_global_shortcut(accelerator);
            assert!(
                normalized.parse::<Shortcut>().is_ok(),
                "Tauri 无法解析 {accelerator}（转换后为 {normalized}）"
            );
        }
    }

    #[test]
    fn legacy_config_only_exposes_settings() {
        let settings =
            parse_legacy_settings(r#"{"settings":{"lang":"zh-CN"},"window":{"width":1440}}"#)
                .unwrap()
                .unwrap();
        assert_eq!(settings["lang"], "zh-CN");
        assert!(settings.get("window").is_none());
    }

    #[test]
    fn legacy_config_rejects_non_object_schemas() {
        assert!(parse_legacy_settings("[]").is_err());
        assert!(parse_legacy_settings(r#"{"settings":[]}"#).is_err());
        assert!(parse_legacy_settings(r#"{"settings":null}"#).is_err());
    }

    #[test]
    fn now_playing_payload_exposes_cover_url() {
        let payload = serde_json::json!({
            "title": "雨爱",
            "coverUrl": "https://example.com/cover.jpg?param=64y64"
        });

        assert_eq!(
            tray_cover_url(&payload),
            Some("https://example.com/cover.jpg?param=64y64")
        );
        assert_eq!(tray_cover_url(&serde_json::json!({ "coverUrl": "" })), None);
    }

    #[test]
    fn tray_title_only_appears_while_player_window_is_hidden() {
        assert_eq!(tray_title_for_visibility(" 正在播放 ", true), "");
        assert_eq!(tray_title_for_visibility(" 正在播放 ", false), "正在播放");
        assert_eq!(
            tray_title_for_visibility(&"a".repeat(45), false),
            format!("{}…", "a".repeat(44))
        );
        assert_eq!(
            tray_title_for_visibility(&"歌".repeat(23), false),
            format!("{}…", "歌".repeat(22))
        );
    }

    #[test]
    fn jpeg_cover_is_decoded_to_a_small_square_tray_icon() {
        let mut encoded = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut encoded)
            .encode(&[33, 66, 99], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();

        let icon = decode_tray_cover(&encoded).unwrap();
        assert_eq!((icon.width(), icon.height()), (64, 64));
        assert_eq!(icon.rgba().len(), 64 * 64 * 4);
    }
}
