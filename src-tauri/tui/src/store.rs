use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::api::{SongRow, Source};
use crate::app::PlayMode;

const SNAPSHOT_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredSong {
    pub id: i64,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: String,
    pub duration_ms: i64,
    pub pic_url: Option<String>,
    /// Defaulted so a library written before artist/album links existed
    /// still loads; those rows simply cannot link until they are refetched.
    #[serde(default)]
    pub artist_id: Option<i64>,
    #[serde(default)]
    pub album_id: Option<i64>,
}

impl From<&SongRow> for StoredSong {
    fn from(row: &SongRow) -> Self {
        Self {
            id: row.id,
            title: row.title.clone(),
            artist: row.artist.clone(),
            album: row.album.clone(),
            duration_ms: row.duration_ms,
            pic_url: row.pic_url.clone(),
            artist_id: row.artist_id,
            album_id: row.album_id,
        }
    }
}

impl StoredSong {
    pub fn into_song_row(self) -> SongRow {
        SongRow {
            id: self.id,
            title: self.title,
            artist: self.artist,
            album: self.album,
            duration_ms: self.duration_ms,
            pic_url: self.pic_url,
            artist_id: self.artist_id,
            album_id: self.album_id,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LibraryStore {
    root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    saved_at_unix: u64,
    rows: Vec<StoredSong>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredPlayback {
    pub queue: Vec<StoredSong>,
    pub current: Option<StoredSong>,
    pub queue_pos: Option<usize>,
    pub position_ms: u64,
    pub volume: f32,
    pub volume_before_mute: Option<f32>,
    pub play_mode: PlayMode,
    pub shuffle: bool,
    pub queue_source: Source,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlaybackSnapshot {
    version: u32,
    saved_at_unix: u64,
    playback: StoredPlayback,
}

/// Last verified account identity; lets an offline start adopt the stored
/// session instead of treating an unreachable network as a logout.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredProfile {
    pub uid: i64,
    pub nickname: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProfileSnapshot {
    version: u32,
    saved_at_unix: u64,
    profile: StoredProfile,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileLoadError {
    #[error("failed to read stored profile at {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("stored profile at {} is invalid: {source}", path.display())]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "stored profile at {} uses unsupported version {found}",
        path.display()
    )]
    UnsupportedVersion { path: PathBuf, found: u32 },
    #[error("failed to scan legacy library snapshots at {}: {source}", path.display())]
    LegacyScan {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl LibraryStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn load(&self, uid: i64, source: &str) -> Option<Vec<StoredSong>> {
        self.read_snapshot(uid, source)
            .map(|snapshot| snapshot.rows)
    }

    pub fn save(&self, uid: i64, source: &str, rows: &[StoredSong]) -> io::Result<()> {
        validate_source(source)?;
        fs::create_dir_all(&self.root)?;

        let snapshot = Snapshot {
            version: SNAPSHOT_VERSION,
            saved_at_unix: unix_now()?,
            rows: rows.to_vec(),
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(io::Error::other)?;
        let path = self.snapshot_path(uid, source);
        let temporary = self.temporary_path(uid, source);
        fs::write(&temporary, bytes)?;
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(temporary);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn load_playback(&self) -> Option<StoredPlayback> {
        let bytes = fs::read(self.root.join("playback.json")).ok()?;
        let snapshot: PlaybackSnapshot = serde_json::from_slice(&bytes).ok()?;
        (snapshot.version == SNAPSHOT_VERSION).then_some(snapshot.playback)
    }

    pub(crate) fn save_playback(&self, playback: &StoredPlayback) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        let snapshot = PlaybackSnapshot {
            version: SNAPSHOT_VERSION,
            saved_at_unix: unix_now()?,
            playback: playback.clone(),
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(io::Error::other)?;
        let path = self.root.join("playback.json");
        let temporary = self.root.join("playback.json.tmp");
        fs::write(&temporary, bytes)?;
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(temporary);
            return Err(error);
        }
        Ok(())
    }

    pub fn load_profile(&self) -> Result<Option<StoredProfile>, ProfileLoadError> {
        let path = self.root.join("profile.json");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.profile_from_snapshots();
            }
            Err(source) => return Err(ProfileLoadError::Read { path, source }),
        };
        let snapshot: ProfileSnapshot =
            serde_json::from_slice(&bytes).map_err(|source| ProfileLoadError::Decode {
                path: path.clone(),
                source,
            })?;
        if snapshot.version != SNAPSHOT_VERSION {
            return Err(ProfileLoadError::UnsupportedVersion {
                path,
                found: snapshot.version,
            });
        }
        Ok(Some(snapshot.profile))
    }

    /// Pre-profile installs already have per-uid library snapshots. Adopt the
    /// most recently synced uid so the first offline start after an upgrade
    /// still reaches the local library.
    fn profile_from_snapshots(&self) -> Result<Option<StoredProfile>, ProfileLoadError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ProfileLoadError::LegacyScan {
                    path: self.root.clone(),
                    source,
                });
            }
        };
        let mut best: Option<(SystemTime, i64)> = None;
        for entry in entries {
            let entry = entry.map_err(|source| ProfileLoadError::LegacyScan {
                path: self.root.clone(),
                source,
            })?;
            let name = entry.file_name();
            let Some(uid) = name
                .to_str()
                .and_then(|name| name.strip_suffix("-liked.json"))
                .and_then(|uid| uid.parse::<i64>().ok())
            else {
                continue;
            };
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map_err(|source| ProfileLoadError::LegacyScan {
                    path: entry.path(),
                    source,
                })?;
            if best.is_none_or(|(freshest, _)| modified > freshest) {
                best = Some((modified, uid));
            }
        }
        Ok(best.map(|(_, uid)| StoredProfile {
            uid,
            nickname: String::new(),
        }))
    }

