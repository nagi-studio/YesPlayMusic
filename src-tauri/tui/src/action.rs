//! Every input becomes an Action; the reducer is the only place state changes.

use yesplaymusic_core::auth::Session;
use yesplaymusic_core::cache::CacheLease;

use crate::api::{ResolvedTrack, SearchChannel, SearchPayload, SongRow, Source};
use crate::pixel::PixelCover;
use crate::player::PlayerEvent;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverSurface {
    Playing,
    Selection,
    /// Mini pixel cover for the bottom player bar on browsing views.
    Bar,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoverRenderRequest {
    pub surface: CoverSurface,
    pub generation: u64,
    pub cells: (u16, u16),
    pub style_revision: u64,
    pub song_id: i64,
    pub source_key: String,
    /// Longest edge to fetch the artwork at. Terminal graphics never upscale,
    /// so this has to out-resolve the cover box in real pixels.
    pub source_edge: u32,
}

/// How the in-app updater ended. "Up to date" is not a failure, so it stays
/// distinct from the error arm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelfUpdateOutcome {
    Installed,
    UpToDate,
    Failed(String),
}

/// Idle-dashboard menu entries; resolved to Actions at the input layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Library,
    Search,
    Login,
    Settings,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    NowPlaying,
    Library,
    Search,
    Queue,
    Login,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionStamp {
    pub epoch: u64,
    pub uid: i64,
}

/// How a session restore failed; only a proven rejection logs the user out,
/// an unreachable network falls back to the stored profile and local data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreFailure {
    Expired,
    Offline,
}

/// A snapshot of what the caches hold versus their caps, for settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheUsage {
    pub audio_used: u64,
    pub audio_max: u64,
    pub cover_used: u64,
    pub cover_max: u64,
}

#[derive(Debug)]
pub enum Action {
    UiTick,
    Quit,
    SwitchView(View),
    Back,
    Escape,
    OpenCommandPalette,
    CloseCommandPalette,
    MoveCommandSelection(i32),
    ExecuteCommand,
    ToggleZen,
    ToggleSpectrum,
    TogglePlay,
    NextTrack,
    PrevTrack,
    SeekBy(i64),
    /// Seek to a fraction of the track, from the progress-bar mouse hit.
    SeekToRatio(f64),
    VolumeBy(f32),
    ToggleMute,
    MoveSelection(i32),
    MovePage(i32),
    Activate,
    AddSelectedToQueue,
    SelectIndex(usize),
    /// vim `g` prefix: two in a row jump to the list top.
    GKey,
    ToggleShuffle,
    CycleRepeat,
    CyclePlaybackMode,
    StartFilter,
    SelectSearchChannel(SearchChannel),
    ToggleLibraryFocus,
    ToggleHelp,
    /// Repaint from scratch (Ctrl+L / regained focus): a multiplexer that
    /// drops a frame leaves stale cells ratatui's diff will never touch.
    ForceRedraw,
    ToggleLike,
    /// Replace the queue with a fresh personal-FM batch and start playing.
    StartPersonalFm,
    /// "Never play again": trash the current FM track and skip on.
    TrashFmTrack,
    SetVolumeTo(f32),
    OpenSource(usize),
    SelectSetting(usize),
    AdjustSetting(i32),
    SaveSettings,
    CancelSettings,
    NerdFontProbeFinished(crate::nerd_font::Status),
    CacheUsageProbed(CacheUsage),
    /// Open the artist or album page a click landed on. The target is the
    /// one that was on screen, not whatever is playing when this is reduced.
    OpenPage(crate::ui::PageLink),
    UpdateAvailable(String),
    /// Download, verify and swap in the newest release from inside the app.
    StartSelfUpdate,
    SelfUpdateProgress(String),
    SelfUpdateFinished(SelfUpdateOutcome),
    JumpTop,
    JumpBottom,
    ConfirmYes,
    Mouse(crossterm::event::MouseEvent),
    RawKey(crossterm::event::KeyEvent),
    Paste(String),
    Resize {
        cols: u16,
        rows: u16,
    },
    Player(PlayerEvent),
    TrackResolved {
        generation: u64,
        track: ResolvedTrack,
    },
    RowCacheReady {
        generation: u64,
        row: SongRow,
        lease: Option<CacheLease>,
    },
    ResolvedCacheReady {
        generation: u64,
        track: ResolvedTrack,
        lease: Option<CacheLease>,
    },
    CacheFallbackResolved {
        generation: u64,
        track: ResolvedTrack,
    },
    ResolveFailed {
        generation: u64,
        message: String,
    },
    TrackUnavailable {
        generation: u64,
    },
    CoverLoaded {
        request: CoverRenderRequest,
        cover: PixelCover,
    },
    /// A cover load definitively failed: the surface must fall back to the
    /// placeholder instead of keeping the previous track's art forever.
    CoverLoadFailed {
        surface: CoverSurface,
        generation: u64,
    },
    CoverDecoded {
        surface: CoverSurface,
        generation: u64,
        style_revision: u64,
        image: image::DynamicImage,
    },
    SelectionCoverDue {
        generation: u64,
        row: SongRow,
        neighbors: Vec<SongRow>,
        needs_network: bool,
    },
    SelectionCoverWarmed {
        generation: u64,
        style_revision: u64,
        pixel_key: String,
        cover: PixelCover,
    },
    IdleArtBytes {
        bytes: Vec<u8>,
    },
    IdleArtLoaded {
        cells: (u16, u16),
        style_revision: u64,
        cover: PixelCover,
    },
    StartLogin,
    LoginQrReady {
        attempt: u64,
        art: String,
    },
    LoginProgress {
        attempt: u64,
        message: String,
    },
    LoginFailed {
        attempt: u64,
        message: String,
    },
    LoginSucceeded {
        attempt: u64,
        session: Session,
        uid: i64,
        nickname: String,
    },
    SessionRestored {
        epoch: u64,
        uid: i64,
        nickname: String,
    },
    SessionRestoreFailed {
        epoch: u64,
        failure: RestoreFailure,
    },
    LibraryLoaded {
        session: SessionStamp,
        request: u64,
        source: Source,
        rows: Vec<SongRow>,
    },
    LibraryFailed {
        session: SessionStamp,
        request: u64,
        message: String,
    },
    FmMore {
        session: SessionStamp,
        rows: Vec<SongRow>,
    },
    FmLoadFailed {
        session: SessionStamp,
        message: String,
    },
    FmTrashFinished {
        session: SessionStamp,
        message: Option<String>,
    },
    PrefetchReady {
        index: usize,
        track: ResolvedTrack,
    },
    SearchResults {
        seq: u64,
        query: String,
        channel: SearchChannel,
        offset: u32,
        payload: SearchPayload,
    },
    SearchFailed {
        seq: u64,
        query: String,
        channel: SearchChannel,
        offset: u32,
        message: String,
    },
    SearchDetailLoaded {
        seq: u64,
        channel: SearchChannel,
        id: i64,
        rows: Vec<SongRow>,
    },
    SearchDetailFailed {
        seq: u64,
        channel: SearchChannel,
        id: i64,
        message: String,
    },
    LikedIds {
        session: SessionStamp,
        ids: std::collections::HashSet<i64>,
    },
    LikeFinished {
        session: SessionStamp,
        id: i64,
        mutation: u64,
        attempted_like: bool,
        error: Option<String>,
    },
    LyricsLoaded {
        generation: u64,
        lines: Vec<crate::lyrics::LyricLine>,
    },
}
