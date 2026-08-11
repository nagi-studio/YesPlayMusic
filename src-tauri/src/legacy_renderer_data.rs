use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use cookie::{time::OffsetDateTime, SameSite};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use tauri::{webview::Cookie, AppHandle, WebviewWindow};

use crate::window_preferences::legacy_electron_config_path;

const LEGACY_ORIGIN: &str = "http://localhost:27232";
const CURRENT_COOKIE_DOMAIN: &str = "127.0.0.1";
const MAX_LEVELDB_BYTES: u64 = 64 * 1024 * 1024;
const MAX_COOKIE_DB_BYTES: u64 = 64 * 1024 * 1024;
const MAX_COOKIE_COUNT: usize = 512;
const MAX_STORAGE_VALUE_BYTES: usize = 16 * 1024 * 1024;
const CHROMIUM_EPOCH_OFFSET_MICROS: i64 = 11_644_473_600_000_000;
const STORAGE_KEYS: [&str; 4] = ["data", "lastfm", "player", "playerCurrentTrackTime"];

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyRendererData {
    pub local_storage: HashMap<String, String>,
    pub cookies_imported: usize,
    pub encrypted_cookies_skipped: usize,
    pub cookies_failed: usize,
    pub auth_cookie_source: AuthCookieSource,
    pub cache_detected: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthCookieSource {
    Existing,
    Legacy,
    #[default]
    None,
}

#[derive(Debug)]
struct StoredCookie {
    name: String,
    value: String,
    path: String,
    secure: bool,
    http_only: bool,
    same_site: i64,
    expires: Option<OffsetDateTime>,
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(windows)]
fn has_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn has_reparse_point(_: &fs::Metadata) -> bool {
    false
}

fn validate_regular_file(path: &Path, max_bytes: u64) -> Result<Option<fs::Metadata>, io::Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || has_reparse_point(&metadata) {
        return Err(invalid_data("legacy data path must not be a link"));
    }
    if !metadata.is_file() {
        return Err(invalid_data("legacy data path must be a regular file"));
    }
    if metadata.len() > max_bytes {
        return Err(invalid_data("legacy data file is too large"));
    }
    Ok(Some(metadata))
}

fn validate_directory(path: &Path) -> Result<bool, io::Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || has_reparse_point(&metadata) {
        return Err(invalid_data("legacy data directory must not be a link"));
    }
    if !metadata.is_dir() {
        return Err(invalid_data("legacy data path must be a directory"));
    }
    Ok(true)
}

fn decode_dom_string(bytes: &[u8]) -> Result<String, io::Error> {
    let Some((&encoding, content)) = bytes.split_first() else {
        return Err(invalid_data("empty DOM storage string"));
    };
    match encoding {
        1 => Ok(content.iter().map(|byte| char::from(*byte)).collect()),
        0 if content.len() % 2 == 0 => {
            let units = content
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
            String::from_utf16(&units.collect::<Vec<_>>())
                .map_err(|_| invalid_data("invalid UTF-16 DOM storage string"))
        }
        0 => Err(invalid_data("odd UTF-16 DOM storage string")),
        _ => Err(invalid_data("unknown DOM storage string encoding")),
    }
}

fn read_leveldb_records(path: &Path) -> Result<Vec<leveldb_core::Record>, io::Error> {
    if !validate_directory(path)? {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let extension = entry_path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        if !matches!(extension.as_deref(), Some("ldb" | "sst" | "log")) {
            continue;
        }
        let Some(metadata) = validate_regular_file(&entry_path, MAX_LEVELDB_BYTES)? else {
            continue;
        };
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| invalid_data("legacy LevelDB size overflow"))?;
        if total_bytes > MAX_LEVELDB_BYTES {
            return Err(invalid_data("legacy LevelDB is too large"));
        }
        files.push(entry_path);
    }
    files.sort();

    let mut records = Vec::new();
    for file in files {
        let bytes = fs::read(&file)?;
        let parsed = match file.extension().and_then(|value| value.to_str()) {
            Some(extension) if extension.eq_ignore_ascii_case("log") => {
                leveldb_core::parse_log_bytes(&bytes, &file)
            }
            _ => leveldb_core::parse_table_bytes(&bytes, &file),
        }
        .map_err(|error| invalid_data(error.to_string()))?;
        records.extend(parsed);
    }
    Ok(records)
}

