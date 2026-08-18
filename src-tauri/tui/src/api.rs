//! NCM service: a typed façade over ncm-api-rs for the TUI. Every call
//! injects the persisted session cookie; anonymous calls degrade the same
//! way the desktop client does (standard quality, no personal data).

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use yesplaymusic_core::auth::Session;
use yesplaymusic_core::cache::{AudioCodec, AudioQuality, CacheKey};
use yesplaymusic_core::ncm::{
    AccountReason, NcmClient, NcmClientError, PlaybackSource, SongUrlError,
};
use yesplaymusic_core::unm::UnmState;

pub use yesplaymusic_core::ncm::{
    AlbumHit, ArtistHit, LyricsPayload, PlaylistHit, QrStatus, SearchChannel, SearchPage, SongRow,
};

use yesplaymusic_core::ncm::{SearchPayload as CoreSearchPayload, SongItem};

/// Tab order for the TUI's search view; the MV/user channels core also
/// models have no TUI tab.
pub trait SearchChannelTabs: Sized + Copy + PartialEq {
    const TABS: [SearchChannel; 4];
    fn index(self) -> usize;
    fn cycle(self, delta: i32) -> Self;
}

impl SearchChannelTabs for SearchChannel {
    const TABS: [SearchChannel; 4] = [
        SearchChannel::Songs,
        SearchChannel::Artists,
        SearchChannel::Albums,
        SearchChannel::Playlists,
    ];

    fn index(self) -> usize {
        Self::TABS.iter().position(|tab| *tab == self).unwrap_or(0)
    }

    fn cycle(self, delta: i32) -> Self {
        let index = (SearchChannelTabs::index(self) as i32 + delta)
            .rem_euclid(Self::TABS.len() as i32) as usize;
        Self::TABS[index]
    }
}

/// TUI-facing search payload: song hits collapsed to plain render rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchPayload {
    Songs(SearchPage<SongRow>),
    Artists(SearchPage<ArtistHit>),
    Albums(SearchPage<AlbumHit>),
    Playlists(SearchPage<PlaylistHit>),
}

fn song_row_from_hit(hit: SongItem) -> SongRow {
    SongRow {
        id: hit.id,
        title: hit.name,
        artist: hit
            .artists
            .first()
            .map(|artist| artist.name.clone())
            .unwrap_or_else(|| "?".to_owned()),
        artist_id: hit
            .artists
            .first()
            .map(|artist| artist.id)
            .filter(|id| *id > 0),
        album_id: (hit.album.id > 0).then_some(hit.album.id),
        album: hit.album.name,
        duration_ms: hit.duration_ms,
        pic_url: hit.album.pic_url,
    }
}

use crate::i18n::{self, Key};

const UNM_RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);
/// Every outbound HTTP call this crate owns is bounded: a dead CDN or a
/// captive portal must not hang a cover fetch forever.
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared, bounded HTTP client for the plain (non-NCM-signed) requests.
/// Reusing one client also keeps the connection pool warm across covers.
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| build_http_client(HTTP_CONNECT_TIMEOUT, HTTP_REQUEST_TIMEOUT))
}

fn build_http_client(connect: Duration, total: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect)
        .timeout(total)
        .build()
        // Builder failure means no TLS backend at all; an unbounded default
        // client still beats refusing to show any cover.
        .unwrap_or_default()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedMedia {
    NeteaseUrl(String),
    UnmUrl(String),
    UnmBytes(Vec<u8>),
}

