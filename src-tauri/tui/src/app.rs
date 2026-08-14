//! Single state source: input becomes Action, update() is the only writer,
//! ui::draw() only reads.

pub(crate) mod command_palette;
mod filter;
mod reducer;
mod search;
mod session;
pub(crate) mod settings;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use image::{DynamicImage, Rgba};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};
use ratatui_image::StatefulImage;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use yesplaymusic_core::cache::{
    CacheKey, CacheLease, CacheMetadata, CacheWriteRequest, TrackCache,
};

use crate::action::{Action, CoverRenderRequest, CoverSurface, View};
use crate::api::{self, Ncm, SongRow, Source};
use crate::config::{self, Config, CoverMode, LoadedConfig};
use crate::cover_cache::{CoverCache, PixelKeyInputs};
use crate::event;
use crate::i18n::{self, Key};
use crate::pixel::{self, PixelCover};
use crate::player::{self, PlayerCommand, PlayerEvent, PlayerHandle};
use crate::spectrum::SpectrumView;
use crate::theme::Theme;
use crate::ui;

use self::command_palette::CommandPaletteState;
use self::filter::ListFilter;
use self::search::SearchState;
use self::session::SessionState;

/// Side-effect handles the reducer may use; state itself stays plain data.
pub struct Effects {
    pub player: PlayerHandle,
    pub ncm: Arc<Ncm>,
    pub store: Arc<crate::store::LibraryStore>,
    pub actions: mpsc::UnboundedSender<Action>,
    pub cache_root: Option<std::path::PathBuf>,
    pub covers: Option<Arc<CoverCache>>,
    pub config_path: std::path::PathBuf,
}

const COVER_SOURCE_EDGE: u32 = 500;
pub(crate) const PREVIEW_CELLS: (u16, u16) = (22, 11);
const HOT_PIXEL_COVER_LIMIT: usize = 64;

struct OriginalCover {
    picker: Picker,
    protocol: ThreadProtocol,
    generation: Option<u64>,
    pending: Option<PendingOriginalCover>,
    pending_requests: Option<mpsc::UnboundedSender<ResizeRequest>>,
}

struct PendingOriginalCover {
    protocol: ThreadProtocol,
    generation: u64,
}

impl OriginalCover {
    fn new(picker: Picker, requests: mpsc::UnboundedSender<ResizeRequest>) -> Self {
        Self {
            picker,
            protocol: ThreadProtocol::new(requests, None),
            generation: None,
            pending: None,
            pending_requests: None,
        }
    }

    fn buffered(
        picker: Picker,
        requests: mpsc::UnboundedSender<ResizeRequest>,
        pending_requests: mpsc::UnboundedSender<ResizeRequest>,
    ) -> Self {
        Self {
            picker,
            protocol: ThreadProtocol::new(requests, None),
            generation: None,
            pending: None,
            pending_requests: Some(pending_requests),
        }
    }

    fn clear(&mut self) {
        self.generation = None;
        self.protocol.empty_protocol();
        self.cancel_pending();
    }

    fn replace(&mut self, generation: u64, image: DynamicImage) {
        if self.generation.is_some() {
            if let Some(requests) = self.pending_requests.clone() {
                let protocol = self.picker.new_resize_protocol(image);
                if let Some(pending) = &mut self.pending {
                    pending.protocol.replace_protocol(protocol);
                    pending.generation = generation;
                } else {
                    self.pending = Some(PendingOriginalCover {
                        protocol: ThreadProtocol::new(requests, Some(protocol)),
                        generation,
                    });
                }
                return;
            }
        }
        self.protocol
            .replace_protocol(self.picker.new_resize_protocol(image));
        self.generation = Some(generation);
    }

    fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    fn render(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let ready_without_resize = if let Some(pending) = &mut self.pending {
            frame.render_stateful_widget(StatefulImage::new(), area, &mut pending.protocol);
            pending.protocol.protocol_type().is_some()
        } else {
            false
        };
        if ready_without_resize {
            self.promote_pending();
            return;
        }
        frame.render_stateful_widget(StatefulImage::new(), area, &mut self.protocol);
    }

    fn cancel_pending(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.protocol.empty_protocol();
        }
    }

    fn update_pending(&mut self, response: ResizeResponse, current_generation: u64) {
        let ready = self.pending.as_mut().is_some_and(|pending| {
            pending.generation == current_generation
                && pending.protocol.update_resized_protocol(response)
        });
        if ready {
            self.promote_pending();
        }
    }

    fn promote_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.protocol = pending.protocol;
        self.generation = Some(pending.generation);
    }

    fn set_background(&mut self, background: Color) {
        let color = match background {
            Color::Rgb(red, green, blue) => Some(Rgba([red, green, blue, 255])),
            _ => None,
        };
        self.picker.set_background_color(color);
    }
}

fn select_graphics_picker(mode: CoverMode, picker: Option<Picker>) -> Option<Picker> {
    if mode != CoverMode::Original {
        return None;
    }
    picker.filter(|picker| picker.protocol_type() != ProtocolType::Halfblocks)
}

fn query_graphics_picker(mode: CoverMode, background: Color) -> Option<Picker> {
    if mode != CoverMode::Original {
        return None;
    }
    let queried = Picker::from_query_stdio().ok();
    // ratatui-image's query bundle includes its own OSC 11, and its parser
    // can stop reading before a slow terminal finishes answering. Sweep the
    // leftovers before the event stream starts, or they arrive as phantom
    // key presses (seen as the command palette popping open with hex noise).
    crate::terminal_background::drain_pending_responses();
    let mut picker = select_graphics_picker(mode, queried)?;
    if let Color::Rgb(red, green, blue) = background {
        picker.set_background_color(Some(Rgba([red, green, blue, 255])));
    }
    Some(picker)
}

fn initialize_audio_cache(config: &Config) -> Option<std::path::PathBuf> {
    let root = config::cache_dir().join("audio");
    let cache = match TrackCache::open(&root) {
        Ok(cache) => cache,
        Err(error) => {
            tracing::warn!(%error, "audio cache unavailable");
            return None;
        }
    };
    if let Some(limit_mib) = config.cache_limit_mib {
        let Some(max_bytes) = limit_mib.checked_mul(1024 * 1024) else {
            tracing::warn!("audio cache limit is too large");
            return Some(root);
        };
        if let Err(error) = cache.set_max_bytes(max_bytes) {
            tracing::warn!(%error, "audio cache policy update failed");
        }
    }
    Some(root)
}

fn initialize_cover_cache() -> Option<Arc<CoverCache>> {
    match CoverCache::new(config::cache_dir().join("covers")) {
        Ok(cache) => Some(Arc::new(cache)),
        Err(error) => {
            tracing::warn!(%error, "cover cache unavailable");
            None
        }
    }
}