fn select_local_storage(
    records: Vec<leveldb_core::Record>,
) -> Result<HashMap<String, String>, io::Error> {
    let prefix = format!("_{LEGACY_ORIGIN}\0").into_bytes();
    let mut latest: HashMap<Vec<u8>, leveldb_core::Record> = HashMap::new();
    for record in records {
        if !record.key.starts_with(&prefix) {
            continue;
        }
        let should_replace = latest
            .get(&record.key)
            .is_none_or(|existing| record.seq > existing.seq);
        if should_replace {
            latest.insert(record.key.clone(), record);
        }
    }

    let mut output = HashMap::new();
    for (key, record) in latest {
        if record.deleted || record.value.len() > MAX_STORAGE_VALUE_BYTES {
            continue;
        }
        let name = decode_dom_string(&key[prefix.len()..])?;
        if !STORAGE_KEYS.contains(&name.as_str()) {
            continue;
        }
        output.insert(name, decode_dom_string(&record.value)?);
    }
    Ok(output)
}

pub fn read_local_storage(profile_dir: &Path) -> Result<HashMap<String, String>, io::Error> {
    select_local_storage(read_leveldb_records(
        &profile_dir.join("Local Storage").join("leveldb"),
    )?)
}

fn snapshot_cookie_database(
    profile_dir: &Path,
) -> Result<Option<(tempfile::TempDir, PathBuf)>, io::Error> {
    let source = profile_dir.join("Cookies");
    if validate_regular_file(&source, MAX_COOKIE_DB_BYTES)?.is_none() {
        return Ok(None);
    }
    let directory = tempfile::tempdir()?;
    let snapshot = directory.path().join("Cookies");
    fs::copy(&source, &snapshot)?;
    for suffix in ["-wal", "-shm"] {
        let mut source_sidecar = source.as_os_str().to_os_string();
        source_sidecar.push(suffix);
        let source_sidecar = PathBuf::from(source_sidecar);
        if validate_regular_file(&source_sidecar, MAX_COOKIE_DB_BYTES)?.is_some() {
            let mut snapshot_sidecar = snapshot.as_os_str().to_os_string();
            snapshot_sidecar.push(suffix);
            fs::copy(&source_sidecar, PathBuf::from(snapshot_sidecar))?;
        }
    }
    Ok(Some((directory, snapshot)))
}

fn read_plaintext_cookies(profile_dir: &Path) -> Result<(Vec<StoredCookie>, usize, usize), String> {
    let Some((_snapshot, path)) =
        snapshot_cookie_database(profile_dir).map_err(|error| error.to_string())?
    else {
        return Ok((Vec::new(), 0, 0));
    };
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT name, value, encrypted_value, path, is_secure, is_httponly, samesite, \
                    has_expires, expires_utc \
             FROM cookies WHERE host_key IN ('localhost', '.localhost', '127.0.0.1') \
             ORDER BY last_access_utc DESC LIMIT ?1",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([MAX_COOKIE_COUNT as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, bool>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .map_err(|error| error.to_string())?;

    let mut cookies = Vec::new();
    let mut encrypted_skipped = 0;
    let mut invalid_skipped = 0;
    let mut seen = HashSet::new();
    for row in rows {
        let (name, value, encrypted, path, secure, http_only, same_site, has_expires, expires_utc) =
            row.map_err(|error| error.to_string())?;
        if !seen.insert((name.clone(), path.clone())) {
            continue;
        }
        if !encrypted.is_empty() {
            encrypted_skipped += 1;
            continue;
        }
        if name.is_empty() || name.len() > 256 || value.len() > 16 * 1024 {
            invalid_skipped += 1;
            continue;
        }
        let expires = if has_expires {
            let Some(unix_seconds) = expires_utc
                .checked_sub(CHROMIUM_EPOCH_OFFSET_MICROS)
                .map(|value| value / 1_000_000)
            else {
                invalid_skipped += 1;
                continue;
            };
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_secs();
            let now = i64::try_from(now).map_err(|error| error.to_string())?;
            if unix_seconds <= now {
                continue;
            }
            match OffsetDateTime::from_unix_timestamp(unix_seconds) {
                Ok(expires) => Some(expires),
                Err(_) => {
                    invalid_skipped += 1;
                    continue;
                }
            }
        } else {
            None
        };
        cookies.push(StoredCookie {
            name,
            value,
            path: if path.starts_with('/') {
                path
            } else {
                "/".into()
            },
            secure,
            http_only,
            same_site,
            expires,
        });
    }
    Ok((cookies, encrypted_skipped, invalid_skipped))
}

