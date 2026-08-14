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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoverRenderRequest {
    pub surface: CoverSurface,
    pub generation: u64,
    pub cells: (u16, u16),
    pub style_revision: u64,
    pub song_id: i64,
    pub source_key: String,
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
    ToggleLike,
    SetVolumeTo(f32),
    OpenSource(usize),
    SelectSetting(usize),
    AdjustSetting(i32),
    SaveSettings,
    CancelSettings,
    NerdFontProbeFinished(crate::nerd_font::Status),
    UpdateAvailable(String),
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
        message: String,
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
    PrefetchReady {
        index: usize,
        track: ResolvedTrack,
    },
    SearchResults {
        seq: u64,
        query: String,
        channel: SearchChannel,
        payload: SearchPayload,
    },
    SearchFailed {
        seq: u64,
        query: String,
        channel: SearchChannel,
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