fn spawn_resize_worker(
    mut requests: mpsc::UnboundedReceiver<ResizeRequest>,
    responses: mpsc::UnboundedSender<ResizeResponse>,
) {
    tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            let response = tokio::task::spawn_blocking(move || request.resize_encode()).await;
            let Ok(Ok(response)) = response else { continue };
            if responses.send(response).is_err() {
                break;
            }
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMode {
    Off,
    List,
    One,
}

impl PlayMode {
    fn next(self) -> Self {
        match self {
            PlayMode::Off => PlayMode::List,
            PlayMode::List => PlayMode::One,
            PlayMode::One => PlayMode::Off,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlaybackModeSlot {
    Sequential,
    RepeatList,
    RepeatOne,
    Shuffle,
}

impl PlaybackModeSlot {
    pub(crate) fn from_parts(shuffle: bool, repeat: PlayMode) -> Self {
        if shuffle {
            return Self::Shuffle;
        }
        match repeat {
            PlayMode::Off => Self::Sequential,
            PlayMode::List => Self::RepeatList,
            PlayMode::One => Self::RepeatOne,
        }
    }

    fn next_parts(self) -> (bool, PlayMode) {
        match self {
            Self::Sequential => (false, PlayMode::List),
            Self::RepeatList => (false, PlayMode::One),
            Self::RepeatOne => (true, PlayMode::Off),
            Self::Shuffle => (false, PlayMode::Off),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayLayout {
    /// Cover fills the height, lyrics beside it.
    Side,
    /// Cover centered on top, lyrics below.
    Stacked,
}

impl PlayLayout {
    fn from_config(value: &str) -> Self {
        match value {
            "stacked" => Self::Stacked,
            _ => Self::Side,
        }
    }
}

pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MarqueeTarget {
    view: View,
    source: Source,
    underlying: usize,
    id: i64,
    title: String,
    artist: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectionCoverKey {
    view: View,
    source: Source,
    underlying: usize,
    id: i64,
    pic_url: Option<String>,
    style_revision: u64,
    original: bool,
}

struct SelectionCoverState {
    generation: u64,
    key: Option<SelectionCoverKey>,
    pixel: Option<PixelCover>,
    pixel_key: Option<String>,
    placeholder: PixelCover,
}

#[derive(Default)]
struct HotPixelCovers {
    entries: VecDeque<(String, PixelCover)>,
}

impl HotPixelCovers {
    fn get(&mut self, key: &str) -> Option<PixelCover> {
        let index = self.entries.iter().position(|(cached, _)| cached == key)?;
        let entry = self.entries.remove(index)?;
        let cover = entry.1.clone();
        self.entries.push_back(entry);
        Some(cover)
    }

    fn insert(&mut self, key: String, cover: PixelCover) {
        if let Some(index) = self.entries.iter().position(|(cached, _)| cached == &key) {
            self.entries.remove(index);
        }
        self.entries.push_back((key, cover));
        while self.entries.len() > HOT_PIXEL_COVER_LIMIT {
            self.entries.pop_front();
        }
    }
}

pub struct AppState {
    pub view: View,
    pub zen: bool,
    /// Restored sessions land on the dashboard first; the now-playing
    /// layout appears once the user actually engages with playback.
    pub dashboard_hold: bool,
    /// Mirrors terminal mouse capture; the `mouse` palette command
    /// releases it so native text selection works.
    pub(crate) mouse_captured: bool,
    pub theme: Theme,
    pub(crate) terminal_background: Option<Color>,
    pub(crate) terminal_is_light: Option<bool>,
    pub config: Config,
    pub(crate) settings: settings::SettingsState,
    pub(crate) command_palette: CommandPaletteState,
    pub library: Vec<SongRow>,
    pub selected: usize,
    marquee_frame: u64,
    marquee_target: Option<MarqueeTarget>,
    pub queue: Vec<SongRow>,
    pub queue_pos: Option<usize>,
    queue_source: Source,
    pub play_mode: PlayMode,
    pub shuffle: bool,
    shuffle_order: Vec<usize>,
    shuffle_cursor: usize,
    pub(crate) filter: ListFilter,
    pub liked: std::collections::HashSet<i64>,
    like_mutations: std::collections::HashMap<i64, u64>,
    like_in_flight: std::collections::HashMap<i64, u64>,
    pub current_track_id: Option<i64>,
    active_row: Option<SongRow>,
    pub library_source: Source,
    pub sidebar_focus: bool,
    pub sidebar_selected: usize,
    pending_fm_next: bool,
    fm_request_pending: bool,
    cover_prefetched: bool,
    /// Next queue item resolved ahead of time — track switches feel instant.
    prefetched: Option<(usize, api::ResolvedTrack)>,
    enter_replaces_queue: bool,
    pub session: SessionState,
    pub search: SearchState,
    pub now: Option<NowPlaying>,
    pub cover: Option<PixelCover>,
    pub layout: PlayLayout,
    pub thick_progress: bool,
    pixel_detail_scale: f32,
    original_cover: Option<OriginalCover>,
    selected_original_cover: Option<OriginalCover>,
    selected_cover: SelectionCoverState,
    hot_pixel_covers: HotPixelCovers,
    pub idle_art: PixelCover,
    pub library_synced: bool,
    library_request: u64,
    /// Idle art pre-rendered at the playing-cover size, so the loading
    /// placeholder swaps to the real cover without a size jump.
    pub placeholder: Option<PixelCover>,
    /// Source image for the idle art; None = procedural vinyl.
    idle_bytes: Option<Vec<u8>>,
    idle_path: Option<std::path::PathBuf>,
    pub lyrics: Vec<crate::lyrics::LyricLine>,
    pub position: Duration,
    pub duration: Option<Duration>,
    pub paused: bool,
    pub volume: f32,
    volume_before_mute: Option<f32>,
    resume_on_play: Option<Duration>,
    seek_after_start: Option<Duration>,
    pub status: Option<String>,
    pub(crate) command_feedback: Option<String>,
    pub(crate) command_feedback_error: bool,
    command_feedback_ticks: u8,
    pub generation: u64,
    style_revision: u64,
    terminal_size: (u16, u16),
    pub confirm_quit: bool,
    pub show_help: bool,
    pub spectrum: SpectrumView,
    pending_g: bool,
    pending_auto_next: bool,
    should_quit: bool,
}

impl AppState {
    pub fn new(config: &Config) -> Self {
        let theme = Theme::by_name(&config.theme);
        let terminal_size = crossterm::terminal::size().unwrap_or((80, 24));
        let idle_cells = desired_idle_cells(terminal_size);
        let idle_path = config.idle_art.as_deref().map(shellexpand_home);
        let idle_bytes = idle_path.is_none().then(|| LOGO_BYTES.to_vec());
        Self {
            view: View::NowPlaying,
            zen: false,
            theme,
            terminal_background: None,
            terminal_is_light: None,
            config: config.clone(),
            settings: settings::SettingsState::default(),
            command_palette: CommandPaletteState::default(),
            // Demo rows for the logged-out state; replaced by 我喜欢的音乐
            // right after login (id 0 = resolve via search).
            library: vec![
                SongRow {
                    id: 0,
                    title: "反方向的钟".into(),
                    artist: String::new(),
                    album: String::new(),
                    duration_ms: 0,
                    pic_url: None,
                },
                SongRow {
                    id: 0,
                    title: "海阔天空".into(),
                    artist: "Beyond".into(),
                    album: String::new(),
                    duration_ms: 0,
                    pic_url: None,
                },
            ],
            selected: 0,
            marquee_frame: 0,
            marquee_target: None,
            queue: Vec::new(),
            queue_pos: None,
            queue_source: Source::Liked,
            play_mode: PlayMode::Off,
            shuffle: false,
            shuffle_order: Vec::new(),
            shuffle_cursor: 0,
            filter: ListFilter::default(),
            liked: std::collections::HashSet::new(),
            like_mutations: std::collections::HashMap::new(),
            like_in_flight: std::collections::HashMap::new(),
            current_track_id: None,
            active_row: None,
            library_source: Source::Liked,
            sidebar_focus: false,
            sidebar_selected: 0,
            pending_fm_next: false,
            fm_request_pending: false,
            cover_prefetched: false,
            prefetched: None,
            enter_replaces_queue: config.enter_replaces_queue,
            session: SessionState::default(),
            search: SearchState::new(),
            now: None,
            cover: None,
            layout: PlayLayout::from_config(&config.layout),
            thick_progress: config.progress_style == "bar",
            pixel_detail_scale: config.pixel_scale.clamp(0.5, 4.0),
            original_cover: None,
            selected_original_cover: None,
            selected_cover: SelectionCoverState {
                generation: 0,
                key: None,
                pixel: None,
                pixel_key: None,
                placeholder: pixel::vinyl(
                    theme.palette,
                    theme.bg,
                    PREVIEW_CELLS.0,
                    PREVIEW_CELLS.1,
                    config.cover_detail,
                ),
            },
            hot_pixel_covers: HotPixelCovers::default(),
            idle_art: pixel::vinyl(
                theme.palette,
                theme.bg,
                idle_cells.0,
                idle_cells.1,
                config.cover_detail,
            ),
            library_synced: false,
            library_request: 0,
            placeholder: None,
            idle_bytes,
            idle_path,
            lyrics: Vec::new(),
            position: Duration::ZERO,
            duration: None,
            paused: false,
            volume: 1.0,
            volume_before_mute: None,
            resume_on_play: None,
            dashboard_hold: false,
            mouse_captured: true,
            seek_after_start: None,
            status: None,
            command_feedback: None,
            command_feedback_error: false,
            command_feedback_ticks: 0,
            generation: 0,
            style_revision: 0,
            terminal_size,
            confirm_quit: false,
            show_help: false,
            spectrum: SpectrumView::new(config.spectrum_style),
            pending_g: false,
            pending_auto_next: false,
            should_quit: false,
        }
    }

    pub(crate) fn set_terminal_background(&mut self, background: Option<Color>) {
        self.terminal_background = background;
        self.spectrum.set_terminal_background(background);
    }

    pub(crate) fn selection_style(&self) -> Style {
        self.theme.selection_style(self.terminal_background)
    }

    /// The cover cell grid that fits the current terminal and layout.
    /// Height-driven in Side layout, width-bounded in Stacked.
    fn desired_cover_cells(&self) -> (u16, u16) {
        let (cols, rows) = self.terminal_size;
        let shell_rows = if self.zen {
            0
        } else {
            ui::HEADER_HEIGHT + ui::FOOTER_HEIGHT + ui::PANEL_GAP_Y * 2
        };
        let playing_rows = rows.saturating_sub(shell_rows);
        let spectrum_rows = ui::now_playing::spectrum_band_height(
            playing_rows,
            self.config.spectrum_enabled,
            self.layout,
        );
        let main_rows = playing_rows
            .saturating_sub(ui::now_playing::PROGRESS_HEIGHT)
            .saturating_sub(spectrum_rows);
        let height = match self.layout {
            PlayLayout::Side => main_rows,
            PlayLayout::Stacked => main_rows / 2,
        };
        let height = height.clamp(8, 40);
        let content_width = ui::centered_content(Rect::new(0, 0, cols, rows)).width;
        let width = (height * 2).min(match self.layout {
            PlayLayout::Side => content_width
                .saturating_sub(ui::now_playing::SIDE_PANEL_RESERVED_COLS)
                .max(16),
            PlayLayout::Stacked => content_width.saturating_sub(4).max(16),
        });
        (width, width / 2)
    }

    fn sidebar_visible(&self) -> bool {
        self.terminal_size.0 >= ui::library::COLLAPSE_BELOW
    }

    pub(crate) fn visible_rows<'a>(&self, rows: &'a [SongRow]) -> Vec<(usize, &'a SongRow)> {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| self.filter.matches(row))
            .collect()
    }

    fn visible_len(&self) -> usize {
        match self.view {
            View::Library => self.visible_rows(&self.library).len(),
            View::Queue => self.visible_rows(&self.queue).len(),
            View::Search => self.search.song_rows().map_or_else(
                || self.search.current_len(),
                |rows| self.visible_rows(rows).len(),
            ),
            _ => 0,
        }
    }

    pub(crate) fn visible_row(&self, index: usize) -> Option<(usize, SongRow)> {
        let rows = match self.view {
            View::Library => self.library.as_slice(),
            View::Queue => self.queue.as_slice(),
            View::Search => self.search.song_rows()?,
            _ => return None,
        };
        self.visible_rows(rows)
            .get(index)
            .map(|(underlying, row)| (*underlying, (*row).clone()))
    }

    fn active_marquee_target(&self) -> Option<MarqueeTarget> {
        if self.terminal_size.1 < 8 {
            let now = self.now.as_ref()?;
            if !ui::mini_player::marquee_needed(self, self.terminal_size.0) {
                return None;
            }
            return Some(MarqueeTarget {
                view: View::NowPlaying,
                source: self.queue_source,
                underlying: self.queue_pos.unwrap_or(usize::MAX),
                id: self.current_track_id.unwrap_or_default(),
                title: now.title.clone(),
                artist: now.artist.clone(),
            });
        }
        if self.show_help
            || self.confirm_quit
            || self.command_palette.open
            || self.filter.input
            || self.view == View::Library && self.sidebar_focus
            || self.view == View::Search && self.search.input
        {
            return None;
        }
        let (underlying, row) = self.visible_row(self.selected)?;
        let shell_width =
            ui::centered_content(Rect::new(0, 0, self.terminal_size.0, self.terminal_size.1)).width;
        let preview_visible = self.selection_preview_visible();
        let needs_marquee = match self.view {
            View::Library => ui::library::marquee_needed(&row, shell_width, preview_visible),
            View::Search => ui::search::marquee_needed(&row, shell_width, preview_visible),
            View::Queue => ui::queue::marquee_needed(&row, shell_width),
            _ => false,
        };
        if !needs_marquee {
            return None;
        }
        Some(MarqueeTarget {
            view: self.view,
            source: match self.view {
                View::Library => self.library_source,
                View::Search => Source::Search,
                View::Queue => self.queue_source,
                _ => return None,
            },
            underlying,
            id: row.id,
            title: row.title,
            artist: row.artist,
        })
    }

    fn advance_marquee(&mut self) {
        let target = self.active_marquee_target();
        if target.is_some() && target == self.marquee_target {
            self.marquee_frame = self.marquee_frame.wrapping_add(1);
        } else {
            self.marquee_target = target;
            self.marquee_frame = 0;
        }
    }

    fn set_command_feedback(&mut self, message: String, is_error: bool) {
        self.command_feedback = Some(message);
        self.command_feedback_error = is_error;
        self.command_feedback_ticks = 24;
    }

    fn clear_command_feedback(&mut self) {
        self.command_feedback = None;
        self.command_feedback_error = false;
        self.command_feedback_ticks = 0;
    }

    fn advance_command_feedback(&mut self) {
        if self.command_feedback_ticks == 0 {
            self.command_feedback = None;
            self.command_feedback_error = false;
            return;
        }
        self.command_feedback_ticks -= 1;
        if self.command_feedback_ticks == 0 {
            self.command_feedback = None;
            self.command_feedback_error = false;
        }
    }

    fn reconcile_marquee(&mut self) {
        let target = self.active_marquee_target();
        if target != self.marquee_target {
            self.marquee_target = target;
            self.marquee_frame = 0;
        }
    }

    pub(crate) fn marquee_frame(&self) -> u64 {
        if self.active_marquee_target() == self.marquee_target {
            self.marquee_frame
        } else {
            0
        }
    }

    fn marquee_active(&self) -> bool {
        self.active_marquee_target().is_some()
    }

    fn visible_rows_owned(&self) -> Vec<SongRow> {
        let rows = match self.view {
            View::Library => self.library.as_slice(),
            View::Queue => self.queue.as_slice(),
            View::Search => match self.search.song_rows() {
                Some(rows) => rows,
                None => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        self.visible_rows(rows)
            .into_iter()
            .map(|(_, row)| row.clone())
            .collect()
    }

    fn list_page_size(&self) -> usize {
        let body = self.terminal_size.1.saturating_sub(2) as usize;
        match self.view {
            View::Library => body.saturating_sub(1).max(1),
            View::Search => ui::search::page_size(self, self.terminal_size.1),
            View::Queue => body.max(1),
            _ => 1,
        }
    }

    fn clear_filter(&mut self) {
        self.filter.clear();
        self.selected = 0;
    }

    fn reset_shuffle_order(&mut self) {
        self.shuffle_order.clear();
        self.shuffle_cursor = 0;
        if self.shuffle {
            if let Some(current) = self.queue_pos.filter(|index| *index < self.queue.len()) {
                self.shuffle_order = shuffled_order(self.queue.len(), current);
            }
        }
    }

    fn ensure_shuffle_order(&mut self) {
        let current = self.queue_pos;
        let valid = current.is_some_and(|current| {
            self.shuffle_order.len() == self.queue.len()
                && self.shuffle_order.get(self.shuffle_cursor) == Some(&current)
        });
        if !valid {
            self.reset_shuffle_order();
        }
    }

    fn playback_snapshot(&self) -> crate::store::StoredPlayback {
        crate::store::StoredPlayback {
            queue: self
                .queue
                .iter()
                .map(crate::store::StoredSong::from)
                .collect(),
            current: self.active_row.as_ref().map(crate::store::StoredSong::from),
            queue_pos: self.queue_pos,
            position_ms: self.position.as_millis().min(u128::from(u64::MAX)) as u64,
            volume: self.volume,
            volume_before_mute: self.volume_before_mute,
            play_mode: self.play_mode,
            shuffle: self.shuffle,
            queue_source: self.queue_source,
        }
    }

    fn restore_playback(&mut self, playback: crate::store::StoredPlayback) {
        self.queue = playback
            .queue
            .into_iter()
            .map(crate::store::StoredSong::into_song_row)
            .collect();
        self.queue_pos = playback.queue_pos.filter(|index| *index < self.queue.len());
        self.position = Duration::from_millis(playback.position_ms);
        self.volume = playback.volume.clamp(0.0, 1.5);
        self.volume_before_mute = playback.volume_before_mute;
        self.play_mode = playback.play_mode;
        self.shuffle = playback.shuffle;
        self.queue_source = playback.queue_source;
        self.paused = true;
        self.status = None;
        self.active_row = playback
            .current
            .map(crate::store::StoredSong::into_song_row);
        if let (Some(active), Some(queued)) = (
            self.active_row.as_mut(),
            self.queue_pos.and_then(|index| self.queue.get(index)),
        ) {
            if active.id == queued.id
                && active.title.trim().is_empty()
                && !queued.title.trim().is_empty()
            {
                active.title.clone_from(&queued.title);
            }
        }
        if let Some(row) = &self.active_row {
            self.current_track_id = (row.id > 0).then_some(row.id);
            self.duration =
                (row.duration_ms > 0).then(|| Duration::from_millis(row.duration_ms as u64));
            self.now = Some(NowPlaying {
                title: row.title.clone(),
                artist: row.artist.clone(),
                album: row.album.clone(),
            });
            self.resume_on_play = Some(self.position);
            // Land on the dashboard; 1/Space/next reveal the player.
            self.dashboard_hold = true;
        } else {
            self.current_track_id = None;
            self.duration = None;
            self.now = None;
            self.resume_on_play = None;
        }
        self.reset_shuffle_order();
    }

    fn toggle_play(&mut self, fx: &Effects) {
        self.dashboard_hold = false;
        let Some(position) = self.resume_on_play else {
            fx.player.send(PlayerCommand::TogglePause);
            return;
        };
        let Some(row) = self.active_row.clone() else {
            return;
        };
        self.play_row(fx, row);
        self.position = position;
        self.resume_on_play = Some(position);
        self.seek_after_start = Some(position);
    }

    fn desired_idle_cells(&self) -> (u16, u16) {
        desired_idle_cells(self.terminal_size)
    }

    fn ensure_placeholder(&mut self) {
        let desired = self.desired_cover_cells();
        let current = self.placeholder.as_ref().map(|art| (art.width, art.height));
        if current != Some(desired) {
            self.placeholder = Some(pixel::vinyl(
                self.theme.palette,
                self.theme.bg,
                desired.0,
                desired.1,
                self.config.cover_detail,
            ));
        }
    }

    fn clear_cover(&mut self) {
        self.cover = None;
        if let Some(original) = &mut self.original_cover {
            original.clear();
        }
    }

    fn clear_selected_cover(&mut self) {
        self.selected_cover.pixel = None;
        self.selected_cover.pixel_key = None;
        if let Some(original) = &mut self.selected_original_cover {
            original.clear();
        }
    }

    fn load_idle_art(&mut self, fx: &Effects) {
        if let Some(bytes) = self.idle_bytes.clone() {
            spawn_render_idle(
                fx,
                bytes,
                self.desired_idle_cells(),
                self.pixel_style(),
                self.style_revision,
            );
        } else if let Some(path) = self.idle_path.clone() {
            spawn_idle_load(fx, path);
        }
    }

    pub fn original_cover_is_current(&self) -> bool {
        self.config.cover_mode == CoverMode::Original
            && self
                .original_cover
                .as_ref()
                .is_some_and(|cover| cover.generation == Some(self.generation))
    }

    pub fn render_original_cover(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        if let Some(original) = &mut self.original_cover {
            frame.render_stateful_widget(StatefulImage::new(), area, &mut original.protocol);
        }
    }

    fn apply_original_resize(&mut self, response: ResizeResponse) {
        if let Some(original) = &mut self.original_cover {
            original.protocol.update_resized_protocol(response);
        }
    }

    pub(crate) fn preview_placeholder(&self) -> &PixelCover {
        &self.selected_cover.placeholder
    }

    pub(crate) fn selected_pixel_cover(&self) -> Option<&PixelCover> {
        self.selected_cover.pixel.as_ref()
    }

    pub(crate) fn selected_original_is_available(&self) -> bool {
        self.selected_cover
            .key
            .as_ref()
            .is_some_and(|key| key.original)
            && self
                .selected_original_cover
                .as_ref()
                .is_some_and(|cover| cover.generation.is_some() || cover.has_pending())
    }

    pub(crate) fn render_selected_original(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        if let Some(original) = &mut self.selected_original_cover {
            original.render(frame, area);
        }
    }

    fn apply_selected_original_resize(&mut self, response: ResizeResponse) {
        if let Some(original) = &mut self.selected_original_cover {
            original.protocol.update_resized_protocol(response);
        }
    }

    fn apply_selected_pending_resize(&mut self, response: ResizeResponse) {
        if let Some(original) = &mut self.selected_original_cover {
            original.update_pending(response, self.selected_cover.generation);
        }
    }

    fn uses_original_cover(&self, surface: CoverSurface) -> bool {
        if self.config.cover_mode != CoverMode::Original {
            return false;
        }
        match surface {
            CoverSurface::Playing => self.original_cover.is_some(),
            CoverSurface::Selection => self.selected_original_cover.is_some(),
        }
    }

    fn cover_request(
        &self,
        surface: CoverSurface,
        generation: u64,
        song_id: i64,
        pic_url: &str,
        cells: (u16, u16),
    ) -> CoverRenderRequest {
        CoverRenderRequest {
            surface,
            generation,
            cells,
            style_revision: self.style_revision,
            song_id,
            source_key: CoverCache::original_key(pic_url, COVER_SOURCE_EDGE),
        }
    }

    fn pixel_cover_key(&self, request: &CoverRenderRequest) -> String {
        CoverCache::pixel_key(PixelKeyInputs {
            song_id: request.song_id,
            original_key: &request.source_key,
            cells: request.cells,
            detail_scale: self.pixel_detail_scale,
            detail: self.config.cover_detail,
            palette_mode: self.config.cover_palette,
            background: self.theme.bg,
            palette: self.theme.palette,
        })
    }

    fn pixel_style(&self) -> PixelStyle {
        PixelStyle {
            palette_mode: self.config.cover_palette,
            palette: self.theme.palette,
            background: self.theme.bg,
            detail_scale: self.pixel_detail_scale,
            detail: self.config.cover_detail,
        }
    }

    fn selection_cover_is_ready(&self, row: &SongRow) -> bool {
        let Some(key) = self.selected_cover.key.as_ref() else {
            return false;
        };
        let Some(pic_url) = row.pic_url.as_deref() else {
            return true;
        };
        if key.original {
            return self
                .selected_original_cover
                .as_ref()
                .is_some_and(|cover| cover.generation == Some(self.selected_cover.generation));
        }
        let request = self.cover_request(
            CoverSurface::Selection,
            self.selected_cover.generation,
            row.id,
            pic_url,
            PREVIEW_CELLS,
        );
        let pixel_key = self.pixel_cover_key(&request);
        self.selected_cover.pixel_key.as_deref() == Some(pixel_key.as_str())
    }

    fn load_playing_cover(&mut self, fx: &Effects, row: &SongRow) {
        let Some(pic_url) = row.pic_url.as_deref() else {
            return;
        };
        let request = self.cover_request(
            CoverSurface::Playing,
            self.generation,
            row.id,
            pic_url,
            self.desired_cover_cells(),
        );
        spawn_cover_load(
            fx,
            request,
            pic_url.to_owned(),
            CoverStyle {
                pixel: self.pixel_style(),
                original: self.uses_original_cover(CoverSurface::Playing),
            },
        );
    }

    fn selection_preview_visible(&self) -> bool {
        let (cols, rows) = self.terminal_size;
        let body_height = rows.saturating_sub(
            crate::ui::HEADER_HEIGHT + crate::ui::FOOTER_HEIGHT + crate::ui::PANEL_GAP_Y * 2,
        );
        match self.view {
            View::Library => {
                cols >= crate::ui::library::PREVIEW_MIN_TERMINAL_WIDTH
                    && body_height >= crate::ui::cover_preview::HEIGHT
            }
            View::Search => {
                cols >= crate::ui::search::PREVIEW_MIN_TERMINAL_WIDTH
                    && body_height >= crate::ui::cover_preview::HEIGHT
                    && self.search.song_rows().is_some()
            }
            _ => false,
        }
    }

    fn selected_cover_candidate(&self) -> Option<(SelectionCoverKey, SongRow, Vec<SongRow>)> {
        if !self.selection_preview_visible()
            || self.command_palette.open
            || self.filter.input
            || self.view == View::Library && self.sidebar_focus
            || self.view == View::Search
                && (self.search.input
                    || self.search.current_searching()
                    || self.search.current_error().is_some())
        {
            return None;
        }
        let (underlying, row) = self.visible_row(self.selected)?;
        let source = if self.view == View::Library {
            self.library_source
        } else {
            Source::Search
        };
        let key = SelectionCoverKey {
            view: self.view,
            source,
            underlying,
            id: row.id,
            pic_url: row.pic_url.clone(),
            style_revision: self.style_revision,
            original: self.uses_original_cover(CoverSurface::Selection),
        };
        let rows = match self.view {
            View::Library => self.library.as_slice(),
            View::Search => self.search.song_rows()?,
            _ => return None,
        };
        let visible = self.visible_rows(rows);
        let start = self.selected.saturating_sub(3);
        let end = (self.selected + 4).min(visible.len());
        let mut neighbors: Vec<SongRow> = Vec::new();
        for (_, neighbor) in &visible[start..end] {
            if neighbor.pic_url.is_none() {
                continue;
            }
            if neighbor.id != row.id && !neighbors.iter().any(|cached| cached.id == neighbor.id) {
                neighbors.push((*neighbor).clone());
            }
        }
        Some((key, row, neighbors))
    }

    fn reconcile_selected_cover(&mut self, fx: &Effects) {
        let candidate = self.selected_cover_candidate();
        let key = candidate.as_ref().map(|(key, _, _)| key);
        if self.selected_cover.key.as_ref() == key {
            return;
        }
        self.selected_cover.generation = self.selected_cover.generation.wrapping_add(1);
        self.selected_cover.key = candidate.as_ref().map(|(key, _, _)| key.clone());
        if let Some(original) = &mut self.selected_original_cover {
            original.cancel_pending();
        }
        let Some((key, row, neighbors)) = candidate else {
            self.clear_selected_cover();
            return;
        };

        let generation = self.selected_cover.generation;
        let load = row.pic_url.as_deref().map(|pic_url| CoverLoad {
            request: self.cover_request(
                CoverSurface::Selection,
                generation,
                row.id,
                pic_url,
                PREVIEW_CELLS,
            ),
            style: CoverStyle {
                pixel: self.pixel_style(),
                original: key.original,
            },
        });
        let hot = if key.original {
            false
        } else if let Some(load) = load.as_ref() {
            let pixel_key = self.pixel_cover_key(&load.request);
            if let Some(cover) = self.hot_pixel_covers.get(&pixel_key) {
                self.selected_cover.pixel = Some(cover);
                self.selected_cover.pixel_key = Some(pixel_key);
                true
            } else {
                false
            }
        } else {
            false
        };
        if load.is_none() && neighbors.is_empty() {
            return;
        }
        spawn_selection_cover_lookup(fx, generation, load, row, neighbors, hot);
    }

    fn apply_resolved(&mut self, fx: &Effects, generation: u64, track: api::ResolvedTrack) {
        let row = song_row_from_resolved(&track);
        let track_id = track.id;
        let cache_request = cache_write_request(&track);
        self.active_row = Some(row.clone());
        self.current_track_id = Some(track_id);
        self.now = Some(NowPlaying {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
        });
        self.duration =
            (track.duration_ms > 0).then(|| Duration::from_millis(track.duration_ms as u64));
        self.status = Some(if track.media.is_unm() {
            i18n::t(Key::UnmSourceUsed).into()
        } else {
            i18n::t_playing(&track.kind)
        });
        match track.media {
            api::ResolvedMedia::NeteaseUrl(url) => {
                fx.player.send(PlayerCommand::PlayUrl {
                    generation,
                    url,
                    cache: fx.cache_root.clone().map(|root| player::CacheWritePlan {
                        root,
                        request: cache_request,
                    }),
                    unm_source: false,
                });
            }
            api::ResolvedMedia::UnmUrl(url) => fx.player.send(PlayerCommand::PlayUrl {
                generation,
                url,
                cache: None,
                unm_source: true,
            }),
            api::ResolvedMedia::UnmBytes(bytes) => fx.player.send(PlayerCommand::PlayBytes {
                generation,
                bytes,
                unm_source: true,
            }),
        }
        if !self.cover_prefetched {
            self.load_playing_cover(fx, &row);
        }
        spawn_fetch_lyrics(fx, generation, track_id);
        self.prefetch_next(fx);
    }

    fn apply_cached(&mut self, fx: &Effects, generation: u64, row: SongRow, lease: CacheLease) {
        let kind = lease.metadata().codec.extension();
        self.active_row = Some(row.clone());
        self.current_track_id = Some(row.id);
        self.now = Some(NowPlaying {
            title: row.title.clone(),
            artist: row.artist.clone(),
            album: row.album.clone(),
        });
        self.duration =
            (row.duration_ms > 0).then(|| Duration::from_millis(row.duration_ms as u64));
        self.status = Some(i18n::t_playing(kind));
        fx.player
            .send(PlayerCommand::PlayCached { generation, lease });
        if !self.cover_prefetched {
            self.load_playing_cover(fx, &row);
        }
        spawn_fetch_lyrics(fx, generation, row.id);
        self.prefetch_next(fx);
    }

    fn prepare_resolved(&mut self, fx: &Effects, generation: u64, track: api::ResolvedTrack) {
        if let Some(root) = fx.cache_root.clone() {
            spawn_resolved_cache_lookup(fx, generation, root, track);
        } else {
            self.apply_resolved(fx, generation, track);
        }
    }

    /// Resolve the sequential next queue item ahead of time.
    fn prefetch_next(&mut self, fx: &Effects) {
        if self.shuffle {
            return;
        }
        let Some(position) = self.queue_pos else {
            return;
        };
        let next = if position + 1 < self.queue.len() {
            position + 1
        } else if self.play_mode == PlayMode::List && self.queue_source != Source::Fm {
            0
        } else {
            return;
        };
        if self.prefetched.as_ref().is_some_and(|(i, _)| *i == next) {
            return;
        }
        if let Some(row) = self.queue.get(next).cloned() {
            if row.id > 0 {
                spawn_prefetch(fx, next, row);
            }
        }
    }

    /// Reset the now-playing surface and kick off resolution for a row.
    fn play_row(&mut self, fx: &Effects, row: SongRow) {
        fx.player.send(PlayerCommand::Stop);
        self.dashboard_hold = false;
        self.current_track_id = (row.id > 0).then_some(row.id);
        self.active_row = Some(row.clone());
        self.paused = false;
        self.resume_on_play = None;
        self.seek_after_start = None;
        self.clear_cover();
        self.ensure_placeholder();
        self.generation += 1;
        self.now = Some(NowPlaying {
            title: row.title.clone(),
            artist: row.artist.clone(),
            album: row.album.clone(),
        });
        self.lyrics.clear();
        self.position = Duration::ZERO;
        self.duration = None;
        self.status = Some(i18n::t(Key::Resolving).into());
        // Cover art is independent of URL resolution: start fetching now
        // (queue rows carry pic_url) instead of waiting for TrackResolved.
        self.cover_prefetched = row.pic_url.is_some();
        self.load_playing_cover(fx, &row);
        // Prefetched? Play instantly, skip the resolve round-trip.
        if let Some((_, track)) = self
            .prefetched
            .take_if(|(_, track)| track.id == row.id && row.id > 0)
        {
            let generation = self.generation;
            self.prepare_resolved(fx, generation, track);
            return;
        }
        if row.id > 0 {
            if let Some(root) = fx.cache_root.clone() {
                spawn_row_cache_lookup(
                    fx,
                    self.generation,
                    root,
                    CacheKey::new(row.id, fx.ncm.quality()),
                    row,
                );
                return;
            }
        }
        spawn_resolve(fx, self.generation, row);
    }

    /// Move within the queue (manual n/p and end-of-track auto-advance).
    fn step_queue(&mut self, fx: &Effects, delta: i32, auto: bool, allow_list_wrap: bool) -> bool {
        let Some(position) = self.queue_pos else {
            return false;
        };
        if self.queue.is_empty() {
            return false;
        }
        if auto && self.play_mode == PlayMode::One {
            if let Some(row) = self.queue.get(position).cloned() {
                self.play_row(fx, row);
                return true;
            }
            return false;
        }
        let next = if self.shuffle {
            self.ensure_shuffle_order();
            if delta >= 0 {
                if self.shuffle_cursor + 1 < self.shuffle_order.len() {
                    self.shuffle_cursor += 1;
                    self.shuffle_order.get(self.shuffle_cursor).copied()
                } else if self.queue_source == Source::Fm {
                    self.pending_fm_next = true;
                    self.fetch_fm_more(fx);
                    return true;
                } else if allow_list_wrap && self.play_mode == PlayMode::List {
                    self.shuffle_order = shuffled_order(self.queue.len(), position);
                    self.shuffle_cursor = usize::from(self.shuffle_order.len() > 1);
                    self.shuffle_order.get(self.shuffle_cursor).copied()
                } else {
                    None
                }
            } else if self.shuffle_cursor > 0 {
                self.shuffle_cursor -= 1;
                self.shuffle_order.get(self.shuffle_cursor).copied()
            } else if allow_list_wrap && self.play_mode == PlayMode::List {
                self.shuffle_order = shuffled_order(self.queue.len(), position);
                self.shuffle_cursor = self.shuffle_order.len().saturating_sub(1);
                self.shuffle_order.get(self.shuffle_cursor).copied()
            } else {
                None
            }
        } else {
            let candidate = position as i32 + delta;
            if candidate >= 0 && (candidate as usize) < self.queue.len() {
                Some(candidate as usize)
            } else if self.queue_source == Source::Fm && delta > 0 {
                self.pending_fm_next = true;
                self.fetch_fm_more(fx);
                return true;
            } else if allow_list_wrap && self.play_mode == PlayMode::List {
                Some(if delta >= 0 { 0 } else { self.queue.len() - 1 })
            } else {
                None
            }
        };
        match next.and_then(|next| self.queue.get(next).cloned().map(|row| (next, row))) {
            Some((next, row)) => {
                self.queue_pos = Some(next);
                self.play_row(fx, row);
                true
            }
            None => {
                self.status = Some(i18n::t(Key::QueueFinished).into());
                false
            }
        }
    }

    fn handle_track_unavailable(&mut self, fx: &Effects) {
        if !self.step_queue(fx, 1, false, false) {
            self.paused = true;
            self.resume_on_play = Some(Duration::ZERO);
        }
        self.status = Some(i18n::t(Key::TrackUnavailable).into());
    }

    fn apply_player_event(&mut self, fx: &Effects, event: PlayerEvent) {
        match event {
            PlayerEvent::Started { generation, total } => {
                if generation == self.generation {
                    if let Some(position) = self.seek_after_start.take() {
                        self.position = position;
                        fx.player.send(PlayerCommand::SeekTo(position));
                        self.resume_on_play = None;
                    } else {
                        self.position = Duration::ZERO;
                    }
                    fx.player.send(PlayerCommand::Play);
                    self.duration = total;
                    self.paused = false;
                }
            }
            PlayerEvent::Position {
                generation,
                position,
            } => {
                if generation == self.generation {
                    self.position = position;
                }
            }
            PlayerEvent::Paused { generation, paused } => {
                if generation == self.generation {
                    self.paused = paused;
                }
            }
            PlayerEvent::Ended { generation } => {
                if generation == self.generation {
                    self.pending_auto_next = true;
                }
            }
            PlayerEvent::Failed {
                generation,
                message,
                cached,
                unm_source,
            } => {
                if generation == self.generation {
                    if let (Some(metadata), Some(row)) = (cached, self.active_row.clone()) {
                        self.status = Some(i18n::t(Key::Resolving).into());
                        spawn_cache_fallback(fx, generation, metadata, row);
                    } else if unm_source {
                        self.handle_track_unavailable(fx);
                    } else {
                        self.status = Some(message);
                    }
                }
            }
        }
    }
}

fn song_row_from_resolved(track: &api::ResolvedTrack) -> SongRow {
    SongRow {
        id: track.id,
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        duration_ms: track.duration_ms,
        pic_url: track.pic_url.clone(),
    }
}

fn cache_write_request(track: &api::ResolvedTrack) -> CacheWriteRequest {
    let mut request = CacheWriteRequest::new(track.cache_key, track.codec, track.actual_bitrate);
    if let Some(bytes) = track.expected_bytes {
        request = request.with_expected_bytes(bytes);
    }
    if let Some(md5) = track.expected_md5 {
        request = request.with_expected_md5(md5);
    }
    request
}

fn spawn_resolve(fx: &Effects, generation: u64, row: SongRow) {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let resolved = ncm.resolve_for_playback(&row).await;
        let action = match resolved {
            Ok(track) => Action::TrackResolved { generation, track },
            Err(error) if error.is::<api::TrackUnavailable>() => {
                Action::TrackUnavailable { generation }
            }
            Err(error) => Action::ResolveFailed {
                generation,
                message: error.to_string(),
            },
        };
        let _ = actions.send(action);
    });
}

fn spawn_row_cache_lookup(
    fx: &Effects,
    generation: u64,
    root: std::path::PathBuf,
    key: CacheKey,
    row: SongRow,
) {
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let lease = tokio::task::spawn_blocking(move || {
            TrackCache::open(root).and_then(|cache| cache.lookup(key))
        })
        .await;
        let lease = match lease {
            Ok(Ok(lease)) => lease,
            Ok(Err(error)) => {
                tracing::warn!(%error, "audio cache lookup failed");
                None
            }
            Err(error) => {
                tracing::warn!(%error, "audio cache worker failed");
                None
            }
        };
        let _ = actions.send(Action::RowCacheReady {
            generation,
            row,
            lease,
        });
    });
}

fn spawn_resolved_cache_lookup(
    fx: &Effects,
    generation: u64,
    root: std::path::PathBuf,
    track: api::ResolvedTrack,
) {
    let key = track.cache_key;
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let lease = tokio::task::spawn_blocking(move || {
            TrackCache::open(root).and_then(|cache| cache.lookup(key))
        })
        .await;
        let lease = match lease {
            Ok(Ok(lease)) => lease,
            Ok(Err(error)) => {
                tracing::warn!(%error, "audio cache lookup failed");
                None
            }
            Err(error) => {
                tracing::warn!(%error, "audio cache worker failed");
                None
            }
        };
        let _ = actions.send(Action::ResolvedCacheReady {
            generation,
            track,
            lease,
        });
    });
}

fn spawn_cache_fallback(fx: &Effects, generation: u64, metadata: CacheMetadata, row: SongRow) {
    let Some(root) = fx.cache_root.clone() else {
        spawn_resolve(fx, generation, row);
        return;
    };
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let invalidated = tokio::task::spawn_blocking(move || {
            TrackCache::open(root).and_then(|cache| cache.invalidate(&metadata))
        })
        .await;
        match invalidated {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => tracing::warn!(%error, "audio cache invalidation failed"),
            Err(error) => tracing::warn!(%error, "audio cache worker failed"),
        }

        let action = match ncm.resolve_for_playback(&row).await {
            Ok(track) => Action::CacheFallbackResolved { generation, track },
            Err(error) if error.is::<api::TrackUnavailable>() => {
                Action::TrackUnavailable { generation }
            }
            Err(error) => Action::ResolveFailed {
                generation,
                message: error.to_string(),
            },
        };
        let _ = actions.send(action);
    });
}

fn spawn_fetch_lyrics(fx: &Effects, generation: u64, song_id: i64) {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let Ok(payload) = ncm.lyrics(song_id).await else {
            return; // missing lyrics are cosmetic
        };
        let word_lines = payload
            .yrc
            .as_deref()
            .map(crate::yrc::parse_yrc)
            .unwrap_or_default();
        let lines =
            crate::lyrics::parse_with_yrc(&payload.lrc, payload.tlyric.as_deref(), &word_lines);
        if !lines.is_empty() {
            let _ = actions.send(Action::LyricsLoaded { generation, lines });
        }
    });
}