fn retain_missing_cookies(cookies: &mut Vec<StoredCookie>, current: &[Cookie<'_>]) -> bool {
    let existing = current
        .iter()
        .filter(|cookie| {
            cookie
                .domain()
                .is_none_or(|domain| domain.trim_start_matches('.') == CURRENT_COOKIE_DOMAIN)
        })
        .map(|cookie| {
            (
                cookie.name().to_string(),
                cookie.path().unwrap_or("/").to_string(),
            )
        })
        .collect::<HashSet<_>>();
    let auth_cookie_available = existing.iter().any(|(name, _)| name == "MUSIC_U");
    cookies.retain(|cookie| {
        !(existing.contains(&(cookie.name.clone(), cookie.path.clone()))
            || auth_cookie_available && cookie.name == "MUSIC_U")
    });
    auth_cookie_available
}

fn apply_cookies(
    window: &WebviewWindow,
    mut cookies: Vec<StoredCookie>,
) -> Result<(usize, usize, AuthCookieSource), String> {
    let current = window.cookies().map_err(|error| error.to_string())?;
    let mut auth_cookie_source = if retain_missing_cookies(&mut cookies, &current) {
        AuthCookieSource::Existing
    } else {
        AuthCookieSource::None
    };
    let mut imported = 0;
    let mut failed = 0;
    for stored in cookies {
        let is_auth_cookie = stored.name == "MUSIC_U";
        let mut builder = Cookie::build((stored.name, stored.value))
            .domain(CURRENT_COOKIE_DOMAIN)
            .path(stored.path)
            .secure(stored.secure)
            .http_only(stored.http_only);
        builder = match stored.same_site {
            0 => builder.same_site(SameSite::None),
            1 => builder.same_site(SameSite::Lax),
            2 => builder.same_site(SameSite::Strict),
            _ => builder,
        };
        if let Some(expires) = stored.expires {
            builder = builder.expires(expires);
        }
        if window.set_cookie(builder.build()).is_ok() {
            imported += 1;
            if is_auth_cookie {
                auth_cookie_source = AuthCookieSource::Legacy;
            }
        } else {
            failed += 1;
        }
    }
    Ok((imported, failed, auth_cookie_source))
}

fn legacy_cache_detected(profile_dir: &Path) -> bool {
    let indexed_db = profile_dir.join("IndexedDB");
    validate_directory(&indexed_db).unwrap_or(false)
        && fs::read_dir(indexed_db)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("http_localhost_27232")
            })
}

pub fn import(profile_dir: &Path, window: &WebviewWindow) -> Result<LegacyRendererData, String> {
    let local_storage = read_local_storage(profile_dir).map_err(|error| error.to_string())?;
    let (cookies, encrypted_cookies_skipped, invalid_cookies_skipped) =
        read_plaintext_cookies(profile_dir)?;
    let (cookies_imported, apply_failures, auth_cookie_source) = apply_cookies(window, cookies)?;
    Ok(LegacyRendererData {
        local_storage,
        cookies_imported,
        encrypted_cookies_skipped,
        cookies_failed: invalid_cookies_skipped + apply_failures,
        auth_cookie_source,
        cache_detected: legacy_cache_detected(profile_dir),
    })
}