    pub fn save_profile(&self, profile: &StoredProfile) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        let snapshot = ProfileSnapshot {
            version: SNAPSHOT_VERSION,
            saved_at_unix: unix_now()?,
            profile: profile.clone(),
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(io::Error::other)?;
        let path = self.root.join("profile.json");
        let temporary = self.root.join("profile.json.tmp");
        fs::write(&temporary, bytes)?;
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(temporary);
            return Err(error);
        }
        Ok(())
    }

    fn read_snapshot(&self, uid: i64, source: &str) -> Option<Snapshot> {
        validate_source(source).ok()?;
        let bytes = fs::read(self.snapshot_path(uid, source)).ok()?;
        let snapshot: Snapshot = serde_json::from_slice(&bytes).ok()?;
        (snapshot.version == SNAPSHOT_VERSION).then_some(snapshot)
    }

    fn snapshot_path(&self, uid: i64, source: &str) -> PathBuf {
        self.root.join(format!("{uid}-{source}.json"))
    }

    fn temporary_path(&self, uid: i64, source: &str) -> PathBuf {
        self.root.join(format!("{uid}-{source}.json.tmp"))
    }
}

fn validate_source(source: &str) -> io::Result<()> {
    if !source.is_empty() && source.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "library source must contain only lowercase ASCII letters",
    ))
}