fn spawn_prefetch(fx: &Effects, index: usize, row: SongRow) {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        if let Ok(track) = ncm.resolve_by_id(&row).await {
            let _ = actions.send(Action::PrefetchReady { index, track });
        }
    });
}

fn shuffled_order(len: usize, current: usize) -> Vec<usize> {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let mut order = (0..len)
        .filter(|index| *index != current)
        .collect::<Vec<_>>();
    for index in (1..order.len()).rev() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        order.swap(index, seed as usize % (index + 1));
    }
    order.insert(0, current);
    order
}

#[derive(Clone)]
struct CoverLoad {
    request: CoverRenderRequest,
    style: CoverStyle,
}

#[derive(Clone, Copy)]
struct CoverStyle {
    pixel: PixelStyle,
    original: bool,
}

#[derive(Clone, Copy)]
struct PixelStyle {
    palette_mode: pixel::CoverPalette,
    palette: &'static [(u8, u8, u8)],
    background: Color,
    detail_scale: f32,
    detail: pixel::CoverDetail,
}

fn spawn_cover_load(fx: &Effects, request: CoverRenderRequest, pic_url: String, style: CoverStyle) {
    let cache = fx.covers.clone();
    let actions = fx.actions.clone();
    let load = CoverLoad { request, style };
    tokio::spawn(async move {
        if send_cached_cover(cache.clone(), &actions, &load).await {
            return;
        }
        download_and_send_cover(cache, &actions, load, pic_url).await;
    });
}