#[tauri::command]
pub async fn import_legacy_renderer_data(
    app: AppHandle,
    window: WebviewWindow,
) -> Result<LegacyRendererData, String> {
    let config_path = legacy_electron_config_path(&app)?;
    let profile_dir = config_path
        .parent()
        .ok_or_else(|| "legacy Electron profile has no parent directory".to_string())?;
    let profile_dir = profile_dir.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || import(&profile_dir, &window))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, value: &str, seq: u64, deleted: bool) -> leveldb_core::Record {
        let mut key = format!("_{LEGACY_ORIGIN}\0").into_bytes();
        key.push(1);
        key.extend_from_slice(name.as_bytes());
        let mut encoded_value = vec![1];
        encoded_value.extend_from_slice(value.as_bytes());
        leveldb_core::Record {
            key,
            value: encoded_value,
            seq,
            deleted,
            origin_file: PathBuf::from("fixture.ldb"),
        }
    }

    #[test]
    fn decodes_latin1_and_utf16_dom_strings() {
        assert_eq!(
            decode_dom_string(&[1, b'p', b'l', b'a', b'y']).unwrap(),
            "play"
        );
        assert_eq!(
            decode_dom_string(&[0, 0xad, 0x64, 0x3e, 0x65]).unwrap(),
            "播放"
        );
        assert!(decode_dom_string(&[0, 1]).is_err());
        assert!(decode_dom_string(&[2, b'x']).is_err());
    }

    #[test]
    fn accepts_only_expected_legacy_cache_directory() {
        let directory = tempfile::tempdir().unwrap();
        let indexed_db = directory.path().join("IndexedDB");
        fs::create_dir(&indexed_db).unwrap();
        fs::create_dir(indexed_db.join("https_example.indexeddb.leveldb")).unwrap();
        assert!(!legacy_cache_detected(directory.path()));
        fs::create_dir(indexed_db.join("http_localhost_27232.indexeddb.leveldb")).unwrap();
        assert!(legacy_cache_detected(directory.path()));
    }

    #[test]
    fn local_storage_uses_latest_live_whitelisted_values() {
        let mut other_origin = record("data", r#"{"user":{"id":1}}"#, 10, false);
        other_origin.key = b"_https://example.com\0\x01data".to_vec();
        let values = select_local_storage(vec![
            record("player", r#"{"_current":1}"#, 1, false),
            record("player", r#"{"_current":2}"#, 3, false),
            record("lastfm", r#"{"name":"old"}"#, 2, true),
            record("settings", r#"{"lang":"zh-CN"}"#, 4, false),
            other_origin,
        ])
        .unwrap();

        assert_eq!(values.len(), 1);
        assert_eq!(values.get("player"), Some(&r#"{"_current":2}"#.into()));
    }

    #[test]
    fn cookie_import_reads_only_plaintext_loopback_rows() {
        let directory = tempfile::tempdir().unwrap();
        let database = Connection::open(directory.path().join("Cookies")).unwrap();
        database
            .execute_batch(
                "CREATE TABLE cookies (
                    host_key TEXT NOT NULL,
                    name TEXT NOT NULL,
                    value TEXT NOT NULL,
                    encrypted_value BLOB NOT NULL,
                    path TEXT NOT NULL,
                    is_secure INTEGER NOT NULL,
                    is_httponly INTEGER NOT NULL,
                    samesite INTEGER NOT NULL,
                    has_expires INTEGER NOT NULL,
                    expires_utc INTEGER NOT NULL,
                    last_access_utc INTEGER NOT NULL
                );",
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO cookies VALUES ('localhost', 'MUSIC_U', 'plain', X'', '/', 0, 1, 1, 0, 0, 3)",
                [],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO cookies VALUES ('.localhost', 'MUSIC_U', 'stale', X'', '/', 0, 1, 1, 0, 0, 1)",
                [],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO cookies VALUES ('localhost', 'encrypted', '', X'0102', '/', 0, 1, -1, 0, 0, 2)",
                [],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO cookies VALUES ('example.com', 'foreign', 'secret', X'', '/', 0, 0, -1, 0, 0, 1)",
                [],
            )
            .unwrap();
        drop(database);

        let (cookies, encrypted_skipped, invalid_skipped) =
            read_plaintext_cookies(directory.path()).unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "MUSIC_U");
        assert_eq!(cookies[0].value, "plain");
        assert!(cookies[0].http_only);
        assert_eq!(encrypted_skipped, 1);
        assert_eq!(invalid_skipped, 0);
    }

    #[test]
    fn invalid_cookie_expiry_never_becomes_a_session_cookie() {
        let directory = tempfile::tempdir().unwrap();
        let database = Connection::open(directory.path().join("Cookies")).unwrap();
        database
            .execute_batch(
                "CREATE TABLE cookies (
                    host_key TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL,
                    encrypted_value BLOB NOT NULL, path TEXT NOT NULL,
                    is_secure INTEGER NOT NULL, is_httponly INTEGER NOT NULL,
                    samesite INTEGER NOT NULL, has_expires INTEGER NOT NULL,
                    expires_utc INTEGER NOT NULL, last_access_utc INTEGER NOT NULL
                );",
            )
            .unwrap();
        for (name, expires, access) in [("min", i64::MIN, 2), ("max", i64::MAX, 1)] {
            database
                .execute(
                    "INSERT INTO cookies VALUES ('localhost', ?1, 'value', X'', '/', 0, 0, -1, 1, ?2, ?3)",
                    rusqlite::params![name, expires, access],
                )
                .unwrap();
        }
        drop(database);

        let (cookies, _, invalid_skipped) = read_plaintext_cookies(directory.path()).unwrap();
        assert!(cookies.is_empty());
        assert_eq!(invalid_skipped, 2);
    }

    #[test]
    fn existing_tauri_auth_cookie_wins_across_cookie_paths() {
        let mut legacy = vec![StoredCookie {
            name: "MUSIC_U".into(),
            value: "legacy".into(),
            path: "/".into(),
            secure: false,
            http_only: true,
            same_site: 1,
            expires: None,
        }];
        let current = vec![Cookie::build(("MUSIC_U", "current"))
            .domain(CURRENT_COOKIE_DOMAIN)
            .path("/api")
            .build()];
        assert!(retain_missing_cookies(&mut legacy, &current));

        assert!(legacy.is_empty());
        assert_eq!(
            serde_json::to_value(AuthCookieSource::Existing).unwrap(),
            serde_json::json!("existing")
        );
    }
}