impl ResolvedMedia {
    pub const fn is_unm(&self) -> bool {
        matches!(self, Self::UnmUrl(_) | Self::UnmBytes(_))
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedTrack {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub media: ResolvedMedia,
    pub kind: String,
    pub cache_key: CacheKey,
    pub codec: AudioCodec,
    pub actual_bitrate: u32,
    pub expected_bytes: Option<u64>,
    pub expected_md5: Option<[u8; 16]>,
    pub duration_ms: i64,
    pub pic_url: Option<String>,
    pub artist_id: Option<i64>,
    pub album_id: Option<i64>,
}

#[derive(Debug)]
enum SongUrlFailure {
    /// NCM answered `code: 200` with no playable URL — really no rights.
    Unavailable,
    /// NCM refused the request itself (expired cookie, rate limit, risk
    /// control). Distinct from `Unavailable` because UNM must not run: it
    /// would hide a fixable sign-in problem behind "no copyright" and burn
    /// one round trip per track.
    Rejected(Option<i64>),
    Other(anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
#[error("track has no playable source")]
pub(crate) struct TrackUnavailable;

#[derive(Clone, Debug, Eq, PartialEq)]
struct UnmResolution {
    provider: String,
    url: String,
}

trait UnmResolver: Send + Sync {
    fn resolve<'a>(&'a self, payload: &'a Value) -> BoxFuture<'a, Result<Option<UnmResolution>>>;
}

impl UnmResolver for UnmState {
    fn resolve<'a>(&'a self, payload: &'a Value) -> BoxFuture<'a, Result<Option<UnmResolution>>> {
        Box::pin(async move {
            Ok(UnmState::resolve(self, payload)
                .await
                .map_err(|_| anyhow!("UNM rejected the track payload"))?
                .map(|retrieved| UnmResolution {
                    provider: retrieved.source.into_owned(),
                    url: retrieved.url,
                }))
        })
    }
}

/// Which library list is on screen / feeding the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Liked,
    Daily,
    Fm,
    Cloud,
    Search,
}

/// Why `account()` failed — the split decides whether a startup treats the
/// stored session as dead or merely unverifiable.
#[derive(Debug)]
pub enum AccountError {
    /// NCM never answered (offline, DNS, timeout, rate limit, 5xx). The
    /// stored session may still be valid; don't log the user out over it.
    Unreachable(anyhow::Error),
    /// NCM answered and rejected or omitted the account: the session is dead.
    Expired(anyhow::Error),
}

impl fmt::Display for AccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(error) | Self::Expired(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AccountError {}

/// Translate a core account failure, preserving the Expired/Unreachable split.
fn account_error(error: yesplaymusic_core::ncm::AccountError) -> AccountError {
    use yesplaymusic_core::ncm::AccountError as Core;
    match error {
        Core::Expired(reason) => AccountError::Expired(account_reason_text(reason)),
        Core::Unreachable(reason) => AccountError::Unreachable(account_reason_text(reason)),
    }
}

fn account_reason_text(reason: AccountReason) -> anyhow::Error {
    match reason {
        AccountReason::Api(error) => anyhow!(i18n::t_api_failed(Key::OpAccount, error)),
        AccountReason::InvalidPayload => anyhow!(i18n::t(Key::ApiInvalidSession)),
    }
}

/// Translate library endpoint failures: transport errors carry the operation
/// label, a missing payload keeps its dedicated message, parse errors pass
/// through untranslated (they are diagnostic English paths).
fn library_error(operation: Key, error: NcmClientError) -> anyhow::Error {
    match error {
        NcmClientError::Api(error) => anyhow!(i18n::t_api_failed(operation, error)),
        NcmClientError::MissingPayload(_) => anyhow!(i18n::t(Key::ApiLibraryPayloadMissing)),
        other => anyhow!(other),
    }
}

pub struct Ncm {
    core: NcmClient,
    quality: RwLock<AudioQuality>,
    unm_enabled: bool,
    unm: Arc<dyn UnmResolver>,
    unm_timeout: Duration,
}

impl Ncm {
    pub fn new(session_path: PathBuf, quality: AudioQuality, unm_enabled: bool) -> Self {
        Self::with_unm(
            session_path,
            quality,
            unm_enabled,
            Arc::new(UnmState::new()),
            UNM_RESOLVE_TIMEOUT,
        )
    }

    fn with_unm(
        session_path: PathBuf,
        quality: AudioQuality,
        unm_enabled: bool,
        unm: Arc<dyn UnmResolver>,
        unm_timeout: Duration,
    ) -> Self {
        Self {
            core: NcmClient::new(session_path),
            quality: RwLock::new(quality),
            unm_enabled,
            unm,
            unm_timeout,
        }
    }

    pub fn session_snapshot(&self) -> Option<Session> {
        self.core.session_snapshot()
    }

    pub(crate) fn quality(&self) -> AudioQuality {
        *self.quality.read().expect("quality lock")
    }

    pub(crate) fn set_quality(&self, quality: AudioQuality) {
        *self.quality.write().expect("quality lock") = quality;
    }

    pub fn commit_session(&self, session: &Session) -> Result<()> {
        self.core
            .commit_session(session)
            .map_err(|error| match error {
                // Unwrap to the io::Error so the user-facing message keeps
                // its original form without the enum's Debug prefix.
                NcmClientError::PersistSession(error) => {
                    anyhow!(i18n::t_api_failed(Key::OpPersistSession, error))
                }
                other => anyhow!(i18n::t_api_failed(Key::OpPersistSession, other)),
            })
    }

    // ── login ────────────────────────────────────────────────────────

    pub async fn qr_key(&self) -> Result<String> {
        self.core.qr_key().await.map_err(|error| match error {
            NcmClientError::Api(error) => anyhow!(i18n::t_api_failed(Key::OpQrKey, error)),
            _ => anyhow!(i18n::t(Key::ApiQrKeyMissing)),
        })
    }

    pub fn qr_login_url(key: &str) -> String {
        NcmClient::qr_login_url(key)
    }

    pub async fn qr_check(&self, key: &str) -> Result<QrStatus> {
        self.core.qr_check(key).await.map_err(|error| match error {
            NcmClientError::Api(error) => anyhow!(i18n::t_api_failed(Key::OpQrCheck, error)),
            NcmClientError::UnknownQrStatus(status) => anyhow!(i18n::t_unknown_qr_status(status)),
            _ => anyhow!(i18n::t(Key::ApiLoginCookieMissing)),
        })
    }

    // ── account & library ────────────────────────────────────────────

    pub async fn account(
        &self,
        session: Option<&Session>,
    ) -> std::result::Result<(i64, String), AccountError> {
        self.core.account(session).await.map_err(account_error)
    }

    /// The user's "我喜欢的音乐" — by NCM convention the first playlist.
    pub async fn liked_songs(&self, uid: i64, session: Option<&Session>) -> Result<Vec<SongRow>> {
        let playlist_id = self
            .core
            .liked_playlist_id(uid, session)
            .await
            .map_err(|error| match error {
                NcmClientError::Api(error) => {
                    anyhow!(i18n::t_api_failed(Key::OpUserPlaylist, error))
                }
                _ => anyhow!(i18n::t(Key::ApiLikedPlaylistMissing)),
            })?;
        self.playlist_songs(playlist_id, session).await
    }

    pub async fn playlist_songs(
        &self,
        playlist_id: i64,
        session: Option<&Session>,
    ) -> Result<Vec<SongRow>> {
        self.core
            .playlist_songs(playlist_id, session)
            .await
            .map_err(|error| library_error(Key::OpPlaylistTracks, error))
    }

    pub async fn set_like(&self, id: i64, like: bool, session: Option<&Session>) -> Result<()> {
        self.core
            .set_like(id, like, session)
            .await
            .map_err(|_| like_error())
    }

    pub async fn liked_ids(
        &self,
        uid: i64,
        session: Option<&Session>,
    ) -> Result<std::collections::HashSet<i64>> {
        // The TUI only ever asks "is this song liked"; the ordered list the
        // GUI renders collapses into a set here.
        self.core
            .liked_ids(uid, session)
            .await
            .map(|ids| ids.into_iter().collect())
            .map_err(|error| library_error(Key::OpUserPlaylist, error))
    }

    pub async fn daily_songs(&self, session: Option<&Session>) -> Result<Vec<SongRow>> {
        self.core
            .daily_songs(session)
            .await
            .map_err(|error| library_error(Key::OpPlaylistTracks, error))
    }

    pub async fn personal_fm(&self, session: Option<&Session>) -> Result<Vec<SongRow>> {
        self.core
            .personal_fm(session)
            .await
            .map_err(|error| library_error(Key::OpPlaylistTracks, error))
    }

    pub async fn fm_trash(&self, id: i64, session: Option<&Session>) -> Result<()> {
        self.core
            .fm_trash(id, session)
            .await
            .map_err(|error| match error {
                NcmClientError::Rejected(code) => anyhow!(i18n::t_fm_trash_rejected(code)),
                NcmClientError::Api(error) => anyhow!(i18n::t_api_failed(Key::OpFmTrash, error)),
                other => anyhow!(other),
            })
    }

    pub async fn cloud_songs(&self, session: Option<&Session>) -> Result<Vec<SongRow>> {
        self.core
            .cloud_songs(session)
            .await
            .map_err(|error| library_error(Key::OpPlaylistTracks, error))
    }

    // ── playback resolution ──────────────────────────────────────────

    async fn song_url(
        &self,
        id: i64,
    ) -> std::result::Result<(AudioQuality, PlaybackSource), SongUrlFailure> {
        let requested_quality = self.quality();
        let source = self
            .core
            .song_url(id, requested_quality.bitrate())
            .await
            .map_err(|error| match error {
                SongUrlError::Unavailable => SongUrlFailure::Unavailable,
                SongUrlError::Rejected(code) => SongUrlFailure::Rejected(code),
                SongUrlError::Other(NcmClientError::Api(error)) => {
                    SongUrlFailure::Other(anyhow!(i18n::t_api_failed(Key::OpSongUrl, error)))
                }
                SongUrlError::Other(error) => SongUrlFailure::Other(anyhow!(error)),
            })?;
        Ok((requested_quality, source))
    }

    /// Raw line-, translation-, and word-synchronised lyrics for a song.
    pub async fn lyrics(&self, id: i64) -> Result<LyricsPayload> {
        self.core.lyrics(id).await.map_err(|error| match error {
            NcmClientError::Api(error) => anyhow!(i18n::t_api_failed(Key::OpLyrics, error)),
            other => anyhow!(other),
        })
    }

    pub async fn search_channel(
        &self,
        keywords: &str,
        channel: SearchChannel,
        limit: u32,
    ) -> Result<SearchPayload> {
        let payload = self
            .core
            .search_channel(keywords, channel, limit, 0)
            .await
            .map_err(|error| match error {
                NcmClientError::Api(error) => anyhow!(i18n::t_api_failed(Key::OpSearch, error)),
                other => anyhow!(other),
            })?;
        Ok(match payload {
            CoreSearchPayload::Songs(page) => SearchPayload::Songs(SearchPage {
                items: page.items.into_iter().map(song_row_from_hit).collect(),
                total: page.total,
            }),
            CoreSearchPayload::Artists(page) => SearchPayload::Artists(page),
            CoreSearchPayload::Albums(page) => SearchPayload::Albums(page),
            CoreSearchPayload::Playlists(page) => SearchPayload::Playlists(page),
            // The TUI's four tabs never request these channels.
            CoreSearchPayload::MusicVideos(_) | CoreSearchPayload::Users(_) => {
                return Err(anyhow!(i18n::t(Key::ApiLibraryPayloadMissing)))
            }
        })
    }

    pub async fn artist_top_songs(&self, artist_id: i64) -> Result<Vec<SongRow>> {
        self.core
            .artist_top_songs(artist_id)
            .await
            .map_err(|error| library_error(Key::OpPlaylistTracks, error))
    }

    pub async fn album_songs(&self, album_id: i64) -> Result<Vec<SongRow>> {
        self.core
            .album_songs(album_id)
            .await
            .map_err(|error| library_error(Key::OpPlaylistTracks, error))
    }

    pub async fn playlist_detail_songs(&self, playlist_id: i64) -> Result<Vec<SongRow>> {
        self.core
            .playlist_detail_songs(playlist_id)
            .await
            .map_err(|error| library_error(Key::OpPlaylistTracks, error))
    }

    pub async fn search_songs(&self, keywords: &str, limit: u32) -> Result<Vec<Value>> {
        self.core
            .search_songs(keywords, limit)
            .await
            .map_err(|error| library_error(Key::OpSearch, error))
    }

    /// Resolve a known song id straight to a playable track.
    pub async fn resolve_by_id(&self, row: &SongRow) -> Result<ResolvedTrack> {
        let (requested_quality, source) =
            self.song_url(row.id).await.map_err(|error| match error {
                SongUrlFailure::Unavailable => anyhow!(i18n::t(Key::ApiPlaybackUrlUnavailable)),
                SongUrlFailure::Rejected(code) => anyhow!(i18n::t_song_url_rejected(code)),
                SongUrlFailure::Other(error) => error,
            })?;
        Ok(self.resolved_track(row.clone(), requested_quality, source))
    }

    /// Resolve for active playback. UNM is only consulted after the NCM
    /// endpoint explicitly returns no URL, never for transport failures.
    pub async fn resolve_for_playback(&self, row: &SongRow) -> Result<ResolvedTrack> {
        if row.id <= 0 {
            return self.resolve_for_play(&row.title, &row.artist).await;
        }
        let requested_quality = self.quality();
        let native = self.song_url(row.id).await.map(|(_, source)| source);
        self.resolve_after_native(row, requested_quality, native)
            .await
    }

    async fn resolve_after_native(
        &self,
        row: &SongRow,
        requested_quality: AudioQuality,
        native: std::result::Result<PlaybackSource, SongUrlFailure>,
    ) -> Result<ResolvedTrack> {
        match native {
            Ok(source) => Ok(self.resolved_track(row.clone(), requested_quality, source)),
            Err(SongUrlFailure::Other(error)) => Err(error),
            // Auth/rate-limit refusals are not a copyright problem — say so
            // instead of spending a UNM round trip on every track.
            Err(SongUrlFailure::Rejected(code)) => Err(anyhow!(i18n::t_song_url_rejected(code))),
            Err(SongUrlFailure::Unavailable) => {
                self.resolve_unm_track(row, requested_quality).await
            }
        }
    }

    async fn resolve_unm_track(
        &self,
        row: &SongRow,
        requested_quality: AudioQuality,
    ) -> Result<ResolvedTrack> {
        if !self.unm_enabled {
            return Err(TrackUnavailable.into());
        }
        let payload = json!({
            "track": {
                "id": row.id,
                "name": row.title,
                "dt": row.duration_ms,
                "ar": [{ "id": 0, "name": row.artist }]
            },
            "context": {}
        });
        let resolution =
            match tokio::time::timeout(self.unm_timeout, self.unm.resolve(&payload)).await {
                Ok(Ok(Some(resolution))) => resolution,
                Ok(Ok(None)) => return Err(TrackUnavailable.into()),
                Ok(Err(error)) => {
                    tracing::warn!(%error, "UNM resolution failed");
                    return Err(TrackUnavailable.into());
                }
                Err(_) => {
                    tracing::warn!("UNM resolution timed out");
                    return Err(TrackUnavailable.into());
                }
            };

        if resolution.url.trim().is_empty() {
            return Err(TrackUnavailable.into());
        }
        let media = if resolution.provider.eq_ignore_ascii_case("bilibili") {
            let bytes = decode_base64(&resolution.url).map_err(|error| {
                tracing::warn!(%error, "UNM returned invalid Bilibili audio");
                TrackUnavailable
            })?;
            ResolvedMedia::UnmBytes(bytes)
        } else {
            ResolvedMedia::UnmUrl(resolution.url)
        };
        let codec = match &media {
            ResolvedMedia::UnmUrl(url) => codec_from_url(url),
            ResolvedMedia::UnmBytes(_) => AudioCodec::Mp3,
            ResolvedMedia::NeteaseUrl(_) => unreachable!(),
        };
        Ok(ResolvedTrack {
            id: row.id,
            title: row.title.clone(),
            artist: row.artist.clone(),
            album: row.album.clone(),
            media,
            kind: codec.extension().to_owned(),
            cache_key: CacheKey::new(row.id, requested_quality),
            codec,
            actual_bitrate: 128_000,
            expected_bytes: None,
            expected_md5: None,
            duration_ms: row.duration_ms,
            pic_url: row.pic_url.clone(),
            artist_id: row.artist_id,
            album_id: row.album_id,
        })
    }

    /// Search by "title artist" and resolve the first *playable* match —
    /// top hits can be VIP-gated with a null URL, so walk the candidates.
    pub async fn resolve_for_play(&self, title: &str, artist: &str) -> Result<ResolvedTrack> {
        let keywords = format!("{title} {artist}");
        let songs = self.search_songs(keywords.trim(), 8).await?;
        if songs.is_empty() {
            return Err(anyhow!(i18n::t_search_not_found(&keywords)));
        }
        for song in &songs {
            let Some(id) = song["id"].as_i64() else {
                continue;
            };
            let (requested_quality, source) = match self.song_url(id).await {
                Ok(resolved) => resolved,
                // A refusal is per-account, not per-track: the remaining
                // candidates would all fail the same way.
                Err(SongUrlFailure::Rejected(code)) => {
                    return Err(anyhow!(i18n::t_song_url_rejected(code)))
                }
                Err(_) => continue,
            };
            let row = SongRow {
                id,
                title: song["name"].as_str().unwrap_or(title).to_owned(),
                artist: song["ar"][0]["name"].as_str().unwrap_or(artist).to_owned(),
                album: song["al"]["name"].as_str().unwrap_or("").to_owned(),
                duration_ms: song["dt"].as_i64().unwrap_or(0),
                pic_url: song["al"]["picUrl"].as_str().map(str::to_owned),
                artist_id: song["ar"][0]["id"].as_i64().filter(|id| *id > 0),
                album_id: song["al"]["id"].as_i64().filter(|id| *id > 0),
            };
            return Ok(self.resolved_track(row, requested_quality, source));
        }
        Err(anyhow!(i18n::t_candidates_unavailable(&keywords)))
    }

    fn resolved_track(
        &self,
        row: SongRow,
        requested_quality: AudioQuality,
        source: PlaybackSource,
    ) -> ResolvedTrack {
        ResolvedTrack {
            id: row.id,
            title: row.title,
            artist: row.artist,
            album: row.album,
            kind: source.codec.extension().to_owned(),
            cache_key: CacheKey::new(row.id, requested_quality),
            codec: source.codec,
            actual_bitrate: source.actual_bitrate,
            expected_bytes: source.expected_bytes,
            expected_md5: source.expected_md5,
            media: ResolvedMedia::NeteaseUrl(source.url),
            duration_ms: row.duration_ms,
            pic_url: row.pic_url,
            artist_id: row.artist_id,
            album_id: row.album_id,
        }
    }
}

fn like_error() -> anyhow::Error {
    anyhow!(i18n::t(Key::LikeFailed))
}

fn codec_from_url(url: &str) -> AudioCodec {
    url.split(['?', '#'])
        .next()
        .and_then(|path| path.rsplit('/').next())
        .and_then(|name| name.rsplit_once('.').map(|(_, extension)| extension))
        .and_then(|extension| extension.parse().ok())
        .unwrap_or(AudioCodec::Mp3)
}

fn decode_base64(input: &str) -> Result<Vec<u8>> {
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(anyhow!("invalid base64 length"));
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunks = bytes.chunks_exact(4);
    let chunk_count = chunks.len();
    for (index, chunk) in chunks.enumerate() {
        let last = index + 1 == chunk_count;
        let high = base64_value(chunk[0]).ok_or_else(|| anyhow!("invalid base64 digit"))?;
        let low = base64_value(chunk[1]).ok_or_else(|| anyhow!("invalid base64 digit"))?;
        decoded.push((high << 2) | (low >> 4));
        match (chunk[2], chunk[3]) {
            (b'=', b'=') if last && low & 0x0f == 0 => {}
            (third, b'=') if last => {
                let third = base64_value(third).ok_or_else(|| anyhow!("invalid base64 digit"))?;
                if third & 0x03 != 0 {
                    return Err(anyhow!("invalid base64 padding"));
                }
                decoded.push((low << 4) | (third >> 2));
            }
            (third, fourth) if third != b'=' && fourth != b'=' => {
                let third = base64_value(third).ok_or_else(|| anyhow!("invalid base64 digit"))?;
                let fourth = base64_value(fourth).ok_or_else(|| anyhow!("invalid base64 digit"))?;
                decoded.push((low << 4) | (third >> 2));
                decoded.push((third << 6) | fourth);
            }
            _ => return Err(anyhow!("invalid base64 padding")),
        }
    }
    Ok(decoded)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Small square cover JPEG from the NCM CDN (`?param=WxH` server-side crop).
pub async fn fetch_cover(pic_url: &str, edge: u32) -> Result<Vec<u8>> {
    let url = format!("{pic_url}?param={edge}y{edge}");
    let response = http_client()
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| anyhow!(i18n::t_api_failed(Key::OpFetchCover, error)))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| anyhow!(i18n::t_api_failed(Key::OpReadCover, error)))?;
    Ok(bytes.to_vec())
}

/// Render a QR login link as terminal half-block art.
pub fn qr_unicode(url: &str) -> Result<String> {
    let code = qrcode::QrCode::new(url.as_bytes())
        .map_err(|error| anyhow!(i18n::t_api_failed(Key::OpBuildQr, error)))?;
    Ok(code
        .render::<qrcode::render::unicode::Dense1x2>()
        .dark_color(qrcode::render::unicode::Dense1x2::Light)
        .light_color(qrcode::render::unicode::Dense1x2::Dark)
        .build())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn ncm(quality: AudioQuality) -> Ncm {
        let dir = tempfile::tempdir().unwrap();
        Ncm::new(dir.path().join("session.json"), quality, true)
    }

    #[derive(Clone)]
    enum FakeUnmOutcome {
        Found(UnmResolution),
        Missing,
    }

    struct FakeUnm {
        outcome: FakeUnmOutcome,
        delay: Duration,
        calls: Arc<AtomicUsize>,
    }

    impl UnmResolver for FakeUnm {
        fn resolve<'a>(
            &'a self,
            _payload: &'a Value,
        ) -> BoxFuture<'a, Result<Option<UnmResolution>>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(self.delay).await;
                Ok(match &self.outcome {
                    FakeUnmOutcome::Found(resolution) => Some(resolution.clone()),
                    FakeUnmOutcome::Missing => None,
                })
            })
        }
    }

    fn ncm_with_unm(
        enabled: bool,
        outcome: FakeUnmOutcome,
        delay: Duration,
        timeout: Duration,
        calls: Arc<AtomicUsize>,
    ) -> Ncm {
        let dir = tempfile::tempdir().unwrap();
        Ncm::with_unm(
            dir.path().join("session.json"),
            AudioQuality::High320,
            enabled,
            Arc::new(FakeUnm {
                outcome,
                delay,
                calls,
            }),
            timeout,
        )
    }

    fn unavailable_row() -> SongRow {
        SongRow {
            id: 42,
            title: "Unavailable".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            duration_ms: 180_000,
            pic_url: None,
            artist_id: None,
            album_id: None,
        }
    }

    #[test]
    fn bilibili_base64_decoder_handles_complete_and_padded_groups() {
        assert_eq!(decode_base64("YQ==").unwrap(), b"a");
        assert_eq!(decode_base64("YWI=").unwrap(), b"ab");
        assert_eq!(decode_base64("YWJj").unwrap(), b"abc");
        assert!(decode_base64("YQ=A").is_err());
    }

    #[test]
    fn every_quality_maps_to_its_exact_ncm_bitrate() {
        let cases = [
            (AudioQuality::Low128, 128_000),
            (AudioQuality::Medium192, 192_000),
            (AudioQuality::High320, 320_000),
            (AudioQuality::Lossless, 350_000),
            (AudioQuality::HiRes, 999_000),
        ];
        for (quality, expected) in cases {
            assert_eq!(ncm(quality).quality().bitrate(), expected);
        }
    }

    #[tokio::test]
    async fn unavailable_native_audio_uses_unm_and_decodes_bilibili_bytes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let ncm = ncm_with_unm(
            true,
            FakeUnmOutcome::Found(UnmResolution {
                provider: "bilibili".into(),
                url: "YXVkaW8gYnl0ZXM=".into(),
            }),
            Duration::ZERO,
            Duration::from_secs(1),
            calls.clone(),
        );

        let track = ncm
            .resolve_after_native(
                &unavailable_row(),
                AudioQuality::High320,
                Err(SongUrlFailure::Unavailable),
            )
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            track.media,
            ResolvedMedia::UnmBytes(b"audio bytes".to_vec())
        );
        assert_eq!(track.codec, AudioCodec::Mp3);
        assert_eq!(track.actual_bitrate, 128_000);
    }

    #[tokio::test]
    async fn unavailable_native_audio_uses_a_regular_unm_url() {
        let calls = Arc::new(AtomicUsize::new(0));
        let url = "https://audio.example/recovered.FLAC?token=secret";
        let ncm = ncm_with_unm(
            true,
            FakeUnmOutcome::Found(UnmResolution {
                provider: "kugou".into(),
                url: url.into(),
            }),
            Duration::ZERO,
            Duration::from_secs(1),
            calls.clone(),
        );

        let track = ncm
            .resolve_after_native(
                &unavailable_row(),
                AudioQuality::High320,
                Err(SongUrlFailure::Unavailable),
            )
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(track.media, ResolvedMedia::UnmUrl(url.into()));
        assert_eq!(track.codec, AudioCodec::Flac);
    }

    #[tokio::test]
    async fn disabled_unm_does_not_call_the_resolver() {
        let calls = Arc::new(AtomicUsize::new(0));
        let ncm = ncm_with_unm(
            false,
            FakeUnmOutcome::Missing,
            Duration::ZERO,
            Duration::from_secs(1),
            calls.clone(),
        );

        let error = ncm
            .resolve_after_native(
                &unavailable_row(),
                AudioQuality::High320,
                Err(SongUrlFailure::Unavailable),
            )
            .await
            .unwrap_err();

        assert!(error.is::<TrackUnavailable>());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn unm_timeout_is_bounded_and_returns_unavailable() {
        let calls = Arc::new(AtomicUsize::new(0));
        let ncm = ncm_with_unm(
            true,
            FakeUnmOutcome::Missing,
            Duration::from_secs(1),
            Duration::from_millis(10),
            calls.clone(),
        );

        let error = ncm
            .resolve_after_native(
                &unavailable_row(),
                AudioQuality::High320,
                Err(SongUrlFailure::Unavailable),
            )
            .await
            .unwrap_err();

        assert!(error.is::<TrackUnavailable>());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn native_transport_errors_never_trigger_unm() {
        let calls = Arc::new(AtomicUsize::new(0));
        let ncm = ncm_with_unm(
            true,
            FakeUnmOutcome::Missing,
            Duration::ZERO,
            Duration::from_secs(1),
            calls.clone(),
        );

        let error = ncm
            .resolve_after_native(
                &unavailable_row(),
                AudioQuality::High320,
                Err(SongUrlFailure::Other(anyhow!("offline"))),
            )
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "offline");
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn resolved_track_keeps_requested_quality_separate_from_actual_audio() {
        let ncm = ncm(AudioQuality::High320);
        let requested_quality = ncm.quality();
        ncm.set_quality(AudioQuality::HiRes);
        let track = ncm.resolved_track(
            SongRow {
                id: 42,
                title: "Track".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                duration_ms: 180_000,
                pic_url: None,
                artist_id: None,
                album_id: None,
            },
            requested_quality,
            PlaybackSource {
                url: "https://example.test/audio.mp3".into(),
                codec: AudioCodec::Mp3,
                actual_bitrate: 320_000,
                expected_bytes: Some(7_654_321),
                expected_md5: Some([0x11; 16]),
            },
        );

        assert_eq!(track.cache_key, CacheKey::new(42, AudioQuality::High320));
        assert_eq!(track.codec, AudioCodec::Mp3);
        assert_eq!(track.kind, "mp3");
        assert_eq!(track.actual_bitrate, 320_000);
        assert_eq!(track.album, "Album");
        assert_eq!(track.expected_bytes, Some(7_654_321));
        assert_eq!(track.expected_md5, Some([0x11; 16]));
    }

    #[test]
    fn unm_url_codec_uses_the_path_extension_and_defaults_to_mp3() {
        assert_eq!(
            codec_from_url("https://audio.example/track.FLAC?token=secret"),
            AudioCodec::Flac
        );
        assert_eq!(
            codec_from_url("https://audio.example/signed?token=secret"),
            AudioCodec::Mp3
        );
    }

    #[test]
    fn qr_login_url_and_unicode_rendering_hold_the_key() {
        let url = Ncm::qr_login_url("abc123");
        assert_eq!(url, "https://music.163.com/login?codekey=abc123");
        let art = qr_unicode(&url).unwrap();
        assert!(art.lines().count() > 10);
    }

    #[tokio::test]
    async fn cover_fetch_accepts_success_and_rejects_http_error_status() {
        let success_url = serve_once("200 OK", b"image bytes").await;
        assert_eq!(fetch_cover(&success_url, 32).await.unwrap(), b"image bytes");

        let missing_url = serve_once("404 Not Found", b"missing").await;
        assert!(fetch_cover(&missing_url, 32).await.is_err());
    }

    #[tokio::test]
    async fn http_requests_give_up_instead_of_hanging_on_a_silent_server() {
        // Accepts the connection, then never answers — the failure mode a
        // bare `reqwest::get` waits out forever.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stalled = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
            drop(stream);
        });

        let client = build_http_client(Duration::from_millis(200), Duration::from_millis(200));
        let started = std::time::Instant::now();
        let error = client
            .get(format!("http://{address}/cover"))
            .send()
            .await
            .unwrap_err();

        assert!(error.is_timeout(), "expected a timeout, got {error:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        stalled.abort();
    }

    #[test]
    fn shared_http_client_carries_the_timeouts_covers_rely_on() {
        assert_eq!(HTTP_CONNECT_TIMEOUT, Duration::from_secs(10));
        assert_eq!(HTTP_REQUEST_TIMEOUT, Duration::from_secs(30));
        // Same instance every call: the pool has to survive across covers.
        assert!(std::ptr::eq(http_client(), http_client()));
    }

    #[tokio::test]
    async fn refused_song_url_reports_sign_in_instead_of_burning_a_unm_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let ncm = ncm_with_unm(
            true,
            FakeUnmOutcome::Found(UnmResolution {
                provider: "kugou".into(),
                url: "https://audio.example/recovered.mp3".into(),
            }),
            Duration::ZERO,
            Duration::from_secs(1),
            calls.clone(),
        );

        let error = ncm
            .resolve_after_native(
                &unavailable_row(),
                AudioQuality::High320,
                Err(SongUrlFailure::Rejected(Some(-462))),
            )
            .await
            .unwrap_err();

        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert!(!error.is::<TrackUnavailable>());
        assert!(error.to_string().contains("-462"));
    }

    async fn serve_once(status: &'static str, body: &'static [u8]) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
        });
        format!("http://{address}/cover")
    }
}
