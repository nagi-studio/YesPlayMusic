use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::io::Write;
use tempfile::TempDir;
use yesplaymusic_core::cache::{AudioCodec, AudioQuality, CacheKey};

use super::*;
use crate::action::{RestoreFailure, SessionStamp};

const PREVIEW_TERMINAL_SIZE: (u16, u16) = (
    ui::library::PREVIEW_MIN_TERMINAL_WIDTH,
    ui::cover_preview::HEIGHT + ui::HEADER_HEIGHT + ui::FOOTER_HEIGHT + ui::PANEL_GAP_Y * 2,
);

fn effects(directory: &TempDir) -> Effects {
    let (player, _events) = player::spawn(tokio::runtime::Handle::current());
    let (actions, _receiver) = mpsc::unbounded_channel();
    Effects {
        player,
        ncm: Arc::new(Ncm::new(
            directory.path().join("session.json"),
            yesplaymusic_core::cache::AudioQuality::High320,
            true,
        )),
        store: Arc::new(crate::store::LibraryStore::new(
            directory.path().join("library"),
        )),
        actions,
        cache_root: None,
        covers: None,
        config_path: directory.path().join("config.toml"),
    }
}

fn window_size(
    columns: u16,
    rows: u16,
    width: u16,
    height: u16,
) -> crossterm::terminal::WindowSize {
    crossterm::terminal::WindowSize {
        columns,
        rows,
        width,
        height,
    }
}

#[test]
fn graphics_geometry_tracks_pixel_scale_without_a_cell_grid_resize() {
    // ioctl may use a different absolute pixel convention than CSI 16 t;
    // only the 2x change is applied to the query's calibrated 10x20 size.
    let baseline = window_size(100, 40, 800, 640);
    let mut tracker =
        GraphicsGeometryTracker::from_window_size(FontSize::new(10, 20), &baseline).unwrap();

    assert!(tracker.observe(&baseline).is_none());
    let retina = tracker
        .observe(&window_size(100, 40, 1_600, 1_280))
        .expect("backing-scale change");
    assert_eq!((retina.width, retina.height), (20, 40));

    let external = tracker
        .observe(&baseline)
        .expect("return to the original display");
    assert_eq!((external.width, external.height), (10, 20));
}

#[test]
fn graphics_geometry_ignores_unavailable_pixel_dimensions() {
    assert!(GraphicsGeometryTracker::from_window_size(
        FontSize::new(10, 20),
        &window_size(100, 40, 0, 0),
    )
    .is_none());
}

#[test]
fn refreshing_graphics_geometry_rejects_old_resize_work() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(FontSize::new(10, 20));
    picker.set_protocol_type(ProtocolType::Halfblocks);
    let (requests, mut worker) = mpsc::unbounded_channel();
    let mut cover = OriginalCover::new(picker, requests);
    let image = DynamicImage::new_rgba8(1_000, 1_000);
    cover.replace(1, image.clone());
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut cover.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let stale = worker.try_recv().expect("old-geometry resize request");

    cover.refresh_font_size(FontSize::new(20, 20));
    cover.replace(1, image);
    let resized = cover
        .protocol
        .size_for(ratatui_image::Resize::Fit(None), PREVIEW_CELLS.into())
        .expect("replacement protocol");
    assert_eq!((resized.width, resized.height), (11, 11));
    assert!(!cover
        .protocol
        .update_resized_protocol(stale.resize_encode().unwrap()));
}

fn raw_key(code: KeyCode) -> Action {
    Action::RawKey(KeyEvent::new(code, KeyModifiers::NONE))
}

fn row(id: i64) -> SongRow {
    SongRow {
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

fn named_row(id: i64, title: &str, artist: &str) -> SongRow {
    SongRow {
        id,
        title: title.into(),
        artist: artist.into(),
        album: "Album".into(),
        duration_ms: 180_000,
        pic_url: None,
        artist_id: None,
        album_id: None,
    }
}

fn covered_row(id: i64) -> SongRow {
    SongRow {
        pic_url: Some(format!("https://example.test/{id}.jpg")),
        ..row(id)
    }
}

fn solid_preview(color: ratatui::style::Color) -> PixelCover {
    PixelCover {
        width: PREVIEW_CELLS.0,
        height: PREVIEW_CELLS.1,
        cells: vec![
            crate::pixel::PixelCell {
                glyph: '█',
                fg: color,
                bg: color,
            };
            usize::from(PREVIEW_CELLS.0) * usize::from(PREVIEW_CELLS.1)
        ],
    }
}

#[cfg(unix)]
#[test]
fn remote_snapshot_projects_the_current_cover_before_it_renders() {
    let mut state = AppState::new(&Config::default());
    let mut current = row(7);
    current.pic_url = Some("http://p3.music.126.net/7.jpg?token=secret".into());
    state.active_row = Some(current);
    state.now = Some(NowPlaying {
        title: "Track 7".into(),
        artist: "Artist".into(),
        album: "Album".into(),
        artist_id: None,
        album_id: None,
    });

    let snapshot = state.remote_snapshot();
    assert_eq!(
        snapshot.cover_url.as_deref(),
        Some("https://p3.music.126.net/7.jpg?param=64y64")
    );
    state.duration = Some(Duration::from_secs(180));
    assert!(
        !state.remote_snapshot().seekable,
        "resolved duration alone must not advertise a seek the player would drop"
    );
    state.player_ready_generation = Some(state.generation);
    assert!(state.remote_snapshot().seekable);
    assert!(state.bar_cover.is_none());

    // A parked row is not current metadata and must not leak stale artwork.
    state.now = None;
    assert_eq!(state.remote_snapshot().cover_url, None);
}

#[tokio::test]
async fn ratio_seek_is_ignored_until_the_current_player_generation_starts() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.generation = 3;
    state.duration = Some(Duration::from_secs(200));
    state.position = Duration::from_secs(10);

    state.update(Action::SeekToRatio(0.5), &fx);
    assert_eq!(state.position, Duration::from_secs(10));

    state.player_ready_generation = Some(3);
    state.update(Action::SeekToRatio(0.5), &fx);
    assert_eq!(state.position, Duration::from_secs(100));
}

#[tokio::test]
async fn paused_ui_ticks_advance_the_marquee_without_consuming_the_gg_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Queue;
    state.paused = true;
    state.terminal_size = (80, 24);
    state.queue = vec![named_row(
        1,
        "A deliberately long queue title that exceeds every available title column",
        "Artist",
    )];

    state.update(Action::GKey, &fx);
    state.update(Action::UiTick, &fx);
    assert_eq!(state.marquee_frame, 0);
    assert!(state.pending_g);

    state.update(Action::UiTick, &fx);
    assert_eq!(state.marquee_frame, 1);
    assert!(state.pending_g);
}

#[tokio::test]
async fn a_clicked_link_opens_its_own_page_even_after_the_track_moved_on() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.zen = true;
    // The click was drawn against 周杰伦, but the queue advanced before the
    // action reduced. The link carries its own target, so the page that
    // opens is still the one the user pointed at.
    state.now = Some(NowPlaying {
        title: "Next".into(),
        artist: "Someone Else".into(),
        album: String::new(),
        artist_id: Some(999),
        album_id: None,
    });
    state.update(
        Action::OpenPage(crate::ui::Hits::link(
            crate::api::SearchChannel::Artists,
            6452,
            "周杰伦".into(),
        )),
        &fx,
    );

    assert_eq!(state.view, View::Search);
    // Zen draws the player over every view, so the page would be invisible.
    assert!(!state.zen);
    let detail = state.search.detail.as_ref().expect("artist page opened");
    assert_eq!(detail.id, 6452);
    assert_eq!(detail.channel, crate::api::SearchChannel::Artists);
    assert_eq!(detail.title, "周杰伦");
    // Back resolves its index against the detail's own bucket, so the tab has
    // to follow the page or an artist index lands in the song list.
    assert_eq!(state.search.channel, crate::api::SearchChannel::Artists);
}

#[test]
fn a_centred_meta_line_puts_its_link_under_the_visible_text() {
    use crate::ui::now_playing::meta_link_rect;
    let area = Rect::new(0, 0, 40, 6);
    let left = meta_link_rect(area, 1, false, 12, 2, 10).expect("left rect");
    assert_eq!((left.x, left.y, left.width), (2, 1, 10));
    // Centred: Ratatui offsets by the whole line width, FM badge included.
    let centred = meta_link_rect(area, 1, true, 20, 0, 10).expect("centred rect");
    assert_eq!(centred.x, 10);
    // Off the bottom, or nothing to click, or pushed past the right edge:
    // none of these may leave a clickable region behind.
    assert!(meta_link_rect(area, 9, false, 12, 2, 10).is_none());
    assert!(meta_link_rect(area, 1, false, 2, 2, 0).is_none());
    assert!(meta_link_rect(Rect::new(0, 0, 3, 6), 1, false, 5, 3, 2).is_none());
}

#[test]
fn the_source_resolution_follows_the_cover_box_in_real_pixels() {
    let mut state = AppState::new(&Config::default());
    // Half-block art is generated locally, so the shipped floor is enough.
    assert_eq!(state.cover_source_edge(), COVER_SOURCE_EDGE);

    state.config.cover_mode = CoverMode::Original;
    state.terminal_size = (124, 32);
    state.original_cover = Some(OriginalCover::new(
        #[allow(deprecated)]
        Picker::from_fontsize(FontSize::new(24, 53)),
        tokio::sync::mpsc::unbounded_channel().0,
    ));
    let (cols, rows) = state.desired_cover_cells();
    let needed = (u32::from(cols) * 24).max(u32::from(rows) * 53);
    let edge = state.cover_source_edge();
    // Terminal graphics never upscale: anything short of the box leaves the
    // artwork floating inside its own column.
    assert!(edge >= needed, "{edge} < {needed}");
    // Bucketed, so dragging the window does not re-fetch every cover.
    assert!(COVER_SOURCE_EDGES.contains(&edge));
}

#[test]
fn cover_size_presets_keep_auto_stable_and_bound_the_extremes() {
    assert_eq!(cover_height_for_preset(20, CoverSize::Compact), 14);
    assert_eq!(cover_height_for_preset(20, CoverSize::Auto), 20);
    assert_eq!(cover_height_for_preset(20, CoverSize::Large), 20);

    assert_eq!(cover_height_for_preset(80, CoverSize::Compact), 28);
    assert_eq!(cover_height_for_preset(80, CoverSize::Auto), 40);
    assert_eq!(cover_height_for_preset(80, CoverSize::Large), 56);
}

#[test]
fn the_cover_box_is_square_in_pixels_not_in_assumed_cells() {
    let mut state = AppState::new(&Config::default());
    // No graphics picker: half-block art packs two sub-pixels per cell, so a
    // square is width/2 rows regardless of the font.
    assert_eq!(state.square_cover_rows(54), 27);
    assert_eq!(state.square_cover_cols(27), 54);

    // A real terminal reported 24x53 pixel cells, which is a tenth taller than
    // the 1:2 the block characters imply. Assuming 1:2 there gave the cover a
    // box 27 rows tall for artwork that could only fill 24, and the image sat
    // at the top of it with dead space underneath.
    state.config.cover_mode = CoverMode::Original;
    state.original_cover = Some(OriginalCover::new(
        // The only constructor that takes a known font size; the query-based
        // ones need a live terminal.
        #[allow(deprecated)]
        Picker::from_fontsize(FontSize::new(24, 53)),
        tokio::sync::mpsc::unbounded_channel().0,
    ));
    assert_eq!(state.square_cover_rows(54), 24);
    assert_eq!(state.square_cover_cols(24), 53);
    // Never zero: a degenerate grid would divide by zero downstream.
    assert_eq!(state.square_cover_rows(1), 1);
}

#[test]
fn the_preview_prediction_accounts_for_the_bottom_player_bar() {
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.terminal_size = (110, 22);
    // Nothing playing: no bar, so the body clears the preview's height.
    assert!(state.selection_preview_visible());

    state.now = Some(NowPlaying {
        title: "T".into(),
        artist: "A".into(),
        album: "B".into(),
        artist_id: None,
        album_id: None,
    });
    // The bar now eats the rows the preview needed. Predicting otherwise arms
    // the marquee for a panel that is not on screen, and it then appears
    // already scrolled.
    assert_eq!(
        state.selection_preview_visible(),
        crate::ui::body_rows(&state, 22) >= crate::ui::cover_preview::HEIGHT
    );
    assert!(
        crate::ui::body_rows(&state, 22)
            < crate::ui::body_rows(&AppState::new(&Config::default()), 22)
    );
}

#[test]
fn a_playing_track_keeps_the_ui_tick_armed_on_its_own() {
    let mut state = AppState::new(&Config::default());
    // Idle: nothing moves by itself, so the loop is free to sleep.
    assert!(!state.needs_ui_tick());

    state.now = Some(NowPlaying {
        title: "Short".into(),
        artist: "Artist".into(),
        album: String::new(),
        artist_id: None,
        album_id: None,
    });
    // A title too short to scroll used to leave the loop with no periodic
    // wake-up at all: the progress bar and lyrics then only advanced while
    // unrelated events (mouse motion over the terminal) happened to arrive.
    assert!(!state.marquee_active());
    assert!(state.needs_ui_tick());

    state.paused = true;
    assert!(!state.needs_ui_tick());
}