fn spawn_selection_cover_lookup(
    fx: &Effects,
    generation: u64,
    load: Option<CoverLoad>,
    row: SongRow,
    neighbors: Vec<SongRow>,
    hot: bool,
) {
    let cache = fx.covers.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let due_at = tokio::time::Instant::now() + Duration::from_millis(150);
        let local_ready = if hot {
            true
        } else if let Some(load) = load.as_ref() {
            send_cached_cover(cache, &actions, load).await
        } else {
            false
        };
        tokio::time::sleep_until(due_at).await;
        let _ = actions.send(Action::SelectionCoverDue {
            generation,
            row,
            neighbors,
            needs_network: load.is_some() && !local_ready,
        });
    });
}

fn spawn_cover_download(fx: &Effects, load: CoverLoad, pic_url: String) {
    let cache = fx.covers.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        if send_cached_cover(cache.clone(), &actions, &load).await {
            return;
        }
        download_and_send_cover(cache, &actions, load, pic_url).await;
    });
}

async fn send_cached_cover(
    cache: Option<Arc<CoverCache>>,
    actions: &mpsc::UnboundedSender<Action>,
    load: &CoverLoad,
) -> bool {
    let Some(cache) = cache else {
        return false;
    };
    let loaded = if load.style.original {
        let Some(bytes) = read_original_cover(cache.clone(), load.request.source_key.clone()).await
        else {
            return false;
        };
        process_cover(Some(cache), load, bytes, false).await
    } else {
        load_cached_pixel(cache, load).await.map(EitherCover::Pixel)
    };
    let Some(loaded) = loaded else {
        return false;
    };
    send_cover(actions, &load.request, loaded);
    true
}