fn unix_now() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::ErrorKind;

    use tempfile::tempdir;

    use super::{
        LibraryStore, ProfileLoadError, ProfileSnapshot, StoredPlayback, StoredProfile, StoredSong,
        SNAPSHOT_VERSION,
    };
    use crate::api::{SongRow, Source};
    use crate::app::PlayMode;

    #[test]
    fn roundtrips_rows_and_song_row_conversion() {
        let directory = tempdir().unwrap();
        let store = LibraryStore::new(directory.path().join("library"));
        let source = SongRow {
            id: 42,
            title: "晚风经过月台".into(),
            artist: "遠い灯".into(),
            album: "夜色".into(),
            duration_ms: 218_000,
            pic_url: Some("https://example.test/cover.jpg".into()),
            artist_id: None,
            album_id: None,
        };
        let stored = StoredSong::from(&source);

        store
            .save(7, "liked", std::slice::from_ref(&stored))
            .unwrap();
        let loaded = store.load(7, "liked").unwrap();

        assert_eq!(loaded, vec![stored.clone()]);
        let restored = stored.into_song_row();
        assert_eq!(restored.id, source.id);
        assert_eq!(restored.title, source.title);
        assert_eq!(restored.artist, source.artist);
        assert_eq!(restored.album, source.album);
        assert_eq!(restored.duration_ms, source.duration_ms);
        assert_eq!(restored.pic_url, source.pic_url);
    }

    #[test]
    fn damaged_json_returns_none() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("library");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("9-daily.json"), b"{not json").unwrap();
        let store = LibraryStore::new(root);

        assert_eq!(store.load(9, "daily"), None);
    }

    #[test]
    fn unsupported_version_returns_none() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("library");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("11-cloud.json"),
            br#"{"version":0,"saved_at_unix":1,"rows":[]}"#,
        )
        .unwrap();
        let store = LibraryStore::new(root);

        assert_eq!(store.load(11, "cloud"), None);
    }

    #[test]
    fn version_one_rows_without_album_remain_readable() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("library");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("12-liked.json"),
            br#"{"version":1,"saved_at_unix":1,"rows":[{"id":42,"title":"Track","artist":"Artist","duration_ms":180000,"pic_url":null}]}"#,
        )
        .unwrap();

        let rows = LibraryStore::new(root).load(12, "liked").unwrap();

        assert_eq!(rows.len(), 1);
        assert!(rows[0].album.is_empty());
    }

    #[test]
    fn save_creates_the_root_directory() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("nested/library");
        let store = LibraryStore::new(root.clone());

        store.save(13, "liked", &[]).unwrap();

        assert!(root.join("13-liked.json").is_file());
        assert_eq!(store.load(13, "liked"), Some(Vec::new()));
    }

    #[test]
    fn atomic_save_leaves_no_temporary_file() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("library");
        let store = LibraryStore::new(root.clone());

        store.save(15, "daily", &[song(1)]).unwrap();
        store.save(15, "daily", &[song(2)]).unwrap();

        assert!(root.join("15-daily.json").is_file());
        assert!(!root.join("15-daily.json.tmp").exists());
        assert_eq!(store.load(15, "daily"), Some(vec![song(2)]));
    }

    #[test]
    fn invalid_source_is_rejected_without_path_traversal() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("library");
        let store = LibraryStore::new(root.clone());

        let error = store.save(17, "../x", &[song(1)]).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        assert_eq!(store.load(17, "../x"), None);
        assert!(!root.exists());
        assert!(!directory.path().join("x.json").exists());
    }

    #[test]
    fn playback_state_roundtrips_and_atomically_replaces_the_previous_exit() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("library");
        let store = LibraryStore::new(root.clone());
        let mut playback = StoredPlayback {
            queue: vec![song(1), song(1), song(2)],
            current: Some(song(1)),
            queue_pos: Some(1),
            position_ms: 42_500,
            volume: 0.0,
            volume_before_mute: Some(0.7),
            play_mode: PlayMode::One,
            shuffle: true,
            queue_source: Source::Fm,
        };

        store.save_playback(&playback).unwrap();
        playback.position_ms = 43_000;
        store.save_playback(&playback).unwrap();

        assert_eq!(store.load_playback(), Some(playback));
        assert!(!root.join("playback.json.tmp").exists());
    }

    #[test]
    fn profile_roundtrips_and_replaces_the_previous_account() {
        let directory = tempdir().unwrap();
        let store = LibraryStore::new(directory.path().join("library"));
        let first = StoredProfile {
            uid: 7,
            nickname: "夜航".into(),
        };
        let second = StoredProfile {
            uid: 9,
            nickname: "Nagi".into(),
        };

        store.save_profile(&first).unwrap();
        store.save_profile(&second).unwrap();

        assert_eq!(store.load_profile().unwrap(), Some(second));
    }

    #[test]
    fn missing_profile_falls_back_to_the_library_snapshot_uid() {
        let directory = tempdir().unwrap();
        let store = LibraryStore::new(directory.path().join("library"));
        store.save(21, "liked", &[song(1)]).unwrap();
        store.save(33, "daily", &[song(2)]).unwrap();

        assert_eq!(
            store.load_profile().unwrap(),
            Some(StoredProfile {
                uid: 21,
                nickname: String::new(),
            })
        );
    }

    #[test]
    fn saved_profile_outranks_the_snapshot_scan() {
        let directory = tempdir().unwrap();
        let store = LibraryStore::new(directory.path().join("library"));
        store.save(21, "liked", &[]).unwrap();
        store
            .save_profile(&StoredProfile {
                uid: 9,
                nickname: "Nagi".into(),
            })
            .unwrap();

        assert_eq!(
            store.load_profile().unwrap().map(|profile| profile.uid),
            Some(9)
        );
    }

    #[test]
    fn damaged_profile_is_not_replaced_by_a_legacy_snapshot() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("library");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("21-liked.json"), b"legacy marker").unwrap();
        fs::write(root.join("profile.json"), b"{broken").unwrap();

        assert!(matches!(
            LibraryStore::new(root).load_profile(),
            Err(ProfileLoadError::Decode { .. })
        ));
    }

    #[test]
    fn unsupported_profile_version_is_an_error() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("library");
        fs::create_dir_all(&root).unwrap();
        let snapshot = ProfileSnapshot {
            version: SNAPSHOT_VERSION + 1,
            saved_at_unix: 1,
            profile: StoredProfile {
                uid: 7,
                nickname: "future".into(),
            },
        };
        fs::write(
            root.join("profile.json"),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            LibraryStore::new(root).load_profile(),
            Err(ProfileLoadError::UnsupportedVersion { found, .. })
                if found == SNAPSHOT_VERSION + 1
        ));
    }

    #[test]
    fn profile_read_errors_are_not_treated_as_missing() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("library");
        fs::create_dir_all(root.join("profile.json")).unwrap();

        assert!(matches!(
            LibraryStore::new(root).load_profile(),
            Err(ProfileLoadError::Read { .. })
        ));
    }

    #[test]
    fn damaged_playback_state_is_ignored() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("library");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("playback.json"), b"{broken").unwrap();

        assert!(LibraryStore::new(root).load_playback().is_none());
    }

    fn song(id: i64) -> StoredSong {
        StoredSong {
            id,
            title: format!("Track {id}"),
            artist: "Artist".into(),
            album: "Album".into(),
            duration_ms: 180_000,
            pic_url: None,
            artist_id: None,
            album_id: None,
        }
    }
}