#[tokio::test]
async fn paused_mini_player_ticks_advance_its_title_marquee() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let hits = ui::Hits::default();
    let mut state = AppState::new(&Config::default());
    state.paused = true;
    state.now = Some(NowPlaying {
        title: "A deliberately long title that must scroll in the mini player".into(),
        artist: "Artist".into(),
        album: String::new(),
        artist_id: None,
        album_id: None,
    });

    apply(&mut state, Action::Resize { cols: 80, rows: 6 }, &fx, &hits);
    assert!(state.marquee_active());
    assert_eq!(state.marquee_frame(), 0);

    apply(&mut state, Action::UiTick, &fx, &hits);
    assert_eq!(state.marquee_frame(), 1);
}

#[tokio::test]
async fn mini_player_keys_override_settings_and_text_input_focus() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.terminal_size = (80, 6);
    state.view = View::Settings;
    state.active_row = Some(row(1));
    state.resume_on_play = Some(Duration::from_secs(12));
    state.show_help = true;

    state.update(raw_key(KeyCode::Char(' ')), &fx);
    assert_eq!(state.generation, 1);
    assert!(!state.show_help);

    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(0);
    state.update(raw_key(KeyCode::Char('n')), &fx);
    assert_eq!(state.queue_pos, Some(1));

    state.update(raw_key(KeyCode::Char('q')), &fx);
    assert!(state.confirm_quit);
    state.update(raw_key(KeyCode::Esc), &fx);

    state.view = View::Search;
    state.search.input = true;
    state.search.query = "needle".into();
    state.update(raw_key(KeyCode::Char('p')), &fx);
    assert_eq!(state.queue_pos, Some(0));
    assert_eq!(state.search.query, "needle");

    state.filter.start();
    state.update(raw_key(KeyCode::Char('*')), &fx);
    assert!(state.filter.query.is_empty());
}

#[tokio::test]
async fn returning_to_a_long_row_restarts_the_marquee_pause() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let hits = ui::Hits::default();
    let mut state = AppState::new(&Config::default());
    state.view = View::Queue;
    state.terminal_size = (80, 24);
    state.queue = vec![
        named_row(
            1,
            "A deliberately long queue title that exceeds every available title column",
            "Artist",
        ),
        named_row(2, "Short", "Artist"),
    ];

    apply(&mut state, Action::UiTick, &fx, &hits);
    apply(&mut state, Action::UiTick, &fx, &hits);
    assert_eq!(state.marquee_frame, 1);

    apply(&mut state, Action::MoveSelection(1), &fx, &hits);
    assert!(state.marquee_target.is_none());
    apply(&mut state, Action::MoveSelection(-1), &fx, &hits);
    assert_eq!(state.marquee_frame, 0);
    assert_eq!(
        state.marquee_target.as_ref().map(|target| target.id),
        Some(1)
    );
}

#[test]
fn marquee_targets_use_the_drawn_list_and_cover_widths() {
    let mut state = AppState::new(&Config::default());
    state.view = View::Queue;
    state.terminal_size = (80, 24);
    state.queue = vec![named_row(1, "123456789012345678901234567890", "Artist")];
    assert!(!state.marquee_active());

    state.view = View::Library;
    state.terminal_size = (120, 40);
    state.library = vec![named_row(2, "1234567890123456789012", "Artist")];
    assert!(state.selection_preview_visible());
    assert!(!state.marquee_active());

    state.library[0].title.push('3');
    assert!(state.marquee_active());
}

#[test]
fn selected_cover_scheduling_uses_the_framed_preview_boundaries() {
    let mut state = AppState::new(&Config::default());
    let minimum_rows = PREVIEW_TERMINAL_SIZE.1;

    state.view = View::Library;
    state.terminal_size = (ui::library::PREVIEW_MIN_TERMINAL_WIDTH - 1, minimum_rows);
    assert!(!state.selection_preview_visible());
    state.terminal_size = PREVIEW_TERMINAL_SIZE;
    assert!(state.selection_preview_visible());

    state.view = View::Search;
    state.terminal_size = (ui::search::PREVIEW_MIN_TERMINAL_WIDTH, minimum_rows - 1);
    assert!(!state.selection_preview_visible());
    state.terminal_size = (ui::search::PREVIEW_MIN_TERMINAL_WIDTH, minimum_rows);
    assert!(state.selection_preview_visible());
}

#[tokio::test]
async fn a_network_cover_waits_for_debounce_and_schedules_three_neighbors_each_side() {
    let directory = tempfile::tempdir().unwrap();
    let (player, _events) = player::spawn(tokio::runtime::Handle::current());
    let (actions, mut receiver) = mpsc::unbounded_channel();
    let fx = Effects {
        player,
        ncm: Arc::new(Ncm::new(
            directory.path().join("session.json"),
            AudioQuality::High320,
            true,
        )),
        store: Arc::new(crate::store::LibraryStore::new(
            directory.path().join("library"),
        )),
        actions,
        cache_root: None,
        covers: None,
        config_path: directory.path().join("config.toml"),
    };
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.terminal_size = PREVIEW_TERMINAL_SIZE;
    state.library = (0..9).map(covered_row).collect();
    state.selected = 4;

    state.reconcile_selected_cover(&fx);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), receiver.recv())
            .await
            .is_err()
    );

    let due = tokio::time::timeout(Duration::from_millis(250), receiver.recv())
        .await
        .expect("debounced cover request")
        .expect("cover action channel");
    let Action::SelectionCoverDue {
        generation,
        row,
        neighbors,
        needs_network,
    } = due
    else {
        panic!("unexpected action");
    };
    assert_eq!(generation, state.selected_cover.generation);
    assert_eq!(row.id, 4);
    assert!(needs_network);
    assert_eq!(
        neighbors.iter().map(|row| row.id).collect::<Vec<_>>(),
        [1, 2, 3, 5, 6, 7]
    );
}

#[tokio::test]
async fn a_hot_selected_cover_replaces_the_placeholder_synchronously() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let selected = covered_row(7);
    let pic_url = selected.pic_url.as_deref().unwrap();
    state.view = View::Library;
    state.terminal_size = PREVIEW_TERMINAL_SIZE;
    state.library = vec![selected.clone()];

    let request = state.cover_request(
        CoverSurface::Selection,
        1,
        selected.id,
        pic_url,
        PREVIEW_CELLS,
    );
    let pixel_key = CoverCache::pixel_key(PixelKeyInputs {
        song_id: request.song_id,
        original_key: &request.source_key,
        cells: request.cells,
        detail_scale: state.pixel_detail_scale,
        detail: state.config.cover_detail,
        palette_mode: state.config.cover_palette,
        background: state.theme.bg,
        palette: state.theme.palette,
    });
    let hot = solid_preview(ratatui::style::Color::Red);
    assert_ne!(&hot, state.preview_placeholder());
    state.hot_pixel_covers.insert(pixel_key, hot.clone());

    state.reconcile_selected_cover(&fx);

    assert_eq!(state.selected_pixel_cover(), Some(&hot));
}

#[tokio::test]
async fn an_l2_hit_is_loaded_before_the_network_debounce() {
    let directory = tempfile::tempdir().unwrap();
    let covers =
        Arc::new(CoverCache::new(directory.path().join("covers"), 256 * 1024 * 1024).unwrap());
    let (player, _events) = player::spawn(tokio::runtime::Handle::current());
    let (actions, mut receiver) = mpsc::unbounded_channel();
    let fx = Effects {
        player,
        ncm: Arc::new(Ncm::new(
            directory.path().join("session.json"),
            AudioQuality::High320,
            true,
        )),
        store: Arc::new(crate::store::LibraryStore::new(
            directory.path().join("library"),
        )),
        actions,
        cache_root: None,
        covers: Some(covers.clone()),
        config_path: directory.path().join("config.toml"),
    };
    let mut state = AppState::new(&Config::default());
    let selected = covered_row(8);
    let pic_url = selected.pic_url.as_deref().unwrap();
    state.view = View::Library;
    state.terminal_size = PREVIEW_TERMINAL_SIZE;
    state.library = vec![selected.clone()];
    let request = state.cover_request(
        CoverSurface::Selection,
        1,
        selected.id,
        pic_url,
        PREVIEW_CELLS,
    );
    let pixel_key = state.pixel_cover_key(&request);
    let cached = solid_preview(ratatui::style::Color::Rgb(10, 20, 200));
    covers.put_pixel(&pixel_key, &cached).unwrap();

    state.reconcile_selected_cover(&fx);

    let action = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .expect("L2 lookup must not wait for the network debounce")
        .expect("cover action channel");
    let Action::CoverLoaded { request, cover } = action else {
        panic!("unexpected action");
    };
    assert_eq!(request.generation, state.selected_cover.generation);
    assert_eq!(cover, cached);
}

#[tokio::test]
async fn a_cold_selection_holds_the_last_pixel_cover() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.terminal_size = PREVIEW_TERMINAL_SIZE;
    state.library = vec![covered_row(1), covered_row(2)];
    let (first_key, _, _) = state.selected_cover_candidate().unwrap();
    let previous = solid_preview(ratatui::style::Color::Green);
    state.selected_cover.key = Some(first_key);
    state.selected_cover.pixel = Some(previous.clone());
    state.selected_cover.pixel_key = Some("previous".into());

    state.selected = 1;
    state.reconcile_selected_cover(&fx);

    assert_eq!(state.selected_cover.key.as_ref().map(|key| key.id), Some(2));
    assert_eq!(state.selected_pixel_cover(), Some(&previous));
    assert_eq!(state.selected_cover.pixel_key.as_deref(), Some("previous"));
}

#[test]
fn hot_pixel_covers_evict_the_least_recently_used_entry() {
    let cover = solid_preview(ratatui::style::Color::Cyan);
    let mut hot = HotPixelCovers::default();
    for index in 0..=HOT_PIXEL_COVER_LIMIT {
        hot.insert(index.to_string(), cover.clone());
    }
    assert!(hot.get("0").is_none());
    assert!(hot.get("1").is_some());

    hot.insert("next".into(), cover);

    assert!(hot.get("2").is_none());
    assert!(hot.get("1").is_some());
}

#[test]
fn original_preview_keeps_the_previous_protocol_until_the_replacement_is_ready() {
    let (main_tx, mut main_rx) = mpsc::unbounded_channel();
    let (pending_tx, mut pending_rx) = mpsc::unbounded_channel();
    let mut cover = OriginalCover::buffered(Picker::halfblocks(), main_tx, pending_tx);
    let image = DynamicImage::new_rgb8(100, 100);
    cover.replace(1, image.clone());
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut cover.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let first = main_rx.try_recv().expect("initial resize request");
    cover
        .protocol
        .update_resized_protocol(first.resize_encode().unwrap());

    cover.replace(2, image);
    let pending = cover.pending.as_mut().expect("pending replacement");
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut pending.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );

    assert_eq!(cover.generation, Some(1));
    assert!(cover.protocol.protocol_type().is_some());
    assert!(pending.protocol.protocol_type().is_none());

    let replacement = pending_rx.try_recv().expect("replacement resize request");
    cover.update_pending(replacement.resize_encode().unwrap(), 2);
    assert_eq!(cover.generation, Some(2));
    // New handover semantics: the encoded stand-in stays until the main
    // protocol finishes its re-encode.
    assert!(cover
        .pending
        .as_ref()
        .is_some_and(|pending| pending.promoted));
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut cover.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let handover = main_rx.try_recv().expect("handover re-encode request");
    assert!(cover
        .protocol
        .update_resized_protocol(handover.resize_encode().unwrap()));
    cover.finish_handover();
    assert!(cover.pending.is_none());
}

#[test]
fn stale_original_replacement_cannot_displace_the_held_frame() {
    let (main_tx, mut main_rx) = mpsc::unbounded_channel();
    let (pending_tx, mut pending_rx) = mpsc::unbounded_channel();
    let mut cover = OriginalCover::buffered(Picker::halfblocks(), main_tx, pending_tx);
    let image = DynamicImage::new_rgb8(100, 100);
    cover.replace(1, image.clone());
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut cover.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let first = main_rx.try_recv().expect("initial resize request");
    cover
        .protocol
        .update_resized_protocol(first.resize_encode().unwrap());
    cover.replace(2, image.clone());
    let pending = cover.pending.as_mut().expect("pending replacement");
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut pending.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let stale = pending_rx.try_recv().expect("stale resize request");

    cover.cancel_pending();
    cover.replace(3, image);
    let pending = cover.pending.as_mut().expect("current replacement");
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut pending.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let current = pending_rx.try_recv().expect("current resize request");
    cover.update_pending(stale.resize_encode().unwrap(), 3);

    assert_eq!(cover.generation, Some(1));
    assert!(cover.pending.is_some());
    assert!(cover.protocol.protocol_type().is_some());

    cover.update_pending(current.resize_encode().unwrap(), 3);
    assert_eq!(cover.generation, Some(3));
    // New handover semantics: the encoded stand-in stays until the main
    // protocol finishes its re-encode.
    assert!(cover
        .pending
        .as_ref()
        .is_some_and(|pending| pending.promoted));
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut cover.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let handover = main_rx.try_recv().expect("handover re-encode request");
    assert!(cover
        .protocol
        .update_resized_protocol(handover.resize_encode().unwrap()));
    cover.finish_handover();
    assert!(cover.pending.is_none());
}

