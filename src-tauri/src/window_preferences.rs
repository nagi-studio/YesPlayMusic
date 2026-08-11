use std::{
    fs,
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tauri::Manager;

const FILE_NAME: &str = "window-preferences.json";
const FILE_VERSION: u8 = 1;
const MAX_PREFERENCE_BYTES: u64 = 4_096;
const MAX_LEGACY_CONFIG_BYTES: u64 = 1_048_576;
const MIN_WINDOW_WIDTH: u32 = 300;
const MIN_WINDOW_HEIGHT: u32 = 48;
const MAX_WINDOW_EDGE: u32 = 8_192;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WindowPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WindowPreferences {
    version: u8,
    pub always_on_top: bool,
    #[serde(default)]
    pub position: Option<WindowPosition>,
    #[serde(default)]
    pub size: Option<WindowSize>,
}

impl Default for WindowPreferences {
    fn default() -> Self {
        Self {
            version: FILE_VERSION,
            always_on_top: false,
            position: None,
            size: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyWindowFrame {
    x: i32,
    y: i32,
    #[serde(default)]
    #[serde(rename = "width")]
    _width: Option<u32>,
    #[serde(default)]
    #[serde(rename = "height")]
    _height: Option<u32>,
    #[serde(default, rename = "alwaysOnTop")]
    always_on_top: bool,
}

pub fn load<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Option<WindowPreferences>, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?;
    load_from_dir(&config_dir).map_err(|error| error.to_string())
}

pub fn save<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    preferences: &WindowPreferences,
) -> Result<(), String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?;
    save_to_dir(&config_dir, preferences).map_err(|error| error.to_string())
}

pub fn legacy_electron_config_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<PathBuf, String> {
    app.path()
        .config_dir()
        .map(|directory| legacy_electron_config_path_in(&directory))
        .map_err(|error| error.to_string())
}

pub fn read_legacy_electron_config<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Option<Vec<u8>>, String> {
    read_legacy_config_from_path(&legacy_electron_config_path(app)?)
        .map_err(|error| error.to_string())
}

pub fn load_legacy_preferences<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Option<WindowPreferences>, String> {
    load_legacy_preferences_from_path(&legacy_electron_config_path(app)?)
        .map_err(|error| error.to_string())
}

pub fn with_legacy_fallback(
    native: Option<WindowPreferences>,
    legacy: Option<WindowPreferences>,
) -> WindowPreferences {
    native.or(legacy).unwrap_or_default()
}

fn preference_file(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE_NAME)
}

fn legacy_electron_config_path_in(config_dir: &Path) -> PathBuf {
    config_dir.join("yesplaymusic").join("config.json")
}

fn load_from_dir(config_dir: &Path) -> Result<Option<WindowPreferences>, io::Error> {
    let bytes = match fs::read(preference_file(config_dir)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if bytes.len() as u64 > MAX_PREFERENCE_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "window preference file is too large",
        ));
    }
    let preference: WindowPreferences = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
    if preference.version != FILE_VERSION {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "window preference version is unsupported",
        ));
    }
    if preference.size.is_some_and(|size| !valid_window_size(size)) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "window preference size is outside supported bounds",
        ));
    }
    Ok(Some(preference))
}

fn save_to_dir(config_dir: &Path, preferences: &WindowPreferences) -> Result<(), io::Error> {
    fs::create_dir_all(config_dir)?;
    let bytes = serde_json::to_vec(preferences).map_err(io::Error::other)?;
    let mut temporary = tempfile::NamedTempFile::new_in(config_dir)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(preference_file(config_dir))
        .map_err(|error| error.error)?;
    sync_directory(config_dir)
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), io::Error> {
    fs::File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> Result<(), io::Error> {
    Ok(())
}

fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(target_os = "windows"))]
    false
}

fn read_legacy_config_from_path(path: &Path) -> Result<Option<Vec<u8>>, io::Error> {
    let Some(directory) = path.parent() else {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "legacy config path has no parent",
        ));
    };
    let directory_metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if is_link_or_reparse_point(&directory_metadata) || !directory_metadata.is_dir() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "legacy config directory must be a real directory",
        ));
    }

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "legacy config must be a regular file",
        ));
    }
    if metadata.len() > MAX_LEGACY_CONFIG_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "legacy preference file is too large",
        ));
    }

    let file = fs::File::open(path)?;
    let opened_metadata = file.metadata()?;
    let current_metadata = fs::symlink_metadata(path)?;
    if !opened_metadata.is_file() || is_link_or_reparse_point(&current_metadata) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "legacy config changed while opening",
        ));
    }
    if opened_metadata.len() > MAX_LEGACY_CONFIG_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "legacy preference file is too large",
        ));
    }

    let mut bytes = Vec::with_capacity(opened_metadata.len() as usize);
    file.take(MAX_LEGACY_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LEGACY_CONFIG_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "legacy preference file is too large",
        ));
    }
    Ok(Some(bytes))
}

fn valid_window_size(size: WindowSize) -> bool {
    size.width >= MIN_WINDOW_WIDTH
        && size.height >= MIN_WINDOW_HEIGHT
        && size.width <= MAX_WINDOW_EDGE
        && size.height <= MAX_WINDOW_EDGE
}