async fn load_cached_pixel(cache: Arc<CoverCache>, load: &CoverLoad) -> Option<PixelCover> {
    let pixel_key = pixel_cache_key(load);
    let read_cache = cache.clone();
    let read_key = pixel_key.clone();
    match tokio::task::spawn_blocking(move || read_cache.get_pixel(&read_key)).await {
        Ok(Ok(Some(cover))) => return Some(cover),
        Ok(Ok(None)) => {}
        Ok(Err(error)) => tracing::warn!(%error, "pixel cover cache read failed"),
        Err(error) => tracing::warn!(%error, "pixel cover cache worker failed"),
    }

    let bytes = read_original_cover(cache.clone(), load.request.source_key.clone()).await?;
    let processed = process_cover(Some(cache), load, bytes, false).await?;
    match processed {
        EitherCover::Pixel(cover) => Some(cover),
        EitherCover::Original(_) => None,
    }
}

async fn read_original_cover(cache: Arc<CoverCache>, source_key: String) -> Option<Vec<u8>> {
    match tokio::task::spawn_blocking(move || cache.get_original(&source_key)).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            tracing::warn!(%error, "original cover cache read failed");
            None
        }
        Err(error) => {
            tracing::warn!(%error, "original cover cache worker failed");
            None
        }
    }
}