#[test]
fn hidden_preview_rejects_a_stale_original_resize_after_it_returns() {
    let (main_tx, mut main_rx) = mpsc::unbounded_channel();
    let (pending_tx, mut pending_rx) = mpsc::unbounded_channel();
    let mut cover = OriginalCover::buffered(Picker::halfblocks(), main_tx, pending_tx);
    let image = DynamicImage::new_rgb8(100, 100);
    cover.replace(1, image.clone());
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut cover.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let first = main_rx.try_recv().expect("initial resize request");
    cover
        .protocol
        .update_resized_protocol(first.resize_encode().unwrap());
    cover.replace(2, image.clone());
    let pending = cover.pending.as_mut().expect("stale replacement");
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut pending.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let stale = pending_rx.try_recv().expect("stale resize request");

    cover.clear();
    cover.replace(3, image.clone());
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut cover.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let current_main = main_rx.try_recv().expect("current primary resize request");
    cover
        .protocol
        .update_resized_protocol(current_main.resize_encode().unwrap());
    cover.replace(4, image);
    let pending = cover.pending.as_mut().expect("current replacement");
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut pending.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let current = pending_rx
        .try_recv()
        .expect("current pending resize request");

    cover.update_pending(stale.resize_encode().unwrap(), 4);
    assert_eq!(cover.generation, Some(3));
    assert!(cover.pending.is_some());

    cover.update_pending(current.resize_encode().unwrap(), 4);
    assert_eq!(cover.generation, Some(4));
    // New handover semantics: the encoded stand-in stays until the main
    // protocol finishes its re-encode.
    assert!(cover
        .pending
        .as_ref()
        .is_some_and(|pending| pending.promoted));
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut cover.protocol,
        &ratatui_image::Resize::Fit(None),
        PREVIEW_CELLS.into(),
    );
    let handover = main_rx.try_recv().expect("handover re-encode request");
    assert!(cover
        .protocol
        .update_resized_protocol(handover.resize_encode().unwrap()));
    cover.finish_handover();
    assert!(cover.pending.is_none());
}

#[tokio::test]
async fn a_prefetched_pixel_warms_the_next_selection() {
    let directory = tempfile::tempdir().unwrap();
    let covers =
        Arc::new(CoverCache::new(directory.path().join("covers"), 256 * 1024 * 1024).unwrap());
    let (player, _events) = player::spawn(tokio::runtime::Handle::current());
    let (actions, mut receiver) = mpsc::unbounded_channel();
    let fx = Effects {
        player,
        ncm: Arc::new(Ncm::new(
            directory.path().join("session.json"),
            AudioQuality::High320,
            true,
        )),
        store: Arc::new(crate::store::LibraryStore::new(
            directory.path().join("library"),
        )),
        actions,
        cache_root: None,
        covers: Some(covers.clone()),
        config_path: directory.path().join("config.toml"),
    };
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.terminal_size = PREVIEW_TERMINAL_SIZE;
    state.library = vec![row(1), covered_row(2)];
    let next = state.library[1].clone();
    let request = state.cover_request(
        CoverSurface::Selection,
        1,
        next.id,
        next.pic_url.as_deref().unwrap(),
        PREVIEW_CELLS,
    );
    let pixel_key = state.pixel_cover_key(&request);
    let disk_prefetched = solid_preview(ratatui::style::Color::Rgb(180, 20, 180));
    covers.put_pixel(&pixel_key, &disk_prefetched).unwrap();

    state.reconcile_selected_cover(&fx);
    let due = tokio::time::timeout(Duration::from_millis(250), receiver.recv())
        .await
        .expect("neighbor prefetch debounce")
        .expect("cover action channel");
    state.update(due, &fx);
    let warmed = tokio::time::timeout(Duration::from_millis(250), receiver.recv())
        .await
        .expect("prefetched L2 result")
        .expect("cover action channel");
    let Action::SelectionCoverWarmed { cover, .. } = &warmed else {
        panic!("unexpected action");
    };
    assert_eq!(cover, &disk_prefetched);
    state.update(warmed, &fx);

    state.selected = 1;
    state.reconcile_selected_cover(&fx);

    assert_eq!(state.selected_pixel_cover(), Some(&disk_prefetched));
}

#[tokio::test]
async fn a_selection_without_art_still_prefetches_neighbor_originals() {
    let directory = tempfile::tempdir().unwrap();
    let (player, _events) = player::spawn(tokio::runtime::Handle::current());
    let (actions, mut receiver) = mpsc::unbounded_channel();
    let fx = Effects {
        player,
        ncm: Arc::new(Ncm::new(
            directory.path().join("session.json"),
            AudioQuality::High320,
            true,
        )),
        store: Arc::new(crate::store::LibraryStore::new(
            directory.path().join("library"),
        )),
        actions,
        cache_root: None,
        covers: None,
        config_path: directory.path().join("config.toml"),
    };
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.terminal_size = PREVIEW_TERMINAL_SIZE;
    state.library = vec![covered_row(1), row(2), covered_row(3)];
    state.selected = 1;
    let previous = solid_preview(ratatui::style::Color::Green);
    state.selected_cover.pixel = Some(previous.clone());
    state.selected_cover.pixel_key = Some("previous".into());

    state.reconcile_selected_cover(&fx);
    assert_eq!(state.selected_pixel_cover(), Some(&previous));
    let due = tokio::time::timeout(Duration::from_millis(250), receiver.recv())
        .await
        .expect("neighbor prefetch request")
        .expect("cover action channel");
    let Action::SelectionCoverDue {
        row,
        neighbors,
        needs_network,
        ..
    } = due
    else {
        panic!("unexpected action");
    };
    assert_eq!(row.id, 2);
    assert!(!needs_network);
    assert_eq!(
        neighbors.iter().map(|row| row.id).collect::<Vec<_>>(),
        [1, 3]
    );
}

#[tokio::test]
async fn mute_restores_the_volume_that_was_active_before_muting() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.volume = 0.65;

    state.update(Action::ToggleMute, &fx);
    assert_eq!(state.volume, 0.0);
    assert_eq!(state.volume_before_mute, Some(0.65));

    state.update(Action::ToggleMute, &fx);
    assert_eq!(state.volume, 0.65);
    assert_eq!(state.volume_before_mute, None);
}

#[tokio::test]
async fn shuffle_and_repeat_change_independently() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());

    state.update(Action::ToggleShuffle, &fx);
    assert!(state.shuffle);
    assert_eq!(state.play_mode, PlayMode::Off);

    state.update(Action::CycleRepeat, &fx);
    assert!(state.shuffle);
    assert_eq!(state.play_mode, PlayMode::List);
    state.update(Action::CycleRepeat, &fx);
    assert_eq!(state.play_mode, PlayMode::One);
    state.update(Action::CycleRepeat, &fx);
    assert_eq!(state.play_mode, PlayMode::Off);
}

#[tokio::test]
async fn clickable_mode_slot_cycles_the_four_visible_states() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());

    for expected in [
        (false, PlayMode::List),
        (false, PlayMode::One),
        (true, PlayMode::Off),
        (false, PlayMode::Off),
    ] {
        state.update(Action::CyclePlaybackMode, &fx);
        assert_eq!((state.shuffle, state.play_mode), expected);
    }

    for repeat in [PlayMode::List, PlayMode::One] {
        state.shuffle = true;
        state.play_mode = repeat;
        state.update(Action::CyclePlaybackMode, &fx);
        assert_eq!((state.shuffle, state.play_mode), (false, PlayMode::Off));
    }
}

#[tokio::test]
async fn list_repeat_wraps_while_single_repeat_replays_only_on_track_end() {
    let directory = tempfile::tempdir().unwrap();
    let mut fx = effects(&directory);
    fx.cache_root = Some(directory.path().join("audio"));
    let mut state = AppState::new(&Config::default());
    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(1);
    state.play_mode = PlayMode::List;

    state.update(Action::NextTrack, &fx);
    assert_eq!(state.queue_pos, Some(0));

    state.queue_pos = Some(1);
    state.play_mode = PlayMode::One;
    let generation = state.generation;
    state.update(Action::Player(PlayerEvent::Ended { generation }), &fx);
    assert_eq!(state.queue_pos, Some(1));
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Track 2")
    );
}

#[tokio::test]
async fn local_filter_maps_visible_rows_to_the_correct_song_and_escape_restores_the_list() {
    let directory = tempfile::tempdir().unwrap();
    let mut fx = effects(&directory);
    fx.cache_root = Some(directory.path().join("audio"));
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.library = vec![
        named_row(1, "Alpha", "One"),
        named_row(2, "Beta", "Two"),
        named_row(3, "Gamma", "Three"),
    ];

    state.update(Action::StartFilter, &fx);
    state.update(raw_key(KeyCode::Char('g')), &fx);
    state.update(raw_key(KeyCode::Char('m')), &fx);
    assert_eq!(state.visible_len(), 1);
    state.update(raw_key(KeyCode::Enter), &fx);
    state.update(Action::Activate, &fx);

    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Gamma")
    );
    assert_eq!(
        state.queue.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![3]
    );

    state.view = View::Library;
    state.update(Action::StartFilter, &fx);
    state.update(raw_key(KeyCode::Char('z')), &fx);
    assert_eq!(state.visible_len(), 0);
    state.update(raw_key(KeyCode::Esc), &fx);
    assert_eq!(state.visible_len(), 3);
    assert!(state.filter.query.is_empty());
    assert_eq!(state.view, View::Library);
}

#[tokio::test]
async fn starting_a_library_filter_moves_focus_from_the_sidebar_to_results() {
    let directory = tempfile::tempdir().unwrap();
    let mut fx = effects(&directory);
    fx.cache_root = Some(directory.path().join("audio"));
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.sidebar_focus = true;
    state.library = vec![named_row(1, "Alpha", "One"), named_row(2, "Beta", "Two")];

    state.update(Action::StartFilter, &fx);
    state.update(raw_key(KeyCode::Char('b')), &fx);
    state.update(raw_key(KeyCode::Enter), &fx);

    assert!(!state.sidebar_focus);
    assert_eq!(state.visible_len(), 1);
    state.update(raw_key(KeyCode::Enter), &fx);
    assert_eq!(state.current_track_id, Some(2));
}

#[tokio::test]
async fn adding_a_filtered_selection_appends_without_starting_playback() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.library = vec![named_row(1, "Alpha", "One"), named_row(2, "Beta", "Two")];
    state.filter.query = "bt".into();

    state.update(Action::AddSelectedToQueue, &fx);

    assert_eq!(
        state.queue.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(state.generation, 0);
    assert!(state.now.is_none());
    assert_eq!(state.view, View::Library);
}

#[tokio::test]
async fn filtered_mouse_rows_keep_their_visible_to_underlying_mapping() {
    let directory = tempfile::tempdir().unwrap();
    let mut fx = effects(&directory);
    fx.cache_root = Some(directory.path().join("audio"));
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.library = vec![
        named_row(1, "Alpha", "One"),
        named_row(2, "Gamma", "Two"),
        named_row(3, "Gamut", "Three"),
    ];
    state.filter.query = "ga".into();
    let mut hits = ui::Hits::default();
    hits.rows.push((Rect::new(4, 5, 20, 1), 1));
    let click = || {
        Action::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        })
    };

    apply(&mut state, click(), &fx, &hits);
    assert_eq!(state.selected, 1);
    assert!(state.now.is_none());

    apply(&mut state, click(), &fx, &hits);
    assert_eq!(state.current_track_id, Some(3));
}

#[tokio::test]
async fn page_home_end_and_tab_follow_the_visible_library() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.library = (0..30).map(row).collect();
    state.update(Action::Resize { cols: 80, rows: 10 }, &fx);

    state.update(Action::MovePage(1), &fx);
    assert_eq!(state.selected, 7);
    state.update(Action::JumpBottom, &fx);
    assert_eq!(state.selected, 29);
    state.update(Action::JumpTop, &fx);
    assert_eq!(state.selected, 0);

    state.update(Action::ToggleLibraryFocus, &fx);
    assert!(state.sidebar_focus);
    state.update(Action::ToggleLibraryFocus, &fx);
    assert!(!state.sidebar_focus);
    state.update(Action::Resize { cols: 40, rows: 10 }, &fx);
    state.update(Action::ToggleLibraryFocus, &fx);
    assert!(!state.sidebar_focus);
}