fn load_legacy_preferences_from_path(path: &Path) -> Result<Option<WindowPreferences>, io::Error> {
    let Some(bytes) = read_legacy_config_from_path(path)? else {
        return Ok(None);
    };
    let root: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
    let Some(window) = root.get("window") else {
        return Ok(None);
    };
    let frame: LegacyWindowFrame = serde_json::from_value(window.clone())
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
    let size = match (frame._width, frame._height) {
        (Some(width), Some(height)) => {
            let size = WindowSize { width, height };
            if !valid_window_size(size) {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "legacy window size is outside supported bounds",
                ));
            }
            Some(size)
        }
        (None, None) => None,
        _ => {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "legacy window size must contain both width and height",
            ));
        }
    };
    Ok(Some(WindowPreferences {
        always_on_top: frame.always_on_top,
        position: Some(WindowPosition {
            x: frame.x,
            y: frame.y,
        }),
        size,
        ..WindowPreferences::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "yesplaymusic-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn preferences_round_trip_through_atomic_replacement() {
        let directory = temporary_directory("window-preference");
        let mut preferences = WindowPreferences {
            always_on_top: true,
            position: Some(WindowPosition { x: 120, y: -40 }),
            ..WindowPreferences::default()
        };
        save_to_dir(&directory, &preferences).unwrap();
        assert_eq!(
            load_from_dir(&directory).unwrap(),
            Some(preferences.clone())
        );

        preferences.position = Some(WindowPosition { x: 640, y: 220 });
        save_to_dir(&directory, &preferences).unwrap();
        assert_eq!(load_from_dir(&directory).unwrap(), Some(preferences));
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_preference_has_no_native_value() {
        let directory = temporary_directory("missing-window-preference");
        assert_eq!(load_from_dir(&directory).unwrap(), None);
    }

    #[test]
    fn malformed_preference_is_rejected() {
        let directory = temporary_directory("invalid-window-preference");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            preference_file(&directory),
            br#"{"version":1,"always_on_top":true,"extra":true}"#,
        )
        .unwrap();
        assert_eq!(
            load_from_dir(&directory).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_one_without_position_remains_compatible() {
        let directory = temporary_directory("old-window-preference");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            preference_file(&directory),
            br#"{"version":1,"always_on_top":true}"#,
        )
        .unwrap();
        assert_eq!(
            load_from_dir(&directory).unwrap(),
            Some(WindowPreferences {
                always_on_top: true,
                ..WindowPreferences::default()
            })
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_window_import_accepts_position_and_size() {
        let directory = temporary_directory("legacy-window-position");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.json");
        fs::write(
            &path,
            br#"{"settings":{"lang":"zh-CN"},"window":{"x":80,"y":120,"width":1440,"height":840}}"#,
        )
        .unwrap();
        assert_eq!(
            load_legacy_preferences_from_path(&path)
                .unwrap()
                .and_then(|preferences| preferences.position),
            Some(WindowPosition { x: 80, y: 120 })
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_window_imports_size_and_always_on_top() {
        let directory = temporary_directory("legacy-window-state");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.json");
        fs::write(
            &path,
            br#"{"window":{"x":80,"y":120,"width":1024,"height":640,"alwaysOnTop":true}}"#,
        )
        .unwrap();

        assert_eq!(
            load_legacy_preferences_from_path(&path).unwrap(),
            Some(WindowPreferences {
                always_on_top: true,
                position: Some(WindowPosition { x: 80, y: 120 }),
                size: Some(WindowSize {
                    width: 1024,
                    height: 640,
                }),
                ..WindowPreferences::default()
            })
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_config_path_preserves_electron_store_layout() {
        assert_eq!(
            legacy_electron_config_path_in(Path::new("config-root")),
            Path::new("config-root")
                .join("yesplaymusic")
                .join("config.json")
        );
    }

    #[test]
    fn legacy_config_rejects_oversized_files() {
        let directory = temporary_directory("oversized-legacy-config");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.json");
        fs::write(&path, vec![b' '; MAX_LEGACY_CONFIG_BYTES as usize + 1]).unwrap();
        assert_eq!(
            read_legacy_config_from_path(&path).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn legacy_config_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = temporary_directory("symlink-legacy-config");
        fs::create_dir_all(&directory).unwrap();
        let target = directory.join("target.json");
        let path = directory.join("config.json");
        fs::write(&target, br#"{"settings":{"lang":"zh-CN"}}"#).unwrap();
        symlink(&target, &path).unwrap();
        assert_eq!(
            read_legacy_config_from_path(&path).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_window_rejects_unexpected_fields() {
        let directory = temporary_directory("invalid-legacy-window-position");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.json");
        fs::write(
            &path,
            br#"{"window":{"x":80,"y":120,"width":1440,"height":840,"display":1}}"#,
        )
        .unwrap();
        assert_eq!(
            load_legacy_preferences_from_path(&path).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn existing_native_position_wins_over_legacy_data() {
        let native = WindowPreferences {
            position: Some(WindowPosition { x: 640, y: 220 }),
            ..WindowPreferences::default()
        };
        let legacy = WindowPreferences {
            always_on_top: true,
            position: Some(WindowPosition { x: 80, y: 120 }),
            size: Some(WindowSize {
                width: 1024,
                height: 640,
            }),
            ..WindowPreferences::default()
        };
        assert_eq!(
            with_legacy_fallback(Some(native.clone()), Some(legacy.clone())),
            native
        );
        assert_eq!(with_legacy_fallback(None, Some(legacy.clone())), legacy);
    }
}