async fn download_and_send_cover(
    cache: Option<Arc<CoverCache>>,
    actions: &mpsc::UnboundedSender<Action>,
    load: CoverLoad,
    pic_url: String,
) {
    let bytes = match api::fetch_cover(&pic_url, COVER_SOURCE_EDGE).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(%error, "cover fetch failed");
            return;
        }
    };
    let Some(processed) = process_cover(cache, &load, bytes, true).await else {
        return;
    };
    send_cover(actions, &load.request, processed);
}

async fn process_cover(
    cache: Option<Arc<CoverCache>>,
    load: &CoverLoad,
    bytes: Vec<u8>,
    downloaded: bool,
) -> Option<EitherCover> {
    let load = load.clone();
    let source_key = load.request.source_key.clone();
    let pixel_key = (!load.style.original).then(|| pixel_cache_key(&load));
    let processed = tokio::task::spawn_blocking(move || {
        if load.style.original {
            let image = image::load_from_memory(&bytes)?;
            write_original_cover(&cache, downloaded, &source_key, &bytes);
            return Ok::<_, anyhow::Error>(EitherCover::Original(image));
        }
        let cover = pixel::from_image_bytes(
            &bytes,
            load.style.pixel.palette_mode,
            load.style.pixel.palette,
            load.style.pixel.background,
            load.request.cells,
            load.style.pixel.detail_scale,
            load.style.pixel.detail,
        )?;
        write_original_cover(&cache, downloaded, &source_key, &bytes);
        if let (Some(cache), Some(pixel_key)) = (&cache, pixel_key) {
            if let Err(error) = cache.put_pixel(&pixel_key, &cover) {
                tracing::warn!(%error, "pixel cover cache write failed");
            }
        }
        Ok(EitherCover::Pixel(cover))
    })
    .await;
    match processed {
        Ok(Ok(cover)) => Some(cover),
        Ok(Err(error)) => {
            tracing::warn!(%error, "cover decode failed");
            None
        }
        Err(error) => {
            tracing::warn!(%error, "cover worker failed");
            None
        }
    }
}