#[tokio::test]
async fn restored_playback_stays_paused_until_space_then_seeks_after_start() {
    let directory = tempfile::tempdir().unwrap();
    let mut fx = effects(&directory);
    fx.cache_root = Some(directory.path().join("audio"));
    let mut state = AppState::new(&Config::default());
    state.restore_playback(crate::store::StoredPlayback {
        queue: vec![crate::store::StoredSong::from(&row(7))],
        current: Some(crate::store::StoredSong::from(&row(7))),
        queue_pos: Some(0),
        position_ms: 42_000,
        volume: 0.0,
        volume_before_mute: Some(0.8),
        play_mode: PlayMode::List,
        shuffle: true,
        queue_source: Source::Fm,
    });

    assert!(state.paused);
    assert_eq!(state.generation, 0);
    assert_eq!(state.position, Duration::from_secs(42));
    assert_eq!(state.current_track_id, Some(7));
    assert!(state.status.is_none());
    assert_eq!(
        state.now.as_ref().map(|now| now.album.as_str()),
        Some("Album")
    );
    // A restored session parks on the dashboard until the user engages.
    assert!(state.dashboard_hold);

    state.update(Action::TogglePlay, &fx);
    assert!(!state.dashboard_hold, "space reveals the player");
    assert_eq!(state.generation, 1);
    assert_eq!(state.position, Duration::from_secs(42));
    assert_eq!(state.status.as_deref(), Some(i18n::t(Key::Resolving)));
    assert_eq!(state.queue_source, Source::Fm);

    state.update(
        Action::Player(PlayerEvent::Started {
            generation: 1,
            total: Some(Duration::from_secs(180)),
        }),
        &fx,
    );
    assert_eq!(state.position, Duration::from_secs(42));
    assert!(state.seek_after_start.is_none());
    assert!(state.resume_on_play.is_none());
}

#[test]
fn restored_mini_player_recovers_the_title_and_follows_the_icon_style() {
    let queued = named_row(7, "Recovered title", "Recovered artist");
    let mut stored_current = crate::store::StoredSong::from(&queued);
    stored_current.title.clear();
    let mut state = AppState::new(&Config {
        icons: crate::config::IconStyle::Nerd,
        ..Config::default()
    });
    state.restore_playback(crate::store::StoredPlayback {
        queue: vec![crate::store::StoredSong::from(&queued)],
        current: Some(stored_current),
        queue_pos: Some(0),
        position_ms: 42_000,
        volume: 0.7,
        volume_before_mute: None,
        play_mode: PlayMode::Off,
        shuffle: false,
        queue_source: Source::Liked,
    });
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Recovered title")
    );

    let backend = TestBackend::new(80, 6);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut hits = ui::Hits::default();
    terminal
        .draw(|frame| ui::draw(frame, &mut state, &mut hits))
        .unwrap();

    let first = (0..80)
        .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
        .collect::<String>();
    // icons = nerd must reach the state glyph too, matching heart/volume.
    assert!(
        first.contains('\u{f04b}'),
        "missing pause-state icon: {first:?}"
    );
    assert!(
        first.contains("Recovered title"),
        "missing restored title: {first:?}"
    );

    state.paused = false;
    terminal
        .draw(|frame| ui::draw(frame, &mut state, &mut hits))
        .unwrap();
    let first = (0..80)
        .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
        .collect::<String>();
    assert!(
        first.contains('\u{f04c}'),
        "missing playing-state icon: {first:?}"
    );
}

#[tokio::test]
async fn a_failed_restored_track_can_be_retried_with_space() {
    let directory = tempfile::tempdir().unwrap();
    let mut fx = effects(&directory);
    fx.cache_root = Some(directory.path().join("audio"));
    let mut state = AppState::new(&Config::default());
    state.restore_playback(crate::store::StoredPlayback {
        queue: vec![crate::store::StoredSong::from(&row(7))],
        current: Some(crate::store::StoredSong::from(&row(7))),
        queue_pos: Some(0),
        position_ms: 42_000,
        volume: 0.7,
        volume_before_mute: None,
        play_mode: PlayMode::Off,
        shuffle: false,
        queue_source: Source::Liked,
    });

    state.update(Action::TogglePlay, &fx);
    state.update(
        Action::ResolveFailed {
            generation: 1,
            message: "offline".into(),
        },
        &fx,
    );
    assert_eq!(state.resume_on_play, Some(Duration::from_secs(42)));

    state.update(Action::TogglePlay, &fx);
    assert_eq!(state.generation, 2);
    assert_eq!(state.position, Duration::from_secs(42));
}

#[tokio::test]
async fn current_unavailable_track_reports_failure_and_advances_the_queue() {
    let directory = tempfile::tempdir().unwrap();
    let mut fx = effects(&directory);
    fx.cache_root = Some(directory.path().join("audio"));
    let mut state = AppState::new(&Config::default());
    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(0);

    state.update(Action::TrackUnavailable { generation: 0 }, &fx);

    assert_eq!(state.queue_pos, Some(1));
    assert_eq!(state.generation, 1);
    assert_eq!(
        state.status.as_deref(),
        Some(i18n::t(Key::TrackUnavailable))
    );
}

#[tokio::test]
async fn unavailable_track_never_wraps_a_list_queue_and_can_be_retried() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.queue = vec![row(1)];
    state.queue_pos = Some(0);
    state.active_row = Some(row(1));
    state.play_mode = PlayMode::List;

    state.update(Action::TrackUnavailable { generation: 0 }, &fx);

    assert_eq!(state.queue_pos, Some(0));
    assert_eq!(state.generation, 0);
    assert!(state.paused);
    assert_eq!(state.resume_on_play, Some(Duration::ZERO));
    assert_eq!(
        state.status.as_deref(),
        Some(i18n::t(Key::TrackUnavailable))
    );

    state.update(Action::TogglePlay, &fx);
    assert_eq!(state.generation, 1);
}

#[tokio::test]
async fn unm_media_failure_reports_unavailable_and_advances() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(0);
    state.active_row = Some(row(1));
    state.generation = 4;

    state.update(
        Action::Player(PlayerEvent::Failed {
            generation: 4,
            message: "decode failed".into(),
            cached: None,
            unm_source: true,
        }),
        &fx,
    );

    assert_eq!(state.queue_pos, Some(1));
    assert_eq!(state.generation, 5);
    assert_eq!(
        state.status.as_deref(),
        Some(i18n::t(Key::TrackUnavailable))
    );
}

#[tokio::test]
async fn ordinary_player_failure_does_not_trigger_unm_auto_next() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(0);

    state.update(
        Action::Player(PlayerEvent::Failed {
            generation: 0,
            message: "audio device unavailable".into(),
            cached: None,
            unm_source: false,
        }),
        &fx,
    );

    assert_eq!(state.queue_pos, Some(0));
    assert_eq!(state.generation, 0);
    assert_eq!(state.status.as_deref(), Some("audio device unavailable"));
}

#[tokio::test]
async fn current_unm_result_uses_the_localised_source_status() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.generation = 3;

    state.update(
        Action::TrackResolved {
            generation: 3,
            track: api::ResolvedTrack {
                id: 42,
                title: "Recovered".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                media: api::ResolvedMedia::UnmBytes(Vec::new()),
                kind: "mp3".into(),
                cache_key: CacheKey::new(42, AudioQuality::High320),
                codec: AudioCodec::Mp3,
                actual_bitrate: 128_000,
                expected_bytes: None,
                expected_md5: None,
                duration_ms: 180_000,
                pic_url: None,
                artist_id: None,
                album_id: None,
            },
        },
        &fx,
    );

    assert_eq!(state.status.as_deref(), Some(i18n::t(Key::UnmSourceUsed)));
    assert_eq!(state.current_track_id, Some(42));
    assert_eq!(
        state.now.as_ref().map(|now| now.album.as_str()),
        Some("Album")
    );
}

#[tokio::test]
async fn stale_unavailable_result_cannot_skip_the_current_track() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(0);
    state.generation = 7;

    state.update(Action::TrackUnavailable { generation: 6 }, &fx);

    assert_eq!(state.queue_pos, Some(0));
    assert_eq!(state.generation, 7);
    assert!(state.status.is_none());
}

#[tokio::test]
async fn quit_dialog_handles_raw_confirm_and_cancel_keys() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);

    for key in [KeyCode::Char('y'), KeyCode::Enter, KeyCode::Char('q')] {
        let mut state = AppState::new(&Config::default());
        state.update(Action::Quit, &fx);
        state.update(raw_key(key), &fx);
        assert!(state.should_quit, "{key:?} should confirm quitting");
    }

    for key in [KeyCode::Char('n'), KeyCode::Esc] {
        let mut state = AppState::new(&Config::default());
        state.update(Action::Quit, &fx);
        state.update(raw_key(key), &fx);
        assert!(!state.confirm_quit, "{key:?} should cancel quitting");
        assert!(!state.should_quit);
    }

    let mut state = AppState::new(&Config::default());
    state.update(Action::Quit, &fx);
    state.update(
        Action::RawKey(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        &fx,
    );
    assert!(state.should_quit);
}

#[tokio::test]
async fn settings_preview_can_be_cancelled_without_touching_disk() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let original_theme = state.theme;

    state.update(raw_key(KeyCode::Char(',')), &fx);
    assert_eq!(state.view, View::Settings);
    state.update(raw_key(KeyCode::Right), &fx);

    assert_eq!(state.config.theme, "pico8");
    assert_ne!(state.theme, original_theme);
    assert!(!fx.config_path.exists());

    state.update(raw_key(KeyCode::Esc), &fx);

    assert_eq!(state.view, View::NowPlaying);
    assert_eq!(state.config.theme, "db16");
    assert_eq!(state.theme, original_theme);
    assert!(!fx.config_path.exists());
}

#[tokio::test]
async fn intro_and_cover_size_settings_cycle_and_persist() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.update(Action::SwitchView(View::Settings), &fx);

    state.settings.selected = super::settings::SettingField::ALL
        .iter()
        .position(|field| *field == super::settings::SettingField::CoverSize)
        .unwrap();
    state.update(Action::AdjustSetting(1), &fx);
    assert_eq!(state.config.cover_size, CoverSize::Large);

    state.settings.selected = super::settings::SettingField::ALL
        .iter()
        .position(|field| *field == super::settings::SettingField::IntroAnimation)
        .unwrap();
    state.update(Action::AdjustSetting(1), &fx);
    // Cycling from the default (notes) lands on the next style and starts
    // the full-screen preview immediately.
    assert_eq!(
        state.config.intro_animation,
        crate::config::IntroStyle::Spectrum
    );
    assert!(state.intro_preview.is_some());
    // Any key dismisses the preview without touching the settings page.
    state.update(
        Action::RawKey(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('x'),
            crossterm::event::KeyModifiers::NONE,
        )),
        &fx,
    );
    assert!(state.intro_preview.is_none());
    assert_eq!(state.view, View::Settings);

    state.update(Action::SaveSettings, &fx);
    let saved: Config = toml::from_str(&std::fs::read_to_string(&fx.config_path).unwrap()).unwrap();
    assert_eq!(saved.cover_size, CoverSize::Large);
    assert_eq!(saved.intro_animation, crate::config::IntroStyle::Spectrum);
}

#[tokio::test]
async fn intro_preview_is_mouse_modal_and_clicks_cannot_press_buttons_beneath() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.update(Action::SwitchView(View::Settings), &fx);
    state.start_intro_preview();
    assert!(state.intro_preview.is_some(), "default style must preview");

    // A click lands on the Save button hidden under the splash; resolving
    // it against hits would save and close the settings page mid-preview.
    let save = ratatui::layout::Rect::new(0, 0, 20, 3);
    let mut hits = crate::ui::Hits::default();
    hits.settings_save.push(save);
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: save.x + 1,
        row: save.y + 1,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    super::apply(&mut state, Action::Mouse(click), &fx, &hits);
    assert!(
        state.intro_preview.is_none(),
        "a click dismisses the splash"
    );
    assert_eq!(
        state.view,
        View::Settings,
        "the button beneath stays unpressed"
    );
    assert!(!fx.config_path.exists(), "nothing was saved by the click");
}

#[tokio::test]
async fn icon_setting_previews_immediately_and_cancel_restores_unicode() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.update(Action::SwitchView(View::Settings), &fx);
    state.settings.selected = super::settings::SettingField::ALL
        .iter()
        .position(|field| *field == super::settings::SettingField::Icons)
        .unwrap();

    state.update(Action::AdjustSetting(1), &fx);
    assert_eq!(state.config.icons, crate::config::IconStyle::Nerd);

    state.update(Action::CancelSettings, &fx);
    assert_eq!(state.config.icons, crate::config::IconStyle::Unicode);
}

#[tokio::test]
async fn cover_detail_setting_rebuilds_pixel_art_immediately() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let previous = state.selected_cover.placeholder.clone();
    let revision = state.style_revision;
    state.update(Action::SwitchView(View::Settings), &fx);
    state.settings.selected = super::settings::SettingField::ALL
        .iter()
        .position(|field| *field == super::settings::SettingField::CoverDetail)
        .unwrap();

    state.update(Action::AdjustSetting(1), &fx);

    assert_eq!(state.config.cover_detail, pixel::CoverDetail::Quad);
    assert_eq!(state.style_revision, revision + 1);
    assert_ne!(state.selected_cover.placeholder, previous);

    state.update(Action::AdjustSetting(1), &fx);
    state.update(Action::AdjustSetting(1), &fx);
    assert_eq!(state.config.cover_detail, pixel::CoverDetail::Octant);

    state.update(Action::AdjustSetting(1), &fx);
    assert_eq!(state.config.cover_detail, pixel::CoverDetail::Half);
}

#[tokio::test]
async fn cover_palette_and_high_pixel_scale_preview_immediately() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.update(Action::SwitchView(View::Settings), &fx);

    state.settings.selected = super::settings::SettingField::ALL
        .iter()
        .position(|field| *field == super::settings::SettingField::CoverPalette)
        .unwrap();
    let palette_revision = state.style_revision;
    state.update(Action::AdjustSetting(1), &fx);
    assert_eq!(state.config.cover_palette, pixel::CoverPalette::Theme);
    assert_eq!(state.style_revision, palette_revision + 1);

    state.settings.selected = super::settings::SettingField::ALL
        .iter()
        .position(|field| *field == super::settings::SettingField::PixelDetail)
        .unwrap();
    for _ in 0..4 {
        state.update(Action::AdjustSetting(1), &fx);
    }
    assert_eq!(state.config.pixel_scale, 4.0);
    assert_eq!(state.pixel_detail_scale, 4.0);

    state.update(Action::CancelSettings, &fx);
    assert_eq!(state.config.cover_palette, pixel::CoverPalette::Original);
    assert_eq!(state.config.pixel_scale, 1.0);
}

#[tokio::test]
async fn spectrum_setting_reflows_cover_geometry_immediately() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    // The stacked layout reserves a bottom band for the spectrum, so
    // enabling it must shrink the cover.
    state.layout = PlayLayout::Stacked;
    state.terminal_size = (60, 40);
    let before = state.desired_cover_cells();

    state.update(Action::SwitchView(View::Settings), &fx);
    state.settings.selected = super::settings::SettingField::ALL
        .iter()
        .position(|field| *field == super::settings::SettingField::SpectrumEnabled)
        .unwrap();
    let revision = state.style_revision;
    state.update(Action::AdjustSetting(1), &fx);

    assert!(state.config.spectrum_enabled);
    assert_eq!(state.style_revision, revision + 1);
    assert!(state.desired_cover_cells().1 < before.1);

    // The side layout hosts the spectrum inside the right panel instead;
    // the cover keeps its geometry.
    state.layout = PlayLayout::Side;
    let side_on = state.desired_cover_cells();
    state.config.spectrum_enabled = false;
    assert_eq!(state.desired_cover_cells(), side_on);
}

#[tokio::test]
async fn escape_walks_out_of_search_instead_of_bouncing_back_to_input() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.update(Action::SwitchView(View::Search), &fx);
    assert!(state.search.input);
    state.search.query = "rain".into();
    let request = state.search.submit().unwrap();
    assert!(state.search.accept(
        request.seq,
        &request.query,
        request.channel,
        request.offset,
        crate::api::SearchPayload::Songs(crate::api::SearchPage {
            items: vec![row(1)],
            total: 1,
        }),
    ));

    // First Esc hands focus to the result list, second Esc leaves the
    // view; navigate_back must not bounce focus back into the input.
    state.update(raw_key(KeyCode::Esc), &fx);
    assert!(!state.search.input);
    state.update(raw_key(KeyCode::Esc), &fx);
    assert_eq!(state.view, View::NowPlaying);
    assert_eq!(state.search.current_len(), 1, "results survive the exit");
}

#[tokio::test]
async fn side_layout_spectrum_toggle_keeps_the_cover_art() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.layout = PlayLayout::Side;
    let revision = state.style_revision;

    state.update(raw_key(KeyCode::Char('v')), &fx);

    assert!(state.config.spectrum_enabled);
    assert_eq!(
        state.style_revision, revision,
        "the side layout hosts the spectrum in the panel; the cover must not reload"
    );
}

#[tokio::test]
async fn global_spectrum_key_toggles_and_persists_immediately() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());

    state.update(raw_key(KeyCode::Char('v')), &fx);

    assert!(state.config.spectrum_enabled);
    let saved: Config = toml::from_str(&std::fs::read_to_string(&fx.config_path).unwrap()).unwrap();
    assert!(saved.spectrum_enabled);
}

#[tokio::test]
async fn spectrum_key_does_not_persist_other_settings_previews() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());

    state.update(Action::SwitchView(View::Settings), &fx);
    state.update(Action::AdjustSetting(1), &fx);
    assert_eq!(state.config.theme, "pico8");
    state.update(raw_key(KeyCode::Char('v')), &fx);
    state.update(Action::CancelSettings, &fx);

    let saved: Config = toml::from_str(&std::fs::read_to_string(&fx.config_path).unwrap()).unwrap();
    assert_eq!(saved.theme, "db16");
    assert!(saved.spectrum_enabled);
    assert_eq!(state.config, saved);
}

#[tokio::test]
async fn settings_save_persists_the_preview_and_updates_playback_quality() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;

    state.update(Action::SwitchView(View::Settings), &fx);
    state.update(Action::AdjustSetting(1), &fx);
    state.settings.selected = super::settings::SettingField::ALL
        .iter()
        .position(|field| *field == super::settings::SettingField::Quality)
        .unwrap();
    state.update(Action::AdjustSetting(1), &fx);
    assert_eq!(state.config.quality, AudioQuality::Lossless);
    assert_eq!(fx.ncm.quality(), AudioQuality::Lossless);

    state.update(Action::SaveSettings, &fx);

    assert_eq!(state.view, View::Library);
    let reloaded: Config =
        toml::from_str(&std::fs::read_to_string(&fx.config_path).unwrap()).unwrap();
    assert_eq!(reloaded.theme, "pico8");
    assert_eq!(reloaded.quality, AudioQuality::Lossless);
}

#[tokio::test]
async fn quality_preview_rejects_a_prefetch_from_the_previous_setting() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.queue = vec![row(42)];
    state.update(Action::SwitchView(View::Settings), &fx);
    state.settings.selected = super::settings::SettingField::ALL
        .iter()
        .position(|field| *field == super::settings::SettingField::Quality)
        .unwrap();
    state.update(Action::AdjustSetting(1), &fx);

    state.update(
        Action::PrefetchReady {
            index: 0,
            track: api::ResolvedTrack {
                id: 42,
                title: "Track 42".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                media: api::ResolvedMedia::NeteaseUrl("https://example.test/audio.mp3".into()),
                kind: "mp3".into(),
                cache_key: CacheKey::new(42, AudioQuality::High320),
                codec: AudioCodec::Mp3,
                actual_bitrate: 320_000,
                expected_bytes: None,
                expected_md5: None,
                duration_ms: 180_000,
                pic_url: None,
                artist_id: None,
                album_id: None,
            },
        },
        &fx,
    );

    assert!(state.prefetched.is_none());
}

#[tokio::test]
async fn a_settings_save_failure_keeps_the_editor_and_preview_open() {
    let directory = tempfile::tempdir().unwrap();
    let mut fx = effects(&directory);
    fx.config_path = directory.path().to_path_buf();
    let mut state = AppState::new(&Config::default());
    state.update(Action::SwitchView(View::Settings), &fx);
    state.update(Action::AdjustSetting(1), &fx);

    state.update(Action::SaveSettings, &fx);

    assert_eq!(state.view, View::Settings);
    assert_eq!(state.config.theme, "pico8");
    assert!(state.settings.feedback.as_deref().is_some_and(
        |message| message.starts_with(crate::i18n::t(crate::i18n::Key::SettingsSaveFailed))
    ));
    assert!(state.status.is_none());
    assert!(directory.path().is_dir());
}

#[tokio::test]
async fn quit_dialog_keeps_processing_async_state_updates() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.update(Action::Quit, &fx);

    state.update(
        Action::LyricsLoaded {
            generation: 0,
            lines: vec![crate::lyrics::LyricLine {
                time: Duration::from_secs(1),
                text: "new lyric".into(),
                translation: None,
                word_timing: Some(crate::yrc::YrcLine {
                    start: Duration::from_secs(1),
                    duration: Duration::from_millis(500),
                    words: vec![crate::yrc::YrcWord {
                        text: "new lyric".into(),
                        start: Duration::from_secs(1),
                        duration: Duration::from_millis(500),
                    }],
                }),
            }],
        },
        &fx,
    );

    assert!(state.confirm_quit);
    assert_eq!(state.lyrics[0].text, "new lyric");
    assert_eq!(
        state.lyrics[0].word_timing.as_ref().map(|line| line.text()),
        Some("new lyric".into())
    );
}

#[tokio::test]
async fn stale_lyrics_cannot_replace_the_current_tracks_word_timeline() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.generation = 2;

    state.update(
        Action::LyricsLoaded {
            generation: 1,
            lines: vec![crate::lyrics::LyricLine {
                time: Duration::ZERO,
                text: "stale".into(),
                translation: None,
                word_timing: Some(crate::yrc::YrcLine {
                    start: Duration::ZERO,
                    duration: Duration::from_secs(1),
                    words: vec![crate::yrc::YrcWord {
                        text: "stale".into(),
                        start: Duration::ZERO,
                        duration: Duration::from_secs(1),
                    }],
                }),
            }],
        },
        &fx,
    );

    assert!(state.lyrics.is_empty());
}

#[tokio::test]
async fn track_end_advances_the_queue_while_quit_dialog_is_open() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(0);
    state.update(Action::Quit, &fx);

    state.update(Action::Player(PlayerEvent::Ended { generation: 0 }), &fx);

    assert!(state.confirm_quit);
    assert_eq!(state.queue_pos, Some(1));
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Track 2")
    );
}

#[tokio::test]
async fn editing_search_rejects_results_and_failures_for_the_previous_query() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let edits = [
        raw_key(KeyCode::Char('x')),
        Action::Paste("x".into()),
        raw_key(KeyCode::Esc),
    ];

    for edit in edits {
        let mut state = AppState::new(&Config::default());
        state.view = View::Search;
        state.search.query = "old".into();
        let request = state.search.submit().unwrap();
        let stale_seq = request.seq;
        let stale_query = request.query.clone();

        state.update(edit, &fx);
        state.update(
            Action::SearchResults {
                seq: request.seq,
                query: request.query,
                channel: request.channel,
                offset: request.offset,
                payload: crate::api::SearchPayload::Songs(crate::api::SearchPage {
                    items: vec![row(1)],
                    total: 1,
                }),
            },
            &fx,
        );
        state.update(
            Action::SearchFailed {
                seq: stale_seq,
                query: stale_query,
                channel: request.channel,
                offset: request.offset,
                message: "old failure".into(),
            },
            &fx,
        );

        assert!(state.search.songs.items.is_empty());
        assert!(state.search.current_error().is_none());
        assert!(!state.search.current_searching());
        assert!(state.search.input);
    }
}

#[tokio::test]
async fn search_row_click_selects_first_then_activates() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.query = "query".into();
    let request = state.search.submit().unwrap();
    assert!(state.search.accept(
        request.seq,
        &request.query,
        request.channel,
        request.offset,
        crate::api::SearchPayload::Songs(crate::api::SearchPage {
            items: vec![row(1), row(2)],
            total: 2,
        }),
    ));
    assert!(state.search.input);
    state.selected = 0;

    let mut hits = ui::Hits::default();
    hits.rows.push((Rect::new(4, 5, 20, 1), 0));
    let click = Action::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });

    apply(&mut state, click, &fx, &hits);
    assert_eq!(state.view, View::Search);
    assert_eq!(state.selected, 0);
    assert!(!state.search.input);

    apply(
        &mut state,
        Action::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        }),
        &fx,
        &hits,
    );
    assert_eq!(state.view, View::NowPlaying);
}

#[tokio::test]
async fn selecting_a_different_search_row_focuses_the_result_list() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = true;
    state.search.songs.items = vec![row(1), row(2)];

    state.update(Action::SelectIndex(1), &fx);

    assert_eq!(state.selected, 1);
    assert!(!state.search.input);
}

#[tokio::test]
async fn jump_bottom_reaches_the_last_search_result() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = false;
    state.search.songs.items = vec![row(1), row(2), row(3)];

    state.update(Action::JumpBottom, &fx);

    assert_eq!(state.selected, 2);
}

#[tokio::test]
async fn reaching_the_search_tail_starts_the_next_page_without_hiding_rows() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = false;
    state.search.query = "query".into();
    let request = state.search.submit().unwrap();
    assert!(state.search.accept(
        request.seq,
        &request.query,
        request.channel,
        request.offset,
        crate::api::SearchPayload::Songs(crate::api::SearchPage {
            items: vec![row(1), row(2)],
            total: 31,
        }),
    ));

    state.update(Action::MoveSelection(1), &fx);

    assert_eq!(state.selected, 1);
    assert_eq!(state.search.current_len(), 2);
    assert!(state.search.current_searching());
}

#[tokio::test]
async fn an_empty_filtered_search_page_advances_the_server_cursor() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.query = "query".into();
    let request = state.search.submit().unwrap();

    state.update(
        Action::SearchResults {
            seq: request.seq,
            query: request.query,
            channel: request.channel,
            offset: request.offset,
            payload: crate::api::SearchPayload::Songs(crate::api::SearchPage {
                items: Vec::new(),
                total: 31,
            }),
        },
        &fx,
    );

    assert_eq!(state.search.current_len(), 0);
    assert!(state.search.current_searching());
}