fn send_cover(
    actions: &mpsc::UnboundedSender<Action>,
    request: &CoverRenderRequest,
    cover: EitherCover,
) {
    let action = match cover {
        EitherCover::Pixel(cover) => Action::CoverLoaded {
            request: request.clone(),
            cover,
        },
        EitherCover::Original(image) => Action::CoverDecoded {
            surface: request.surface,
            generation: request.generation,
            style_revision: request.style_revision,
            image,
        },
    };
    let _ = actions.send(action);
}

fn pixel_cache_key(load: &CoverLoad) -> String {
    CoverCache::pixel_key(PixelKeyInputs {
        song_id: load.request.song_id,
        original_key: &load.request.source_key,
        cells: load.request.cells,
        detail_scale: load.style.pixel.detail_scale,
        detail: load.style.pixel.detail,
        palette_mode: load.style.pixel.palette_mode,
        background: load.style.pixel.background,
        palette: load.style.pixel.palette,
    })
}

enum EitherCover {
    Pixel(PixelCover),
    Original(DynamicImage),
}

fn write_original_cover(
    cache: &Option<Arc<CoverCache>>,
    downloaded: bool,
    source_key: &str,
    bytes: &[u8],
) {
    if !downloaded {
        return;
    }
    if let Some(cache) = cache {
        if let Err(error) = cache.put_original(source_key, bytes) {
            tracing::warn!(%error, "original cover cache write failed");
        }
    }
}

fn spawn_cover_prefetch(
    fx: &Effects,
    generation: u64,
    style_revision: u64,
    rows: Vec<SongRow>,
    style: CoverStyle,
) {
    let Some(cache) = fx.covers.clone() else {
        return;
    };
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        for row in rows {
            let Some(pic_url) = row.pic_url.as_deref() else {
                continue;
            };
            let load = CoverLoad {
                request: CoverRenderRequest {
                    surface: CoverSurface::Selection,
                    generation,
                    cells: PREVIEW_CELLS,
                    style_revision,
                    song_id: row.id,
                    source_key: CoverCache::original_key(pic_url, COVER_SOURCE_EDGE),
                },
                style,
            };
            if style.original {
                if read_original_cover(cache.clone(), load.request.source_key.clone())
                    .await
                    .is_some()
                {
                    continue;
                }
                let Ok(bytes) = api::fetch_cover(pic_url, COVER_SOURCE_EDGE).await else {
                    continue;
                };
                let _ = process_cover(Some(cache.clone()), &load, bytes, true).await;
                continue;
            }
            let pixel_key = pixel_cache_key(&load);
            let cover = match load_cached_pixel(cache.clone(), &load).await {
                Some(cover) => cover,
                None => {
                    let Ok(bytes) = api::fetch_cover(pic_url, COVER_SOURCE_EDGE).await else {
                        continue;
                    };
                    let Some(EitherCover::Pixel(cover)) =
                        process_cover(Some(cache.clone()), &load, bytes, true).await
                    else {
                        continue;
                    };
                    cover
                }
            };
            let _ = actions.send(Action::SelectionCoverWarmed {
                generation,
                style_revision,
                pixel_key,
                cover,
            });
        }
    });
}

fn apply_pixel_cover(
    current: &mut Option<PixelCover>,
    generation: u64,
    desired_cells: (u16, u16),
    style_revision: u64,
    request: CoverRenderRequest,
    cover: PixelCover,
) {
    if request.generation == generation
        && request.cells == desired_cells
        && request.style_revision == style_revision
    {
        *current = Some(cover);
    }
}