#[tokio::test]
async fn filtered_search_results_do_not_load_an_unfiltered_next_page() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = false;
    state.search.query = "query".into();
    let request = state.search.submit().unwrap();
    assert!(state.search.accept(
        request.seq,
        &request.query,
        request.channel,
        request.offset,
        crate::api::SearchPayload::Songs(crate::api::SearchPage {
            items: vec![row(1), row(2)],
            total: 31,
        }),
    ));
    state.filter.query = "Track 2".into();

    state.update(Action::JumpBottom, &fx);

    assert_eq!(state.selected, 0);
    assert!(!state.search.current_searching());
}

#[tokio::test]
async fn next_page_failure_stays_visible_after_leaving_search() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.query = "query".into();
    let first = state.search.submit().unwrap();
    assert!(state.search.accept(
        first.seq,
        &first.query,
        first.channel,
        first.offset,
        crate::api::SearchPayload::Songs(crate::api::SearchPage {
            items: vec![row(1), row(2)],
            total: 31,
        }),
    ));
    let next = state.search.load_more().unwrap();
    state.view = View::Library;

    state.update(
        Action::SearchFailed {
            seq: next.seq,
            query: next.query,
            channel: next.channel,
            offset: next.offset,
            message: "next page failed".into(),
        },
        &fx,
    );

    assert_eq!(state.search.current_len(), 2);
    assert_eq!(state.status.as_deref(), Some("next page failed"));
}

#[tokio::test]
async fn search_entity_activation_pushes_and_back_pops_the_detail_page() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = false;
    state.search.channel = crate::api::SearchChannel::Artists;
    state.search.artists.items = vec![
        crate::api::ArtistHit {
            img1v1_url: None,
            id: 1,
            name: "First".into(),
            pic_url: None,
            album_count: 1,
            song_count: 2,
        },
        crate::api::ArtistHit {
            img1v1_url: None,
            id: 2,
            name: "Second".into(),
            pic_url: None,
            album_count: 3,
            song_count: 4,
        },
    ];
    state.selected = 1;

    state.update(Action::Activate, &fx);

    assert_eq!(state.search.detail_title(), Some("Second"));
    assert_eq!(state.selected, 0);
    assert!(!state.search.input);

    state.update(Action::Back, &fx);

    assert!(state.search.is_results());
    assert_eq!(state.search.channel, crate::api::SearchChannel::Artists);
    assert_eq!(state.selected, 1);
    assert!(!state.search.input);
}

#[tokio::test]
async fn detail_tracks_support_enqueue_and_context_playback() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = false;
    state.search.channel = crate::api::SearchChannel::Albums;
    state.search.albums.items.push(crate::api::AlbumHit {
        mark: 0,
        id: 7,
        name: "Album".into(),
        artist: yesplaymusic_core::ncm::ArtistRef {
            id: 0,
            name: "Artist".into(),
        },
        pic_url: None,
        song_count: 2,
    });
    let request = state.search.open_detail(0).unwrap();
    state.update(
        Action::SearchDetailLoaded {
            seq: request.seq,
            channel: request.channel,
            id: request.id,
            rows: vec![row(10), row(11)],
        },
        &fx,
    );
    state.selected = 1;

    state.update(Action::AddSelectedToQueue, &fx);
    assert_eq!(
        state.queue.iter().map(|row| row.id).collect::<Vec<_>>(),
        [11]
    );

    state.update(Action::Activate, &fx);
    assert_eq!(state.view, View::NowPlaying);
    assert_eq!(
        state.queue.iter().map(|row| row.id).collect::<Vec<_>>(),
        [10, 11]
    );
    assert_eq!(state.queue_pos, Some(1));
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Track 11")
    );
}

#[tokio::test]
async fn detail_tracks_support_local_filtering() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = false;
    state.search.channel = crate::api::SearchChannel::Albums;
    state.search.albums.items.push(crate::api::AlbumHit {
        mark: 0,
        id: 7,
        name: "Album".into(),
        artist: yesplaymusic_core::ncm::ArtistRef {
            id: 0,
            name: "Artist".into(),
        },
        pic_url: None,
        song_count: 2,
    });
    let request = state.search.open_detail(0).unwrap();
    state.update(
        Action::SearchDetailLoaded {
            seq: request.seq,
            channel: request.channel,
            id: request.id,
            rows: vec![named_row(10, "Alpha", "One"), named_row(11, "Beta", "Two")],
        },
        &fx,
    );

    state.update(raw_key(KeyCode::Char('/')), &fx);
    state.update(raw_key(KeyCode::Char('b')), &fx);
    state.update(raw_key(KeyCode::Char('t')), &fx);

    assert!(state.filter.input);
    assert_eq!(state.visible_len(), 1);
    assert_eq!(state.visible_row(0).map(|(_, row)| row.id), Some(11));

    state.update(raw_key(KeyCode::Enter), &fx);
    state.update(Action::AddSelectedToQueue, &fx);

    assert!(!state.filter.input);
    assert_eq!(
        state.queue.iter().map(|row| row.id).collect::<Vec<_>>(),
        [11]
    );
}

#[tokio::test]
async fn detail_track_is_used_for_the_selection_cover_preview() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.terminal_size = (
        ui::search::PREVIEW_MIN_TERMINAL_WIDTH,
        ui::cover_preview::HEIGHT + ui::HEADER_HEIGHT + ui::FOOTER_HEIGHT + ui::PANEL_GAP_Y * 2,
    );
    state.search.input = false;
    state.search.channel = crate::api::SearchChannel::Playlists;
    state.search.playlists.items.push(crate::api::PlaylistHit {
        privacy: 0,
        id: 8,
        name: "Playlist".into(),
        creator: "Listener".into(),
        cover_url: None,
        track_count: 1,
    });
    let request = state.search.open_detail(0).unwrap();
    state.update(
        Action::SearchDetailLoaded {
            seq: request.seq,
            channel: request.channel,
            id: request.id,
            rows: vec![covered_row(9)],
        },
        &fx,
    );

    let (key, row, neighbors) = state.selected_cover_candidate().unwrap();

    assert_eq!(key.id, 9);
    assert_eq!(row.pic_url.as_deref(), Some("https://example.test/9.jpg"));
    assert!(neighbors.is_empty());
}

#[tokio::test]
async fn late_detail_result_does_not_move_another_views_selection() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.search.channel = crate::api::SearchChannel::Playlists;
    state.search.playlists.items.push(crate::api::PlaylistHit {
        privacy: 0,
        id: 8,
        name: "Playlist".into(),
        creator: "Listener".into(),
        cover_url: None,
        track_count: 1,
    });
    let request = state.search.open_detail(0).unwrap();
    state.view = View::Library;
    state.library = vec![row(1), row(2), row(3)];
    state.selected = 2;

    state.update(
        Action::SearchDetailLoaded {
            seq: request.seq,
            channel: request.channel,
            id: request.id,
            rows: vec![row(9)],
        },
        &fx,
    );

    assert_eq!(state.selected, 2);
    assert_eq!(state.search.song_rows().unwrap()[0].id, 9);

    state.update(Action::SwitchView(View::Search), &fx);

    assert_eq!(state.selected, 0);
    assert_eq!(state.visible_len(), 1);
}

#[tokio::test]
async fn search_detail_selection_survives_a_round_trip_through_another_view() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.channel = crate::api::SearchChannel::Albums;
    state.search.albums.items.push(crate::api::AlbumHit {
        mark: 0,
        id: 7,
        name: "Album".into(),
        artist: yesplaymusic_core::ncm::ArtistRef {
            id: 0,
            name: "Artist".into(),
        },
        pic_url: None,
        song_count: 3,
    });
    let request = state.search.open_detail(0).unwrap();
    state.update(
        Action::SearchDetailLoaded {
            seq: request.seq,
            channel: request.channel,
            id: request.id,
            rows: vec![row(1), row(2), row(3)],
        },
        &fx,
    );
    state.selected = 2;

    state.update(Action::SwitchView(View::Library), &fx);
    state.library = vec![row(10), row(11)];
    state.selected = 1;
    state.update(Action::SwitchView(View::Search), &fx);

    assert_eq!(state.selected, 2);
    assert_eq!(state.visible_row(2).map(|(_, row)| row.id), Some(3));
}

#[tokio::test]
async fn a_refresh_cannot_activate_hidden_stale_search_rows() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.query = "query".into();
    let first = state.search.submit().unwrap();
    assert!(state.search.accept(
        first.seq,
        &first.query,
        first.channel,
        first.offset,
        crate::api::SearchPayload::Songs(crate::api::SearchPage {
            items: vec![row(1)],
            total: 1,
        }),
    ));
    assert!(state.search.submit().is_some());

    state.update(raw_key(KeyCode::Down), &fx);
    state.update(Action::Activate, &fx);

    assert!(state.search.input);
    assert!(state.now.is_none());
    assert_eq!(state.visible_len(), 0);
}

#[tokio::test]
async fn search_channel_keys_wrap_without_triggering_seek() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = true;

    state.update(raw_key(KeyCode::BackTab), &fx);
    assert_eq!(state.search.channel, crate::api::SearchChannel::Playlists);

    state.search.input = false;
    state.update(raw_key(KeyCode::Right), &fx);
    assert_eq!(state.search.channel, crate::api::SearchChannel::Songs);
}

#[tokio::test]
async fn switching_search_channels_preserves_the_underlying_filtered_song() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = false;
    state.search.songs.items = vec![
        named_row(1, "Alpha", "One"),
        named_row(2, "Target one", "Two"),
        named_row(3, "Target two", "Three"),
    ];
    state.filter.query = "target".into();
    state.selected = 1;

    state.update(
        Action::SelectSearchChannel(crate::api::SearchChannel::Artists),
        &fx,
    );
    state.update(
        Action::SelectSearchChannel(crate::api::SearchChannel::Songs),
        &fx,
    );

    assert_eq!(state.selected, 2);
    assert!(state.filter.query.is_empty());
    assert_eq!(state.visible_row(2).map(|(_, row)| row.id), Some(3));
}

#[tokio::test]
async fn returning_to_search_input_preserves_the_result_selection() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = false;
    state.search.songs.items = vec![row(1), row(2), row(3)];
    state.selected = 2;

    state.update(Action::Back, &fx);
    assert!(state.search.input);
    state.update(raw_key(KeyCode::Down), &fx);

    assert!(!state.search.input);
    assert_eq!(state.selected, 2);
}

#[tokio::test]
async fn selecting_a_library_row_moves_focus_from_the_sidebar_before_activation() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.library = vec![row(1), row(2)];
    state.sidebar_focus = true;

    state.update(Action::SelectIndex(1), &fx);
    state.update(Action::Activate, &fx);

    assert_eq!(state.view, View::NowPlaying);
    assert_eq!(state.queue_pos, Some(1));
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Track 2")
    );
}

#[tokio::test]
async fn first_click_on_the_selected_library_row_only_moves_focus_from_the_sidebar() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.library = vec![row(1), row(2)];
    state.selected = 0;
    state.sidebar_focus = true;
    let mut hits = ui::Hits::default();
    hits.rows.push((Rect::new(4, 5, 20, 1), 0));
    let click = || {
        Action::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        })
    };

    apply(&mut state, click(), &fx, &hits);

    assert_eq!(state.view, View::Library);
    assert!(!state.sidebar_focus);
    assert!(state.queue.is_empty());

    apply(&mut state, click(), &fx, &hits);

    assert_eq!(state.view, View::NowPlaying);
    assert_eq!(state.queue_pos, Some(0));
}

#[tokio::test]
async fn a_bare_pointer_move_leaves_the_help_overlay_open() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.show_help = true;
    let hits = ui::Hits::default();

    // Brushing the mouse used to close the key map mid-read.
    for kind in [
        MouseEventKind::Moved,
        MouseEventKind::ScrollDown,
        MouseEventKind::Drag(MouseButton::Left),
    ] {
        apply(
            &mut state,
            Action::Mouse(MouseEvent {
                kind,
                column: 5,
                row: 5,
                modifiers: KeyModifiers::NONE,
            }),
            &fx,
            &hits,
        );
        assert!(state.show_help, "{kind:?} keeps the overlay open");
    }
}

#[tokio::test]
async fn mouse_click_over_a_help_overlay_only_dismisses_the_overlay() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.show_help = true;
    let mut hits = ui::Hits::default();
    hits.tabs.push((Rect::new(4, 1, 20, 1), View::Search));

    apply(
        &mut state,
        Action::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }),
        &fx,
        &hits,
    );

    assert!(!state.show_help);
    assert_eq!(state.view, View::Library);
    assert!(state.queue.is_empty());
}

#[tokio::test]
async fn fullwidth_question_mark_from_an_ime_opens_the_help_overlay() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());

    // A Chinese IME sends ？ where the binding says ?.
    state.update(raw_key(KeyCode::Char('？')), &fx);
    assert!(state.show_help);
}

#[tokio::test]
async fn ctrl_l_repaints_without_dismissing_the_help_overlay() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.show_help = true;

    state.update(
        Action::RawKey(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)),
        &fx,
    );
    assert!(state.force_redraw, "Ctrl+L must request a full repaint");
    assert!(state.show_help, "the overlay survives a repaint request");

    // Regained focus arrives pre-mapped and must do the same.
    state.force_redraw = false;
    state.update(Action::ForceRedraw, &fx);
    assert!(state.force_redraw);
    assert!(state.show_help);
}

#[tokio::test]
async fn ctrl_l_repaints_instead_of_typing_into_the_search_box() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Search;
    state.search.input = true;
    state.search.query = "needle".into();

    state.update(
        Action::RawKey(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)),
        &fx,
    );
    assert!(state.force_redraw);
    assert_eq!(state.search.query, "needle");
}

#[tokio::test]
async fn narrowing_the_terminal_clears_hidden_sidebar_focus() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.update(Action::Resize { cols: 80, rows: 24 }, &fx);
    state.update(Action::Back, &fx);
    assert!(state.sidebar_focus);

    state.update(Action::Resize { cols: 40, rows: 24 }, &fx);

    assert!(!state.sidebar_focus);
    assert_eq!(state.view, View::Library);
}

#[tokio::test]
async fn back_leaves_library_when_the_sidebar_is_hidden() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.update(Action::Resize { cols: 40, rows: 24 }, &fx);

    state.update(Action::Back, &fx);

    assert_eq!(state.view, View::NowPlaying);
    assert!(!state.sidebar_focus);
}

#[test]
fn starting_a_cover_load_clears_the_previous_song_art() {
    let mut state = AppState::new(&Config::default());
    state.cover = Some(pixel::vinyl(
        state.theme.palette,
        state.theme.bg,
        4,
        2,
        pixel::CoverDetail::Half,
    ));

    state.clear_cover();

    assert!(state.cover.is_none());
}

#[test]
fn cover_result_for_an_old_size_cannot_replace_the_current_cover() {
    let theme = Theme::db16();
    let current = pixel::vinyl(theme.palette, theme.bg, 4, 2, pixel::CoverDetail::Half);
    let replacement = pixel::vinyl(
        theme.palette,
        Color::Rgb(1, 2, 3),
        4,
        2,
        pixel::CoverDetail::Half,
    );
    let stale = pixel::vinyl(theme.palette, theme.bg, 8, 4, pixel::CoverDetail::Half);
    let mut slot = Some(current.clone());

    apply_pixel_cover(
        &mut slot,
        7,
        (4, 2),
        3,
        CoverRenderRequest {
            surface: CoverSurface::Playing,
            generation: 7,
            cells: (4, 2),
            style_revision: 3,
            song_id: 1,
            source_key: "source".into(),
            source_edge: COVER_SOURCE_EDGE,
        },
        replacement.clone(),
    );
    assert_eq!(slot, Some(replacement.clone()));

    apply_pixel_cover(
        &mut slot,
        7,
        (4, 2),
        3,
        CoverRenderRequest {
            surface: CoverSurface::Playing,
            generation: 6,
            cells: (4, 2),
            style_revision: 3,
            song_id: 1,
            source_key: "source".into(),
            source_edge: COVER_SOURCE_EDGE,
        },
        current,
    );
    apply_pixel_cover(
        &mut slot,
        7,
        (4, 2),
        3,
        CoverRenderRequest {
            surface: CoverSurface::Playing,
            generation: 7,
            cells: (8, 4),
            style_revision: 3,
            song_id: 1,
            source_key: "source".into(),
            source_edge: COVER_SOURCE_EDGE,
        },
        stale,
    );

    let previous_theme = pixel::vinyl(
        Theme::db16().palette,
        Color::Rgb(9, 9, 9),
        4,
        2,
        pixel::CoverDetail::Half,
    );
    apply_pixel_cover(
        &mut slot,
        7,
        (4, 2),
        4,
        CoverRenderRequest {
            surface: CoverSurface::Playing,
            generation: 7,
            cells: (4, 2),
            style_revision: 3,
            song_id: 1,
            source_key: "source".into(),
            source_edge: COVER_SOURCE_EDGE,
        },
        previous_theme,
    );

    assert_eq!(slot, Some(replacement));
}

#[test]
fn graphics_picker_only_accepts_a_real_graphics_protocol() {
    assert!(select_graphics_picker(None).is_none());
    assert!(select_graphics_picker(Some(Picker::halfblocks())).is_none());

    let mut kitty = Picker::halfblocks();
    kitty.set_protocol_type(ProtocolType::Kitty);
    let selected = select_graphics_picker(Some(kitty)).unwrap();
    assert_eq!(selected.protocol_type(), ProtocolType::Kitty);

    // The probe runs regardless of cover mode so pixel ↔ original can
    // switch live; a real protocol is accepted even in pixel mode.
    let mut sixel = Picker::halfblocks();
    sixel.set_protocol_type(ProtocolType::Sixel);
    assert!(select_graphics_picker(Some(sixel)).is_some());
}

#[tokio::test]
async fn a_known_row_replaces_the_old_track_identity_before_resolution() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.current_track_id = Some(99);
    state.paused = true;

    state.play_row(&fx, row(1));

    assert_eq!(state.current_track_id, Some(1));
    assert!(!state.paused);
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Track 1")
    );
}

#[tokio::test]
async fn an_unresolved_demo_row_does_not_keep_the_previous_track_identity() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.current_track_id = Some(99);
    let mut unresolved = row(0);
    unresolved.title = "Search me".into();

    state.play_row(&fx, unresolved);

    assert_eq!(state.current_track_id, None);
}

#[tokio::test]
async fn a_known_track_uses_the_shared_cache_before_resolving_a_url() {
    let directory = tempfile::tempdir().unwrap();
    let cache_root = directory.path().join("audio");
    let key = CacheKey::new(42, yesplaymusic_core::cache::AudioQuality::High320);
    let cache = TrackCache::open(&cache_root).unwrap();
    let mut writer = cache
        .begin_write(CacheWriteRequest::new(
            key,
            yesplaymusic_core::cache::AudioCodec::Mp3,
            320_000,
        ))
        .unwrap();
    writer.write_all(b"cached audio").unwrap();
    writer.finish().unwrap();
    drop(cache);

    let (player, _events) = player::spawn(tokio::runtime::Handle::current());
    let (actions, mut receiver) = mpsc::unbounded_channel();
    let fx = Effects {
        player,
        ncm: Arc::new(Ncm::new(
            directory.path().join("session.json"),
            yesplaymusic_core::cache::AudioQuality::High320,
            true,
        )),
        store: Arc::new(crate::store::LibraryStore::new(
            directory.path().join("library"),
        )),
        actions,
        cache_root: Some(cache_root),
        covers: None,
        config_path: directory.path().join("config.toml"),
    };
    let mut state = AppState::new(&Config::default());
    state.play_row(&fx, row(42));

    let action = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .expect("cache lookup should finish")
        .expect("cache lookup action");
    assert!(matches!(
        &action,
        Action::RowCacheReady {
            generation: 1,
            row: cached_row,
            lease: Some(_),
        } if cached_row.id == 42
    ));
    state.update(action, &fx);

    assert_eq!(state.current_track_id, Some(42));
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Track 42")
    );
}

#[tokio::test]
async fn command_palette_owns_mini_player_input_and_blocks_background_mouse_hits() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.terminal_size = (80, 6);
    state.view = View::Queue;
    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(0);
    state.show_help = true;

    state.update(raw_key(KeyCode::Char(':')), &fx);
    assert!(state.command_palette.open);
    assert!(!state.show_help);

    state.update(raw_key(KeyCode::Char('n')), &fx);
    assert_eq!(state.command_palette.query, "n");
    assert_eq!(state.queue_pos, Some(0));

    let mut hits = ui::Hits::default();
    hits.rows.push((Rect::new(0, 1, 20, 1), 1));
    apply(
        &mut state,
        Action::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }),
        &fx,
        &hits,
    );
    assert_eq!(state.selected, 0);

    state.update(raw_key(KeyCode::Esc), &fx);
    assert!(!state.command_palette.open);
}

#[tokio::test]
async fn colon_remains_text_inside_search_and_list_filter_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());

    state.view = View::Search;
    state.search.input = true;
    state.update(raw_key(KeyCode::Char(':')), &fx);
    assert_eq!(state.search.query, ":");
    assert!(!state.command_palette.open);

    state.view = View::Library;
    state.search.input = false;
    state.filter.start();
    state.update(raw_key(KeyCode::Char(':')), &fx);
    assert_eq!(state.filter.query, ":");
    assert!(!state.command_palette.open);
}

#[tokio::test]
async fn command_theme_and_quality_survive_canceling_an_open_settings_preview() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Library;
    state.open_settings(&fx);

    state.update(Action::OpenCommandPalette, &fx);
    state.update(Action::Paste("主题 pico8".into()), &fx);
    state.update(Action::ExecuteCommand, &fx);
    assert_eq!(state.config.theme, "pico8");
    assert_eq!(state.theme, Theme::by_name("pico8"));

    state.update(Action::OpenCommandPalette, &fx);
    state.update(Action::Paste("quality lossless".into()), &fx);
    state.update(Action::ExecuteCommand, &fx);
    assert_eq!(state.config.quality, AudioQuality::Lossless);
    assert_eq!(fx.ncm.quality(), AudioQuality::Lossless);

    state.update(Action::CancelSettings, &fx);
    assert_eq!(state.config.theme, "pico8");
    assert_eq!(state.config.quality, AudioQuality::Lossless);
    let persisted = std::fs::read_to_string(&fx.config_path).unwrap();
    let persisted: Config = toml::from_str(&persisted).unwrap();
    assert_eq!(persisted.theme, "pico8");
    assert_eq!(persisted.quality, AudioQuality::Lossless);
}

#[tokio::test]
async fn command_feedback_expires_after_being_visible_for_ui_ticks() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());

    state.update(Action::OpenCommandPalette, &fx);
    state.update(Action::Paste("volume 37".into()), &fx);
    state.update(Action::ExecuteCommand, &fx);
    assert!(!state.command_palette.open);
    assert!((state.volume - 0.37).abs() < f32::EPSILON);
    assert!(state.command_feedback.is_some());

    for _ in 0..23 {
        state.update(Action::UiTick, &fx);
    }
    assert!(state.command_feedback.is_some());
    state.update(Action::UiTick, &fx);
    assert!(state.command_feedback.is_none());
}

#[tokio::test]
async fn invalid_command_arguments_keep_the_palette_open_for_correction() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());

    state.update(Action::OpenCommandPalette, &fx);
    state.update(Action::Paste("volume 101".into()), &fx);
    state.update(Action::ExecuteCommand, &fx);

    assert!(state.command_palette.open);
    assert_eq!(state.command_palette.query, "volume 101");
    assert_eq!(state.volume, 1.0);
    assert!(state
        .command_feedback
        .as_deref()
        .is_some_and(|feedback| { feedback.contains("101") && feedback.contains("volume") }));
    assert!(state.command_feedback_error);

    state.update(raw_key(KeyCode::Backspace), &fx);
    assert_eq!(state.command_palette.query, "volume 10");
    assert!(state.command_feedback.is_none());
    assert!(!state.command_feedback_error);

    state.update(Action::ExecuteCommand, &fx);
    assert!(!state.command_palette.open);
    assert!(state.command_feedback.is_some());
    assert!(!state.command_feedback_error);
    state.update(Action::OpenCommandPalette, &fx);
    assert!(state.command_feedback.is_none());
}

#[tokio::test]
async fn spectrum_command_reports_a_persistence_failure_as_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    std::fs::create_dir(&fx.config_path).unwrap();
    let mut state = AppState::new(&Config::default());

    state.update(Action::OpenCommandPalette, &fx);
    state.update(Action::Paste("spectrum".into()), &fx);
    state.update(Action::ExecuteCommand, &fx);

    assert!(!state.command_palette.open);
    assert!(state.command_feedback_error);
    assert!(state
        .command_feedback
        .as_deref()
        .is_some_and(|feedback| feedback.contains(crate::i18n::t(Key::SettingsSaveFailed))));
}