/// The project's own logo (MIT, ships with the repo) — the default
/// dashboard art, pixelated through the same pipeline as covers.
const LOGO_BYTES: &[u8] = include_bytes!("../../../images/logo.png");

fn spawn_render_idle(
    fx: &Effects,
    bytes: Vec<u8>,
    cells: (u16, u16),
    style: PixelStyle,
    style_revision: u64,
) {
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let cover = tokio::task::spawn_blocking(move || {
            pixel::from_image_bytes(
                &bytes,
                style.palette_mode,
                style.palette,
                style.background,
                cells,
                style.detail_scale,
                style.detail,
            )
        })
        .await;
        if let Ok(Ok(cover)) = cover {
            let _ = actions.send(Action::IdleArtLoaded {
                cells,
                style_revision,
                cover,
            });
        }
    });
}

fn spawn_idle_load(fx: &Effects, path: std::path::PathBuf) {
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let bytes = tokio::task::spawn_blocking(move || std::fs::read(path)).await;
        if let Ok(Ok(bytes)) = bytes {
            let _ = actions.send(Action::IdleArtBytes { bytes });
        }
    });
}

/// Idle art scales with the terminal like covers do.
fn desired_idle_cells((cols, rows): (u16, u16)) -> (u16, u16) {
    let height = (rows * 2 / 5).clamp(12, 24);
    let width = (height * 2).min(cols.saturating_sub(4).max(16));
    (width, width / 2)
}

fn shellexpand_home(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(rest),
        None => std::path::PathBuf::from(path),
    }
}

pub async fn run(mut loaded: LoadedConfig) -> Result<()> {
    let mut terminal = ratatui::init();
    let detected_background = crate::terminal_background::probe();
    let terminal_is_light = detected_background.map(|background| {
        matches!(
            background.appearance(),
            crate::terminal_background::Appearance::Light
        )
    });
    if loaded.should_apply_terminal_brightness() {
        loaded.apply_terminal_brightness(terminal_is_light);
    }
    let config = loaded.config;
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste
    );
    let result = event_loop(
        &mut terminal,
        &config,
        detected_background.map(crate::terminal_background::Rgb::color),
        terminal_is_light,
    )
    .await;
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste
    );
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    config: &Config,
    terminal_background: Option<Color>,
    terminal_is_light: Option<bool>,
) -> Result<()> {
    let (player, mut player_events) = player::spawn(tokio::runtime::Handle::current());
    let (actions_tx, mut actions) = mpsc::unbounded_channel();
    let (playing_resize_tx, playing_resize_rx) = mpsc::unbounded_channel();
    let (playing_responses_tx, mut playing_responses) = mpsc::unbounded_channel();
    let (selected_resize_tx, selected_resize_rx) = mpsc::unbounded_channel();
    let (selected_responses_tx, mut selected_responses) = mpsc::unbounded_channel();
    let (selected_pending_resize_tx, selected_pending_resize_rx) = mpsc::unbounded_channel();
    let (selected_pending_responses_tx, mut selected_pending_responses) = mpsc::unbounded_channel();

    // Graphics protocol queries must finish before EventStream starts reading
    // the same terminal response bytes.
    let theme = Theme::by_name(crate::theme::resolved_name(
        &config.theme,
        config.theme_mode,
        terminal_is_light,
    ));
    let picker = query_graphics_picker(config.cover_mode, theme.bg);
    let original_cover = picker
        .clone()
        .map(|picker| OriginalCover::new(picker, playing_resize_tx));
    let selected_original_cover = picker.map(|picker| {
        OriginalCover::buffered(picker, selected_resize_tx, selected_pending_resize_tx)
    });
    spawn_resize_worker(playing_resize_rx, playing_responses_tx);
    spawn_resize_worker(selected_resize_rx, selected_responses_tx);
    spawn_resize_worker(selected_pending_resize_rx, selected_pending_responses_tx);

    let input_tx = actions_tx.clone();
    tokio::spawn(async move {
        let mut stream = crossterm::event::EventStream::new();
        while let Some(result) = stream.next().await {
            // A single unparseable sequence (e.g. an exotic drag-and-drop
            // payload) must not kill the input loop.
            let Ok(event) = result else { continue };
            if let Some(action) = event::action_for(event) {
                if input_tx.send(action).is_err() {
                    break;
                }
            }
        }
    });

    let player_tx = actions_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = player_events.recv().await {
            if player_tx.send(Action::Player(event)).is_err() {
                break;
            }
        }
    });

    let ncm = Arc::new(Ncm::new(
        config::session_path(),
        config.quality,
        config.unm_enabled,
    ));
    let fx = Effects {
        player,
        ncm,
        store: Arc::new(crate::store::LibraryStore::new(
            config::cache_dir().join("library"),
        )),
        actions: actions_tx,
        cache_root: initialize_audio_cache(config),
        covers: initialize_cover_cache(),
        config_path: config::config_dir().join("config.toml"),
    };
    let mut state = AppState::new(config);
    state.set_terminal_background(terminal_background);
    state.terminal_is_light = terminal_is_light;
    state.theme = theme;
    state.original_cover = original_cover;
    state.selected_original_cover = selected_original_cover;
    if let Some(playback) = fx.store.load_playback() {
        state.restore_playback(playback);
        fx.player.send(PlayerCommand::SetVolume(state.volume));
        if let Some(row) = state.active_row.clone() {
            state.ensure_placeholder();
            state.load_playing_cover(&fx, &row);
        }
    }
    state.load_idle_art(&fx);
    // Restore a persisted session: greet + load 我喜欢的音乐.
    if let Some(session) = fx.ncm.session_snapshot() {
        state.begin_session_restore(&fx, session);
    }
    state.reconcile_selected_cover(&fx);
    let mut hits = ui::Hits::default();
    let mut spectrum_ticks = tokio::time::interval(Duration::from_millis(50));
    spectrum_ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    terminal.draw(|frame| ui::draw(frame, &mut state, &mut hits))?;
    let mut ui_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_millis(120),
        Duration::from_millis(120),
    );
    ui_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            action = actions.recv() => {
                let Some(action) = action else { break };
                apply(&mut state, action, &fx, &hits);
                // Coalesce whatever queued up so one draw covers the burst.
                while let Ok(action) = actions.try_recv() {
                    apply(&mut state, action, &fx, &hits);
                }
            }
            response = playing_responses.recv(), if state.original_cover.is_some() => {
                let Some(response) = response else { break };
                state.apply_original_resize(response);
            }
            response = selected_responses.recv(), if state.selected_original_cover.is_some() => {
                let Some(response) = response else { break };
                state.apply_selected_original_resize(response);
            }
            response = selected_pending_responses.recv(), if state.selected_original_cover.is_some() => {
                let Some(response) = response else { break };
                state.apply_selected_pending_resize(response);
            }
            _ = ui_tick.tick(), if state.marquee_active() || state.command_feedback.is_some() => {
                state.update(Action::UiTick, &fx);
            }
            _ = spectrum_ticks.tick(), if state.config.spectrum_enabled || state.view == View::Settings => {
                state.spectrum.tick(
                    fx.player.samples(),
                    state.now.is_some() && !state.paused,
                    state.view == View::Settings,
                );
            }
        }
        if state.should_quit {
            break;
        }
        terminal.draw(|frame| ui::draw(frame, &mut state, &mut hits))?;
    }
    if let Err(error) = fx.store.save_playback(&state.playback_snapshot()) {
        tracing::warn!(%error, "playback state save failed");
    }
    Ok(())
}

/// Mouse events need the draw-time geometry, so they resolve here and
/// everything else goes straight to the reducer.
fn apply(state: &mut AppState, action: Action, fx: &Effects, hits: &ui::Hits) {
    match action {
        Action::Mouse(mouse) => {
            if state.command_palette.open || state.show_help {
                state.update(Action::Mouse(mouse), fx);
            } else {
                let selected = if state.view == View::Search && state.search.input
                    || state.view == View::Library && state.sidebar_focus
                    || state.filter.input
                {
                    usize::MAX
                } else {
                    state.selected
                };
                if let Some(resolved) = event::mouse_action(mouse, hits, selected) {
                    state.update(resolved, fx);
                }
            }
        }
        other => state.update(other, fx),
    }
    state.reconcile_marquee();
    state.reconcile_selected_cover(fx);
}

#[cfg(test)]
mod tests;