// Regression: promoting a buffered cover used to move the pending
// ThreadProtocol (and its pending-channel sender) into the main slot,
// dropping the main channel's only sender. The dead worker then closed its
// response channel and the event loop exited the whole app on the next poll.
#[test]
fn promoting_a_pending_cover_keeps_the_main_resize_channel_open() {
    let picker = ratatui_image::picker::Picker::halfblocks();
    let (main_tx, mut main_rx) = tokio::sync::mpsc::unbounded_channel();
    let (pending_tx, _pending_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut cover = OriginalCover::buffered(picker, main_tx, pending_tx);
    let image = DynamicImage::new_rgba8(4, 4);

    cover.replace(1, image.clone());
    cover.replace(2, image);
    assert!(cover.has_pending());

    cover.promote_pending();

    assert_eq!(cover.generation, Some(2));
    assert!(matches!(
        main_rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn offline_restore_adopts_the_stored_profile_and_local_library() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    fx.store
        .save_profile(&crate::store::StoredProfile {
            uid: 7,
            nickname: "夜航".into(),
        })
        .unwrap();
    fx.store
        .save(7, "liked", &[crate::store::StoredSong::from(&row(42))])
        .unwrap();
    let mut state = AppState::new(&Config::default());
    let epoch = state.session.begin_restore();

    state.update(
        Action::SessionRestoreFailed {
            epoch,
            failure: RestoreFailure::Offline,
        },
        &fx,
    );

    assert_eq!(state.session.nickname.as_deref(), Some("夜航"));
    assert_eq!(state.library.len(), 1);
    assert_eq!(state.library[0].id, 42);
    assert!(state.liked.contains(&42));
    assert_eq!(
        state.status.as_deref(),
        Some(crate::i18n::t(crate::i18n::Key::OfflineLibrary))
    );
}

#[tokio::test]
async fn offline_restore_without_a_profile_reports_the_network_not_a_logout() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let epoch = state.session.begin_restore();

    state.update(
        Action::SessionRestoreFailed {
            epoch,
            failure: RestoreFailure::Offline,
        },
        &fx,
    );

    assert_eq!(state.session.nickname, None);
    assert_eq!(state.library, AppState::new(&Config::default()).library);
    assert_eq!(
        state.status.as_deref(),
        Some(crate::i18n::t(crate::i18n::Key::NetworkUnavailable))
    );
}

#[tokio::test]
async fn expired_restore_still_logs_the_user_out() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    fx.store
        .save_profile(&crate::store::StoredProfile {
            uid: 7,
            nickname: "夜航".into(),
        })
        .unwrap();
    let mut state = AppState::new(&Config::default());
    let epoch = state.session.begin_restore();

    state.update(
        Action::SessionRestoreFailed {
            epoch,
            failure: RestoreFailure::Expired,
        },
        &fx,
    );

    assert_eq!(state.session.nickname, None);
    assert_eq!(state.library, AppState::new(&Config::default()).library);
    assert_eq!(
        state.status.as_deref(),
        Some(crate::i18n::t(crate::i18n::Key::SessionExpired))
    );
}

#[tokio::test]
async fn bar_cover_follows_the_original_mode_and_playing_generation() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.config.cover_mode = crate::config::CoverMode::Original;
    let (bar_tx, _bar_rx) = tokio::sync::mpsc::unbounded_channel();
    state.bar_original_cover = Some(OriginalCover::new(
        ratatui_image::picker::Picker::halfblocks(),
        bar_tx,
    ));
    state.generation = 3;
    assert!(state.uses_original_cover(CoverSurface::Bar));
    assert!(!state.bar_original_visible());

    state.update(
        Action::CoverDecoded {
            surface: CoverSurface::Bar,
            generation: 3,
            style_revision: state.style_revision,
            image: DynamicImage::new_rgba8(4, 4),
        },
        &fx,
    );
    assert!(state.bar_original_visible());

    // Through a track switch the old art deliberately stays visible, but a
    // stale decode must not be adopted as the current track's art.
    state.generation = 4;
    state.update(
        Action::CoverDecoded {
            surface: CoverSurface::Bar,
            generation: 3,
            style_revision: state.style_revision,
            image: DynamicImage::new_rgba8(4, 4),
        },
        &fx,
    );
    assert!(state.bar_original_visible());
    assert_eq!(
        state.bar_original_cover.as_ref().unwrap().generation,
        Some(3)
    );
}

#[tokio::test]
async fn manual_track_change_disarms_a_parked_fm_advance() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let epoch = state.session.begin_restore();
    state.update(
        Action::SessionRestored {
            epoch,
            uid: 7,
            nickname: "n".into(),
        },
        &fx,
    );
    let stamp = SessionStamp { epoch, uid: 7 };
    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(1);
    state.queue_source = Source::Fm;

    // At the FM tail: the step parks an advance for the incoming batch...
    state.step_queue(&fx, 1, false, false);
    // ...but the user goes back to the previous track before it lands.
    state.step_queue(&fx, -1, false, false);
    assert_eq!(state.queue_pos, Some(0));

    state.update(
        Action::FmMore {
            session: stamp,
            rows: vec![row(3)],
        },
        &fx,
    );

    // The batch extends the queue without yanking playback off the pick.
    assert_eq!(state.queue.len(), 3);
    assert_eq!(state.queue_pos, Some(0));
}

#[tokio::test]
async fn switching_tracks_keeps_the_old_original_cover_until_the_new_one_lands() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.config.cover_mode = crate::config::CoverMode::Original;
    let (main_tx, _main_rx) = tokio::sync::mpsc::unbounded_channel();
    let (pending_tx, _pending_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut cover = OriginalCover::buffered(
        ratatui_image::picker::Picker::halfblocks(),
        main_tx,
        pending_tx,
    );
    cover.replace(1, DynamicImage::new_rgba8(4, 4));
    state.original_cover = Some(cover);
    state.generation = 1;
    assert!(state.playing_original_visible());

    // The next track carries art: the old cover must stay on screen while
    // the new one decodes, instead of flashing the placeholder.
    let mut with_art = row(2);
    with_art.pic_url = Some("https://example.test/next.jpg".into());
    state.play_row(&fx, with_art);
    assert!(state.playing_original_visible());

    // An art-less track has nothing incoming, so the stale art must go.
    let mut bare = row(3);
    bare.pic_url = None;
    state.play_row(&fx, bare);
    assert!(!state.playing_original_visible());
}

#[tokio::test]
async fn a_stale_pending_cover_is_dropped_instead_of_promoting_later() {
    let picker = ratatui_image::picker::Picker::halfblocks();
    let (main_tx, _main_rx) = tokio::sync::mpsc::unbounded_channel();
    let (pending_tx, _pending_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut cover = OriginalCover::buffered(picker, main_tx, pending_tx);
    cover.replace(1, DynamicImage::new_rgba8(4, 4));
    cover.replace(2, DynamicImage::new_rgba8(4, 4));
    assert!(cover.has_pending());

    let backend = TestBackend::new(20, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    // The track moved on to generation 3: the parked generation-2 image
    // must vanish, not resurface over the current track's art.
    terminal
        .draw(|frame| cover.render(frame, Rect::new(0, 0, 10, 5), 3))
        .unwrap();

    assert!(!cover.has_pending());
    assert_eq!(cover.generation, Some(1));
}

#[tokio::test]
async fn a_failed_cover_load_clears_the_previous_tracks_art() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.config.cover_mode = crate::config::CoverMode::Original;
    let (main_tx, _main_rx) = tokio::sync::mpsc::unbounded_channel();
    let (pending_tx, _pending_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut cover = OriginalCover::buffered(
        ratatui_image::picker::Picker::halfblocks(),
        main_tx,
        pending_tx,
    );
    cover.replace(1, DynamicImage::new_rgba8(4, 4));
    state.original_cover = Some(cover);
    state.generation = 2;
    assert!(state.playing_original_visible());

    // A failure for an older generation changes nothing...
    state.update(
        Action::CoverLoadFailed {
            surface: CoverSurface::Playing,
            generation: 1,
        },
        &fx,
    );
    assert!(state.playing_original_visible());

    // ...the current track's failure drops the stale art honestly.
    state.update(
        Action::CoverLoadFailed {
            surface: CoverSurface::Playing,
            generation: 2,
        },
        &fx,
    );
    assert!(!state.playing_original_visible());
}

#[tokio::test]
async fn the_playing_view_render_path_promotes_the_next_tracks_cover() {
    let mut state = AppState::new(&Config::default());
    state.config.cover_mode = crate::config::CoverMode::Original;
    let (main_tx, mut main_rx) = tokio::sync::mpsc::unbounded_channel();
    let (pending_tx, mut pending_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut cover = OriginalCover::buffered(
        ratatui_image::picker::Picker::halfblocks(),
        main_tx,
        pending_tx,
    );
    cover.replace(1, DynamicImage::new_rgba8(4, 4));
    cover.replace(2, DynamicImage::new_rgba8(4, 4));
    state.original_cover = Some(cover);
    state.generation = 2;

    let backend = TestBackend::new(30, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| state.render_original_cover(frame, Rect::new(0, 0, 20, 10)))
        .unwrap();

    // The production render path must drive the pending buffer: encode
    // request out, response applied, parked art promoted.
    let request = pending_rx
        .try_recv()
        .expect("playing render must kick the pending encode");
    let response = request.resize_encode().unwrap();
    state.apply_playing_pending_resize(response);
    let cover = state.original_cover.as_ref().unwrap();
    assert_eq!(cover.generation, Some(2));
    // The handover keeps the encoded stand-in on screen while the main
    // protocol re-encodes in the background...
    assert!(cover.has_pending());

    let backend = TestBackend::new(30, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| state.render_original_cover(frame, Rect::new(0, 0, 20, 10)))
        .unwrap();
    // The first render already queued a main request for the old art;
    // only the newest one matches the handover protocol.
    let mut request = None;
    while let Ok(next) = main_rx.try_recv() {
        request = Some(next);
    }
    let request = request.expect("handover must kick the main re-encode");
    let response = request.resize_encode().unwrap();
    let cover = state.original_cover.as_mut().unwrap();
    assert!(cover.protocol.update_resized_protocol(response));
    cover.finish_handover();
    // ...and retires it the moment the re-encode lands.
    assert!(!state.original_cover.as_ref().unwrap().has_pending());
}

#[tokio::test]
async fn small_source_covers_are_upscaled_to_fill_the_cover_area() {
    let mut bytes = Vec::new();
    DynamicImage::new_rgba8(64, 64)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    let load = CoverLoad {
        request: CoverRenderRequest {
            surface: CoverSurface::Playing,
            generation: 1,
            cells: (64, 32),
            style_revision: 0,
            song_id: 1,
            source_key: "dynamic-upscale-test".into(),
            source_edge: 2_048,
        },
        style: CoverStyle {
            pixel: AppState::new(&Config::default()).pixel_style(),
            original: true,
        },
    };

    let processed = process_cover(None, &load, bytes, false).await.unwrap();
    match processed {
        EitherCover::Original(image) => {
            assert_eq!(image.width().max(image.height()), 2_048);
        }
        EitherCover::Pixel(_) => panic!("original load must decode an image"),
    }
}

#[tokio::test]
async fn resize_worker_reports_a_job_panic_instead_of_hiding_it() {
    let (requests_tx, requests_rx) = mpsc::unbounded_channel();
    let (responses_tx, mut responses_rx) = mpsc::unbounded_channel();
    spawn_resize_worker(requests_rx, responses_tx);

    let mut protocol = ThreadProtocol::new(
        requests_tx,
        Some(Picker::halfblocks().new_resize_protocol(DynamicImage::new_rgba8(4, 4))),
    );
    ratatui_image::ResizeEncodeRender::resize_encode(
        &mut protocol,
        &ratatui_image::Resize::Fit(None),
        (0, 0).into(),
    );

    let response = tokio::time::timeout(Duration::from_secs(1), responses_rx.recv())
        .await
        .expect("worker must report the lost protocol")
        .expect("worker must send its failure");
    let error = match response {
        Err(error) => error,
        Ok(_) => panic!("worker unexpectedly encoded an invalid request"),
    };
    assert!(error.contains("panicked"), "{error}");
}

#[test]
fn a_dead_cover_encoder_degrades_to_pixel_art_instead_of_ending_the_session() {
    // The queue keeps playing: a lost encoder costs the artwork, not the app.
    let mut state = AppState::new(&Config::default());
    let (requests_tx, _requests_rx) = mpsc::unbounded_channel();
    state.original_cover = Some(OriginalCover::new(Picker::halfblocks(), requests_tx));

    degrade_original_covers(&mut state, &anyhow::anyhow!("cover resize worker panicked"));

    assert!(state.original_cover.is_none());
    assert!(state.selected_original_cover.is_none());
    assert!(state.bar_original_cover.is_none());
    assert!(state.command_feedback_error);
    let feedback = state
        .command_feedback
        .as_deref()
        .expect("a visible failure");
    assert!(feedback.contains("panicked"), "{feedback}");
}

#[tokio::test]
async fn repeat_cover_loads_reuse_the_decoded_image() {
    let mut bytes = Vec::new();
    DynamicImage::new_rgba8(64, 64)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    let load = |key: &str| CoverLoad {
        request: CoverRenderRequest {
            surface: CoverSurface::Playing,
            generation: 1,
            cells: (64, 32),
            style_revision: 0,
            song_id: 1,
            source_key: key.into(),
            source_edge: COVER_SOURCE_EDGE,
        },
        style: CoverStyle {
            pixel: AppState::new(&Config::default()).pixel_style(),
            original: true,
        },
    };

    assert!(process_cover(None, &load("lru-test"), bytes, false)
        .await
        .is_some());
    // Garbage bytes with the same key still decode: proof the second load
    // came from the in-memory LRU, not another JPEG decode.
    assert!(
        process_cover(None, &load("lru-test"), b"not an image".to_vec(), false)
            .await
            .is_some()
    );
    // An unknown key with garbage bytes must fail: the LRU is keyed.
    assert!(
        process_cover(None, &load("lru-miss"), b"not an image".to_vec(), false)
            .await
            .is_none()
    );
}
