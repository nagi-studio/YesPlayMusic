//! The main view has two moods: an idle dashboard (pixel art + menu,
//! dashboard-nvim style) and the playing layout (cover · lyrics · progress).

use std::time::Duration;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::MenuEntry;
use crate::api::SearchChannel;
use crate::app::{AppState, SelfUpdate};
use crate::i18n::{self, Key};
use crate::ui::text::display_width;
use crate::ui::{format_duration, Hits};

// Cover art travels as a 26×13 cell grid (each cell is two vertical pixels,
// so the pixel grid is square); the frame must hug exactly that grid or the
// border reads as a stretched rectangle.
const COVER_GRID: (u16, u16) = (26, 13);
pub(crate) const PROGRESS_HEIGHT: u16 = 2;
pub(crate) const MAIN_BREATHER_MIN_HEIGHT: u16 = 12;
const STACKED_SPECTRUM_MIN_HEIGHT: u16 = 4;
const STACKED_SPECTRUM_MAX_HEIGHT: u16 = 8;
const STACKED_MIN_MAIN_HEIGHT: u16 = 18;
const STACKED_MIN_META_HEIGHT: u16 = 8;
const MAX_LYRIC_CONTEXT_ROWS: usize = 9;
/// Columns the side layout keeps to the right of the cover for the
/// meta/lyrics/spectrum panel (gutter included); the cover may not grow
/// into this reserve no matter how tall the terminal is.
pub(crate) const SIDE_PANEL_RESERVED_COLS: u16 = 46;
const SIDE_PANEL_GAP_ROWS: u16 = 1;
const SIDE_PANEL_MIN_SPECTRUM_ROWS: u16 = 4;
/// Lyrics take ~40% of the panel rows left after the meta block (bounded
/// below and above), the spectrum absorbs the rest down to the cover's
/// bottom edge. Tall terminals get more context lines AND a taller band.
const SIDE_PANEL_MIN_LYRIC_ROWS: u16 = 3;
const SIDE_PANEL_MAX_LYRIC_ROWS: u16 = 20;
/// The stacked layout caps its band at 8 rows; the side panel needs a cap of
/// its own or a tall terminal hands it twenty-odd rows. Bars are drawn from
/// the bottom, so the surplus reads as a hole between the lyrics and the
/// spectrum rather than as a taller visualisation.
const SIDE_PANEL_MAX_SPECTRUM_ROWS: u16 = 12;

/// The bottom spectrum band only exists in the stacked layout; the side
/// layout hosts the spectrum inside the right panel, bottom-aligned with
/// the cover.
pub(crate) fn spectrum_band_height(
    total_height: u16,
    enabled: bool,
    layout: crate::app::PlayLayout,
    progress_rows: u16,
) -> u16 {
    if !enabled || matches!(layout, crate::app::PlayLayout::Side) {
        return 0;
    }
    let candidate =
        (total_height / 5).clamp(STACKED_SPECTRUM_MIN_HEIGHT, STACKED_SPECTRUM_MAX_HEIGHT);
    if total_height.saturating_sub(progress_rows + candidate) < STACKED_MIN_MAIN_HEIGHT {
        0
    } else {
        candidate
    }
}

/// Rows the side panel can spare for the spectrum once the meta block and
/// the adaptive lyric window are budgeted. Zero hides the panel spectrum.
fn side_panel_spectrum_rows(state: &AppState, panel_height: u16) -> u16 {
    if !state.config.spectrum_enabled {
        return 0;
    }
    let Some(now) = &state.now else {
        return 0;
    };
    let mut text_rows: u16 = 2;
    if !now.album.is_empty() {
        text_rows += 1;
    }
    if !state.lyrics.is_empty() {
        let available = panel_height.saturating_sub(text_rows + 1 + SIDE_PANEL_GAP_ROWS);
        let mut lyric_rows = (available * 2 / 5)
            .clamp(SIDE_PANEL_MIN_LYRIC_ROWS, SIDE_PANEL_MAX_LYRIC_ROWS)
            .min(available);
        // A configured cap frees its unused budget rows to the spectrum
        // instead of leaving them blank around the shrunken window.
        if let Some(configured) = state.config.lyric_rows {
            lyric_rows = lyric_rows.min(configured);
        }
        text_rows += 1 + lyric_rows;
    } else if state.status.is_some() {
        text_rows += 2;
    }
    let rest = panel_height.saturating_sub(text_rows + SIDE_PANEL_GAP_ROWS);
    if rest < SIDE_PANEL_MIN_SPECTRUM_ROWS {
        0
    } else {
        rest.min(SIDE_PANEL_MAX_SPECTRUM_ROWS)
    }
}

pub fn draw(frame: &mut Frame, state: &mut AppState, area: Rect, hits: &mut Hits) {
    if state.now.is_none() || state.dashboard_hold {
        draw_dashboard(frame, state, area, hits);
        return;
    }

    // Zen strips the last chrome: no progress bar, keys keep working.
    let progress_height = if state.zen { 0 } else { PROGRESS_HEIGHT };
    let spectrum_height = spectrum_band_height(
        area.height,
        state.config.spectrum_enabled,
        state.layout,
        progress_height,
    );
    // One breather row keeps the cover/spectrum off the progress bar
    // whenever the terminal can spare it.
    let breather = u16::from(area.height >= MAIN_BREATHER_MIN_HEIGHT && progress_height > 0);
    let [main, _, spectrum_area, progress_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(breather),
        Constraint::Length(spectrum_height),
        Constraint::Length(progress_height),
    ])
    .areas(area);
    let progress_hits = progress_area;
    let (cover_w, cover_h) = state
        .cover
        .as_ref()
        .or(state.placeholder.as_ref())
        .map(|art| (art.width, art.height))
        .unwrap_or(COVER_GRID);

    match state.layout {
        crate::app::PlayLayout::Side => {
            // The cover fixes the group height; meta, lyrics, and spectrum
            // share the right panel with its bottom edge flush against the
            // cover's, and the whole group centers vertically.
            let group_h = cover_h.min(main.height);
            let group = Rect {
                x: main.x,
                y: main.y + main.height.saturating_sub(group_h) / 2,
                width: main.width,
                height: group_h,
            };
            let [cover_column, _, panel] = Layout::horizontal([
                Constraint::Length(cover_w.min(group.width)),
                Constraint::Length(2),
                Constraint::Min(0),
            ])
            .areas(group);
            draw_cover(frame, state, cover_column);
            let panel_spectrum = side_panel_spectrum_rows(state, panel.height);
            if panel_spectrum > 0 {
                let [text_area, _, panel_spectrum_area] = Layout::vertical([
                    Constraint::Min(0),
                    Constraint::Length(SIDE_PANEL_GAP_ROWS),
                    Constraint::Length(panel_spectrum),
                ])
                .areas(panel);
                draw_meta(frame, state, text_area, false, hits);
                // Reflect draws its waterline at 2/3 of the area. Extending
                // the area below the group by half the band puts the line
                // level with the cover's bottom edge and lets the
                // reflection dip below it — the cover stands at the water.
                let spectrum_area =
                    if state.config.spectrum_style == crate::spectrum::SpectrumKind::Reflect {
                        let below_room = main.bottom().saturating_sub(group.bottom());
                        let extension = (panel_spectrum_area.height / 2).min(below_room);
                        Rect {
                            height: panel_spectrum_area.height + extension,
                            ..panel_spectrum_area
                        }
                    } else {
                        panel_spectrum_area
                    };
                state.spectrum.render(
                    state.config.spectrum_style,
                    state.config.spectrum_glow,
                    state.spectrum_render_options(),
                    spectrum_area,
                    frame.buffer_mut(),
                    &state.theme,
                );
            } else {
                draw_meta(frame, state, panel, false, hits);
            }
        }
        crate::app::PlayLayout::Stacked => {
            let art_height = cover_h.min(
                main.height
                    .saturating_sub(STACKED_MIN_META_HEIGHT.saturating_add(1)),
            );
            let [cover_row, _, meta_area] = Layout::vertical([
                Constraint::Length(art_height),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(main);
            let cover_area = centered(cover_row, cover_w, art_height);
            draw_cover(frame, state, cover_area);
            draw_meta(frame, state, meta_area, true, hits);
        }
    }
    if spectrum_height > 0 {
        state.spectrum.render(
            state.config.spectrum_style,
            state.config.spectrum_glow,
            state.spectrum_render_options(),
            spectrum_area,
            frame.buffer_mut(),
            &state.theme,
        );
    }
    if progress_height > 0 {
        draw_progress(frame, state, progress_hits, hits);
    }
}

// ── idle dashboard ──────────────────────────────────────────────────

fn draw_dashboard(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    let theme = &state.theme;
    let menu = menu_entries(state);
    // One blank row between menu entries so the block can breathe; tight
    // terminals fall back to the compact single-spaced list.
    let airy = area.height >= state.idle_art.height + menu.len() as u16 * 2 + 6;
    let row_stride: u16 = if airy { 2 } else { 1 };
    let menu_height = (menu.len() as u16 - 1) * row_stride + 1;
    let art_height = state
        .idle_art
        .height
        .min(area.height.saturating_sub(menu_height + 5));

    let [_, art_area, _, menu_area, _, hint_area, _] = Layout::vertical([
        Constraint::Fill(2),
        Constraint::Length(art_height),
        Constraint::Length(2),
        Constraint::Length(menu_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(3),
    ])
    .areas(area);

    let art_rect = centered(art_area, state.idle_art.width, art_height);
    // The intro overlay animates the logo into exactly this rect; drawing
    // the finished art underneath would leak through transparent margins.
    hits.idle_art = Some(art_rect);
    if state.intro_preview.is_none() {
        frame.render_widget(&state.idle_art, art_rect);
    }

    // The reserved hint row carries the build's own version until an
    // update exists, which is the more useful thing to show there.
    let hint = match (&state.self_update, &state.update_available) {
        (SelfUpdate::Running(line), _) => line.clone(),
        (SelfUpdate::Installed, _) => i18n::t_update_restart_now().to_owned(),
        (_, Some(version)) => i18n::t_update_available(version),
        _ => format!("ypm v{}", env!("CARGO_PKG_VERSION")),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, Style::new().fg(theme.dim)))).centered(),
        hint_area,
    );

    const MENU_WIDTH: u16 = 30;
    for (index, (label, key, entry)) in menu.iter().enumerate() {
        let row = Rect {
            x: area.x + (area.width.saturating_sub(MENU_WIDTH)) / 2,
            y: menu_area.y + index as u16 * row_stride,
            width: MENU_WIDTH.min(area.width),
            height: 1,
        };
        hits.menu.push((row, *entry));
        let pad =
            (MENU_WIDTH as usize).saturating_sub(display_width(label) + display_width(key) + 2);
        let line = Line::from(vec![
            Span::styled(
                format!(" {label}{}", " ".repeat(pad)),
                Style::new().fg(theme.fg),
            ),
            Span::styled((*key).to_owned(), Style::new().fg(theme.accent)),
            Span::raw(" "),
        ]);
        frame.render_widget(Paragraph::new(line), row);
    }
}

fn menu_entries(state: &AppState) -> Vec<(String, &'static str, MenuEntry)> {
    let mut entries = vec![(i18n::t(Key::LikedSongs).to_owned(), "2", MenuEntry::Library)];
    entries.push((i18n::t(Key::Search).to_owned(), "3", MenuEntry::Search));
    entries.push(match &state.session.nickname {
        Some(_) => (i18n::t(Key::Relogin).to_owned(), "", MenuEntry::Login),
        None => (i18n::t(Key::ScanLogin).to_owned(), "", MenuEntry::Login),
    });
    entries.push((i18n::t(Key::Settings).to_owned(), ",", MenuEntry::Settings));
    entries.push((i18n::t(Key::Quit).to_owned(), "q", MenuEntry::Quit));
    entries
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

// ── playing layout ──────────────────────────────────────────────────

fn draw_cover(frame: &mut Frame, state: &mut AppState, area: Rect) {
    if area.height == 0 {
        return;
    }
    if state.playing_original_visible() {
        frame.render_widget(
            ratatui::widgets::Block::new().style(Style::new().bg(state.theme.bg)),
            area,
        );
        state.render_original_cover(frame, area);
        return;
    }
    match (&state.cover, &state.placeholder) {
        (Some(cover), _) => frame.render_widget(cover, area),
        // Cover still loading: idle art pre-rendered at cover size, so
        // the swap keeps position and scale.
        (None, Some(placeholder)) => frame.render_widget(placeholder, area),
        (None, None) => frame.render_widget(&state.idle_art, area),
    }
}

/// Screen rect of a clickable run inside one meta line. `line_width` is the
/// whole line, because a centred Paragraph offsets by it; `prefix_width` is
/// what sits left of the run. Returns `None` when the run is off the bottom
/// of the area or has no visible columns left — either way there is nothing
/// on screen to click.
pub(crate) fn meta_link_rect(
    area: Rect,
    index: usize,
    centered: bool,
    line_width: usize,
    prefix_width: usize,
    run_width: usize,
) -> Option<Rect> {
    let row = u16::try_from(index).ok()?;
    if row >= area.height || run_width == 0 {
        return None;
    }
    // Ratatui centres by the rendered line width, and does not centre a line
    // that already overflows the area.
    let offset = if centered {
        usize::from(area.width).saturating_sub(line_width) / 2
    } else {
        0
    };
    let start = u16::try_from(offset + prefix_width).ok()?;
    let width = u16::try_from(run_width)
        .ok()?
        .min(area.width.saturating_sub(start));
    (width > 0).then_some(Rect {
        x: area.x + start,
        y: area.y + row,
        width,
        height: 1,
    })
}

fn draw_meta(
    frame: &mut Frame,
    state: &mut AppState,
    area: Rect,
    centered_text: bool,
    hits: &mut Hits,
) {
    let theme = state.theme;
    let indent = if centered_text { "" } else { "  " };
    let mut lines = Vec::new();
    if centered_text {
        lines.push(Line::default());
    }
    if let Some(now) = &state.now {
        let title_color = if state.config.title_accent {
            theme.accent
        } else {
            theme.fg
        };
        lines.push(Line::from(Span::styled(
            format!("{indent}{}", now.title),
            Style::new().fg(title_color).add_modifier(Modifier::BOLD),
        )));
        // The FM badge rides the artist line: no extra row, but the queue's
        // origin stays visible while a station track plays.
        let mut artist = vec![Span::styled(
            format!("{indent}{} ", now.artist),
            Style::new().fg(theme.dim),
        )];
        if state.playing_personal_fm() {
            artist.push(Span::styled(
                i18n::t(Key::PersonalFm),
                Style::new().fg(theme.accent),
            ));
        }
        // Registered before the push, so the index is the row this line lands
        // on: a Paragraph renders line N at area.y + N.
        if let Some(id) = now.artist_id.filter(|_| !now.artist.is_empty()) {
            let line_width: usize = artist.iter().map(|span| display_width(&span.content)).sum();
            if let Some(rect) = meta_link_rect(
                area,
                lines.len(),
                centered_text,
                line_width,
                display_width(indent),
                display_width(&now.artist),
            ) {
                hits.links.push((
                    rect,
                    Hits::link(SearchChannel::Artists, id, now.artist.clone()),
                ));
            }
        }
        lines.push(Line::from(artist));
        if !now.album.is_empty() {
            if let Some(id) = now.album_id {
                let album_width = display_width(&now.album);
                if let Some(rect) = meta_link_rect(
                    area,
                    lines.len(),
                    centered_text,
                    display_width(indent) + album_width,
                    display_width(indent),
                    album_width,
                ) {
                    hits.links.push((
                        rect,
                        Hits::link(SearchChannel::Albums, id, now.album.clone()),
                    ));
                }
            }
            lines.push(Line::from(Span::styled(
                format!("{indent}{}", now.album),
                Style::new().fg(theme.dim),
            )));
        }
        if !state.lyrics.is_empty() {
            lines.push(Line::default());
            let reserved = lines.len() as u16;
            lines.extend(lyric_window(state, area.height.saturating_sub(reserved)));
        } else if let Some(status) = &state.status {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                format!("{indent}{status}"),
                Style::new().fg(theme.dim),
            )));
        }
    }
    let paragraph = if centered_text {
        Paragraph::new(lines).centered()
    } else {
        Paragraph::new(lines)
    };
    frame.render_widget(paragraph, area);
}

/// Rows of synced lyrics with the current pair pinned mid-window.
fn lyric_window(state: &AppState, height: u16) -> Vec<Line<'static>> {
    if height == 0 || state.lyrics.is_empty() {
        return Vec::new();
    }
    let current = current_lyric_index(&state.lyrics, state.position);
    let rows = usize::from(height);
    let anchor = current.unwrap_or(0);
    let current_cost = lyric_display_rows(&state.lyrics[anchor]);
    let mut window_cap = MAX_LYRIC_CONTEXT_ROWS * 2 + current_cost;
    if let Some(configured) = state.config.lyric_rows {
        window_cap = window_cap.min(usize::from(configured));
    }
    let window_rows = rows.min(window_cap);
    let target_above = ((window_rows.saturating_sub(current_cost)) / 2).min(MAX_LYRIC_CONTEXT_ROWS);
    let target_below = window_rows
        .saturating_sub(target_above + current_cost)
        .min(MAX_LYRIC_CONTEXT_ROWS);

    let mut start = anchor;
    let mut used_above = 0_usize;
    while start > 0 {
        let previous = start - 1;
        let cost = lyric_display_rows(&state.lyrics[previous]);
        if used_above + cost > target_above {
            break;
        }
        used_above += cost;
        start = previous;
    }

    let mut end = anchor + 1;
    let mut used_below = 0_usize;
    while end < state.lyrics.len() {
        let cost = lyric_display_rows(&state.lyrics[end]);
        if used_below + cost > target_below {
            break;
        }
        used_below += cost;
        end += 1;
    }

    let window_top = rows.saturating_sub(window_rows) / 2;
    let leading = window_top + target_above.saturating_sub(used_above);
    let mut lines = vec![Line::default(); leading];
    for (index, lyric) in state.lyrics.iter().enumerate().take(end).skip(start) {
        let is_current = Some(index) == current;
        lines.extend(lyric_pair_lines(state, lyric, is_current));
    }
    lines.truncate(rows);
    lines
}

fn lyric_display_rows(lyric: &crate::lyrics::LyricLine) -> usize {
    1 + usize::from(
        lyric
            .translation
            .as_ref()
            .is_some_and(|translation| !translation.is_empty()),
    )
}

fn lyric_pair_lines(
    state: &AppState,
    lyric: &crate::lyrics::LyricLine,
    is_current: bool,
) -> Vec<Line<'static>> {
    let theme = &state.theme;
    let original_style = if is_current {
        Style::new().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.dim)
    };
    let marker_style = if is_current {
        Style::new().fg(theme.accent)
    } else {
        original_style
    };
    let mut original = vec![Span::styled(
        if is_current { "▎ " } else { "  " },
        marker_style,
    )];
    if is_current {
        original.extend(current_lyric_spans(lyric, state.position, theme));
    } else {
        original.push(Span::styled(lyric.text.clone(), original_style));
    }
    let mut lines = vec![Line::from(original)];
    if let Some(translation) = lyric
        .translation
        .as_ref()
        .filter(|translation| !translation.is_empty())
    {
        let translation_style = if is_current {
            Style::new().fg(theme.fg)
        } else {
            Style::new().fg(theme.faint)
        }
        .add_modifier(Modifier::ITALIC);
        let marker_style = if is_current {
            Style::new().fg(theme.accent)
        } else {
            translation_style
        };
        lines.push(Line::from(vec![
            Span::styled(if is_current { "▎ " } else { "  " }, marker_style),
            Span::styled(translation.clone(), translation_style),
        ]));
    }
    lines
}

/// Prefer an upcoming YRC line as soon as its word timeline starts, while
/// never letting late word data move the display back behind the LRC line.
fn current_lyric_index(lyrics: &[crate::lyrics::LyricLine], position: Duration) -> Option<usize> {
    let line_index = crate::lyrics::line_index_at(lyrics, position);
    let word_index = lyrics
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.word_timing
                .as_ref()
                .is_some_and(|timing| timing.start <= position)
        })
        .map(|(index, _)| index)
        .next_back();
    match (line_index, word_index) {
        (Some(line), Some(word)) => Some(line.max(word)),
        (line, word) => line.or(word),
    }
}

/// Color a YRC line by half-open word intervals. A zero-duration token is
/// treated as sung at its timestamp and never flashes as the current word.
fn current_lyric_spans(
    lyric: &crate::lyrics::LyricLine,
    position: Duration,
    theme: &crate::theme::Theme,
) -> Vec<Span<'static>> {
    let Some(timing) = lyric
        .word_timing
        .as_ref()
        .filter(|timing| !timing.words.is_empty())
    else {
        return vec![Span::styled(
            lyric.text.clone(),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        )];
    };

    timing
        .words
        .iter()
        .map(|word| {
            let end = word.start.saturating_add(word.duration);
            let style = if position < word.start {
                Style::new().fg(theme.fg)
            } else if !word.duration.is_zero() && position < end {
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme.accent)
            };
            Span::styled(word.text.clone(), style)
        })
        .collect()
}

/// Bottom strip for the browsing views. Short terminals get two content
/// rows (title + controls); tall ones add the mini cover and the current
/// lyric pair. The last row always stays blank so the bar never touches
/// the footer hints.
pub(super) const PLAYER_BAR_HEIGHT: u16 = 3;
pub(super) const PLAYER_BAR_EXPANDED_HEIGHT: u16 = 9;
const PLAYER_BAR_EXPANDED_MAX_HEIGHT: u16 = 14;
const PLAYER_BAR_EXPAND_MIN_TERMINAL_HEIGHT: u16 = 26;

/// The bar grows with the terminal: a quarter of the rows, between the
/// classic 9 and a generous 14, so big windows get a bigger cover too.
pub(super) fn player_bar_height(total_height: u16) -> u16 {
    if total_height >= PLAYER_BAR_EXPAND_MIN_TERMINAL_HEIGHT {
        (total_height / 4).clamp(PLAYER_BAR_EXPANDED_HEIGHT, PLAYER_BAR_EXPANDED_MAX_HEIGHT)
    } else {
        PLAYER_BAR_HEIGHT
    }
}

/// Rows kept clear between the bar's baseline and the footer hints; taller
/// bars can afford a wider breather so the controls don't crowd the keys.
fn player_bar_gap(bar_height: u16) -> u16 {
    if bar_height >= 11 {
        2
    } else {
        1
    }
}

/// Cover cells for the bar at this terminal height: the art fills the bar's
/// content rows, twice as wide as tall for a square half-block pixel grid.
pub(crate) fn player_bar_cover_cells(total_height: u16) -> (u16, u16) {
    let bar = player_bar_height(total_height);
    let rows = bar.saturating_sub(player_bar_gap(bar));
    (rows.saturating_mul(2), rows)
}

pub(super) fn draw_player_bar(
    frame: &mut Frame,
    state: &mut AppState,
    area: Rect,
    hits: &mut Hits,
) {
    if state.now.is_none() || area.height < PLAYER_BAR_HEIGHT || area.width < 8 {
        return;
    }
    let theme = state.theme;
    let expanded = area.height >= PLAYER_BAR_EXPANDED_HEIGHT;
    // The last row(s) are the breathing gap above the footer hints.
    let content = Rect {
        height: area.height - player_bar_gap(area.height),
        ..area
    };
    // The cover fills the bar's content rows; it only appears when the row
    // also leaves the text column usable width, otherwise it would overlap
    // the title and controls.
    let cover_cells = (content.height.saturating_mul(2), content.height);
    // The panels above are airy line borders, so sitting on the content
    // edge suits them; a solid block of art on that same column reads as
    // pressed against the screen. Inset it to the panels' inner text column:
    // one border column plus one padding column. (Numerically equal to
    // PANEL_GAP_X, but that names the gap BETWEEN panels — a coincidence,
    // not a shared meaning.)
    const BAR_COVER_INSET: u16 = 2;
    let cover_inset = BAR_COVER_INSET;
    let cover_slot = cover_inset + cover_cells.0 + 2;
    const MIN_TEXT_COLUMN: u16 = 24;
    let (cover_area, right) = if expanded && content.width >= cover_slot + MIN_TEXT_COLUMN {
        (
            Some(Rect {
                x: content.x + cover_inset,
                width: cover_cells.0,
                height: cover_cells.1,
                ..content
            }),
            Rect {
                x: content.x + cover_slot,
                width: content.width - cover_slot,
                ..content
            },
        )
    } else {
        (None, content)
    };
    if let Some(cover_area) = cover_area {
        // Same policy as the now-playing view: terminal-graphics original when
        // it is ready for this track, pixel art otherwise.
        if state.bar_original_visible() {
            state.render_bar_original(frame, cover_area);
        } else if let Some(cover) = &state.bar_cover {
            frame.render_widget(cover, cover_area);
        }
    }

    let Some(now) = &state.now else { return };
    let note = "♪ ";
    let text_width = usize::from(right.width).saturating_sub(display_width(note) + 2);
    let title = crate::ui::text::pad_or_marquee(
        &format!("{} — {}", now.title, now.artist),
        text_width,
        false,
        0,
    );
    // Two fixed lyric rows keep the layout stable while lines change; the
    // translation row simply stays blank when a line has none.
    let lyric_rows: u16 = if expanded && !state.lyrics.is_empty() {
        2
    } else {
        0
    };
    // The progress row is pinned to the baseline (the cover's last row);
    // the title/lyric block centers in the space above it, so the bar has
    // one clean bottom edge without going bottom-heavy.
    let (title_y, progress_y) = if expanded {
        let text_rows = 1 + lyric_rows;
        let anchor = cover_area.map_or(content.height, |cover| cover.height);
        let above = anchor.saturating_sub(1);
        let top = above.saturating_sub(text_rows) / 2;
        (right.y + top, right.y + anchor.max(text_rows + 1) - 1)
    } else {
        (right.y, right.y + 1)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(note, Style::new().fg(theme.accent)),
            Span::styled(title, Style::new().fg(theme.dim)),
        ])),
        Rect {
            y: title_y,
            height: 1,
            ..right
        },
    );

    if lyric_rows > 0 {
        if let Some(index) = current_lyric_index(&state.lyrics, state.position) {
            if let Some(lyric) = state.lyrics.get(index) {
                let mut spans = vec![Span::raw("   ")];
                spans.extend(current_lyric_spans(lyric, state.position, &theme));
                frame.render_widget(
                    Paragraph::new(Line::from(spans)),
                    Rect {
                        y: title_y + 1,
                        height: 1,
                        ..right
                    },
                );
                if let Some(translation) = &lyric.translation {
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::raw("   "),
                            Span::styled(translation.clone(), Style::new().fg(theme.dim)),
                        ])),
                        Rect {
                            y: title_y + 2,
                            height: 1,
                            ..right
                        },
                    );
                }
            }
        }
    }

    let progress = Rect {
        y: progress_y,
        height: 1,
        ..right
    };
    draw_progress(frame, state, progress, hits);
}

/// Register a mouse target only where it was actually drawn: on narrow
/// bars the span math can spill past the row, and an unclipped rect would
/// catch clicks outside the player area.
fn push_clipped(targets: &mut Vec<(Rect, ())>, area: Rect, rect: Rect) {
    let clipped = rect.intersection(area);
    if !clipped.is_empty() {
        targets.push((clipped, ()));
    }
}

fn draw_progress(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    let theme = &state.theme;
    let icons = crate::icons::for_style(state.config.icons);
    let play_icon = if state.paused {
        icons.play
    } else {
        icons.pause
    };
    let mode = crate::app::PlaybackModeSlot::from_parts(state.shuffle, state.play_mode);
    let mode_icon = match mode {
        crate::app::PlaybackModeSlot::Sequential => icons.sequential,
        crate::app::PlaybackModeSlot::RepeatList => icons.repeat_list,
        crate::app::PlaybackModeSlot::RepeatOne => icons.repeat_one,
        crate::app::PlaybackModeSlot::Shuffle => icons.shuffle,
    };
    let liked = state
        .current_track_id
        .map(|id| state.liked.contains(&id))
        .unwrap_or(false);
    let elapsed = format_duration(state.position);
    let total = state
        .duration
        .map(format_duration)
        .unwrap_or_else(|| "--:--".into());

    let play_slot_width = display_width(icons.play).max(display_width(icons.pause));
    let heart_slot_width = display_width(icons.heart);
    let mode_slot_width = [
        icons.sequential,
        icons.repeat_list,
        icons.repeat_one,
        icons.shuffle,
    ]
    .into_iter()
    .map(display_width)
    .max()
    .unwrap_or(1);
    const VOLUME_CELLS: usize = 10;
    // Eleven literal spaces separate the row's segments (see the span
    // assembly below) plus one trailing breathing column: the meter ends
    // flush with the content edge instead of leaving a dead right margin.
    let fixed = play_slot_width
        + display_width(&elapsed)
        + display_width(&total)
        + heart_slot_width
        + mode_slot_width
        + display_width(icons.volume_high)
        + VOLUME_CELLS
        + 12;
    let bar_width = (area.width as usize).saturating_sub(fixed).max(8);
    let ratio = match state.duration {
        Some(duration) if !duration.is_zero() => {
            (state.position.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    let head = ((bar_width as f64 - 1.0) * ratio).round() as usize;

    let bar_spans = if state.thick_progress {
        let filled = ((bar_width as f64) * ratio).round() as usize;
        vec![
            Span::styled("█".repeat(filled), Style::new().fg(theme.accent)),
            Span::styled(
                "▁".repeat(bar_width.saturating_sub(filled)),
                Style::new().fg(theme.faint),
            ),
        ]
    } else {
        vec![
            Span::styled("━".repeat(head), Style::new().fg(theme.accent)),
            Span::styled("●", Style::new().fg(theme.accent)),
            Span::styled(
                "─".repeat(bar_width.saturating_sub(head + 1)),
                Style::new().fg(theme.faint),
            ),
        ]
    };
    push_clipped(
        &mut hits.play,
        area,
        Rect {
            x: area.x + 1,
            y: area.y,
            width: display_width(play_icon) as u16,
            height: 1,
        },
    );
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(play_icon, Style::new().fg(theme.fg)),
        Span::raw(" ".repeat(play_slot_width.saturating_sub(display_width(play_icon)) + 1)),
        Span::styled(elapsed, Style::new().fg(theme.faint)),
        Span::raw(" "),
    ];
    let bar_x = area.x
        + spans
            .iter()
            .map(|span| display_width(&span.content) as u16)
            .sum::<u16>();
    push_clipped(
        &mut hits.progress,
        area,
        Rect {
            x: bar_x,
            y: area.y,
            width: bar_width as u16,
            height: 1,
        },
    );
    spans.extend(bar_spans);
    spans.push(Span::raw(" "));
    spans.push(Span::styled(total, Style::new().fg(theme.faint)));
    // Clickable heart: its cell position is the rendered width so far + 2.
    let heart_x = spans
        .iter()
        .map(|span| display_width(&span.content) as u16)
        .sum::<u16>()
        + area.x
        + 2;
    push_clipped(
        &mut hits.heart,
        area,
        Rect {
            x: heart_x,
            y: area.y,
            width: display_width(icons.heart) as u16,
            height: 1,
        },
    );
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        icons.heart,
        Style::new().fg(if liked { theme.accent2 } else { theme.faint }),
    ));
    spans.push(Span::raw("  "));
    let mode_x = area.x
        + spans
            .iter()
            .map(|span| display_width(&span.content) as u16)
            .sum::<u16>();
    push_clipped(
        &mut hits.playback_mode,
        area,
        Rect {
            x: mode_x,
            y: area.y,
            width: display_width(mode_icon) as u16,
            height: 1,
        },
    );
    spans.push(Span::styled(
        mode_icon,
        Style::new().fg(if mode == crate::app::PlaybackModeSlot::Sequential {
            theme.faint
        } else {
            theme.accent
        }),
    ));
    spans.push(Span::raw(
        " ".repeat(mode_slot_width.saturating_sub(display_width(mode_icon))),
    ));
    // Click or drag across the dot meter to set the volume.
    let filled = (state.volume.clamp(0.0, 1.0) * VOLUME_CELLS as f32).round() as usize;
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        icons.volume_at(state.volume),
        Style::new().fg(theme.faint),
    ));
    spans.push(Span::raw(" "));
    let volume_x = spans
        .iter()
        .map(|span| display_width(&span.content) as u16)
        .sum::<u16>()
        + area.x;
    push_clipped(
        &mut hits.volume,
        area,
        Rect {
            x: volume_x,
            y: area.y,
            width: VOLUME_CELLS as u16,
            height: 1,
        },
    );
    spans.push(Span::styled(
        icons.volume_full.repeat(filled),
        Style::new().fg(theme.dim),
    ));
    spans.push(Span::styled(
        icons.volume_empty.repeat(VOLUME_CELLS - filled),
        Style::new().fg(theme.faint),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;
    use ratatui::Terminal;

    use super::{
        current_lyric_index, current_lyric_spans, draw_meta, draw_progress, lyric_window,
        menu_entries, spectrum_band_height, PROGRESS_HEIGHT,
    };
    use crate::action::{Action, MenuEntry};
    use crate::app::{AppState, NowPlaying, PlayLayout, PlayMode};
    use crate::config::Config;
    use crate::event;
    use crate::lyrics::LyricLine;
    use crate::spectrum::{SampleBuffer, SpectrumKind};
    use crate::ui::Hits;
    use crate::yrc::{YrcLine, YrcWord};

    fn rect_text(buffer: &Buffer, rect: Rect) -> String {
        (rect.x..rect.right())
            .map(|x| buffer[(x, rect.y)].symbol())
            .collect()
    }

    fn buffer_text(buffer: &Buffer) -> String {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn player_bar_cover_keeps_a_breathing_column_off_the_content_edge() {
        // The body panels above are airy line borders sitting on the content
        // edge; a solid block of art on that same column reads as pressed
        // against the screen. The bar insets the cover to the panels' inner
        // text column instead.
        let config = Config::default();
        let mut state = AppState::new(&config);
        state.now = Some(NowPlaying {
            title: "雨とカプチーノ".into(),
            artist: "ヨルシカ".into(),
            album: String::new(),
            artist_id: None,
            album_id: None,
        });
        let area = Rect::new(0, 0, 90, 10);
        state.bar_cover = Some(crate::pixel::vinyl(
            state.theme.palette,
            state.theme.bg,
            (area.height - 1) * 2,
            area.height - 1,
            crate::pixel::CoverDetail::Half,
        ));
        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();
        terminal
            .draw(|frame| super::draw_player_bar(frame, &mut state, area, &mut hits))
            .unwrap();
        let buffer = terminal.backend().buffer();
        // Border + padding of the bordered panels above: their text starts
        // two columns in, and the cover aligns with that text.
        let theme = AppState::new(&Config::default()).theme;
        let panel_text_x = crate::ui::panel_block(&theme, "t", None)
            .inner(Rect::new(0, 0, 20, 5))
            .x;
        let inked = |x: u16| (0..area.height).any(|y| buffer[(x, y)].symbol() != " ");
        for x in 0..panel_text_x {
            assert!(!inked(x), "column {x} must stay clear of the cover");
        }
        assert!(
            inked(panel_text_x),
            "the cover must start right at the panels' text column"
        );
    }

    fn click(rect: Rect) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn the_idle_dashboard_shows_the_build_version_until_an_update_exists() {
        let mut state = AppState::new(&Config::default());
        let mut hits = Hits::default();
        let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();

        terminal
            .draw(|frame| super::draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();
        let idle = buffer_text(terminal.backend().buffer());
        assert!(
            idle.contains(&format!("ypm v{}", env!("CARGO_PKG_VERSION"))),
            "idle dashboard shows its own version"
        );

        // An available update owns the row instead: it is the more useful
        // thing to read there, and it names the newer version anyway.
        state.update_available = Some("v9.9.9".into());
        terminal
            .draw(|frame| super::draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();
        let updating = buffer_text(terminal.backend().buffer());
        assert!(updating.contains("9.9.9"));
        assert!(!updating.contains(&format!("ypm v{}", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn lyrics_use_four_distinct_original_and_translation_styles() {
        let mut state = AppState::new(&Config::default());
        state.position = Duration::from_secs(1);
        state.lyrics = vec![
            LyricLine {
                time: Duration::ZERO,
                text: "Current".into(),
                translation: Some("当前翻译".into()),
                word_timing: None,
            },
            LyricLine {
                time: Duration::from_secs(10),
                text: "Context".into(),
                translation: Some("上下文翻译".into()),
                word_timing: None,
            },
        ];

        let lines = lyric_window(&state, 6);
        assert_eq!(lines[2].spans[0].content, "▎ ");
        assert_eq!(lines[2].spans[0].style.fg, Some(state.theme.accent));
        assert_eq!(lines[2].spans[1].style.fg, Some(state.theme.fg));
        assert!(lines[2].spans[1]
            .style
            .add_modifier
            .contains(Modifier::BOLD));

        assert_eq!(lines[3].spans[0].content, "▎ ");
        assert_eq!(lines[3].spans[0].style.fg, Some(state.theme.accent));
        assert_eq!(lines[3].spans[1].style.fg, Some(state.theme.fg));
        assert!(lines[3].spans[1]
            .style
            .add_modifier
            .contains(Modifier::ITALIC));

        assert_eq!(lines[4].spans[1].style.fg, Some(state.theme.dim));
        assert!(!lines[4].spans[1]
            .style
            .add_modifier
            .contains(Modifier::ITALIC));
        assert_eq!(lines[5].spans[1].style.fg, Some(state.theme.faint));
        assert!(lines[5].spans[1]
            .style
            .add_modifier
            .contains(Modifier::ITALIC));
    }

    #[test]
    fn lyric_context_is_capped_and_the_current_line_stays_centered() {
        let mut state = AppState::new(&Config::default());
        state.position = Duration::from_secs(15);
        state.lyrics = (0..31)
            .map(|index| LyricLine {
                time: Duration::from_secs(index),
                text: format!("Line {index}"),
                translation: None,
                word_timing: None,
            })
            .collect();

        let lines = lyric_window(&state, 31);
        assert_eq!(lines.len(), 25, "six leading rows plus a 19-row window");
        let current = lines
            .iter()
            .position(|line| line.spans.first().is_some_and(|span| span.content == "▎ "))
            .unwrap();
        assert_eq!(current, 15);
        assert_eq!(
            lines
                .iter()
                .filter(|line| line
                    .spans
                    .get(1)
                    .is_some_and(|span| span.content.starts_with("Line")))
                .count(),
            19
        );
    }

    #[test]
    fn configured_lyric_rows_shrink_the_window_and_keep_it_centered() {
        let mut state = AppState::new(&Config::default());
        state.config.lyric_rows = Some(5);
        state.position = Duration::from_secs(15);
        state.lyrics = (0..31)
            .map(|index| LyricLine {
                time: Duration::from_secs(index),
                text: format!("Line {index}"),
                translation: None,
                word_timing: None,
            })
            .collect();

        let lines = lyric_window(&state, 31);
        let lyric_rows: Vec<&str> = lines
            .iter()
            .filter_map(|line| line.spans.get(1).map(|span| span.content.as_ref()))
            .filter(|content| content.starts_with("Line"))
            .collect();
        assert_eq!(lyric_rows.len(), 5, "the cap wins over the panel height");
        assert_eq!(lyric_rows.first().copied(), Some("Line 13"));
        let current = lines
            .iter()
            .position(|line| line.spans.first().is_some_and(|span| span.content == "▎ "))
            .unwrap();
        assert_eq!(
            current, 15,
            "the window floats mid-panel, current mid-window"
        );
    }

    fn mirror_rows(buffer: &Buffer, range: std::ops::Range<u16>, height: u16) -> Vec<u16> {
        (0..height)
            .filter(|y| range.clone().all(|x| buffer[(x, *y)].symbol() == "─"))
            .collect()
    }

    #[test]
    fn bottom_spectrum_band_is_stacked_only() {
        assert_eq!(
            spectrum_band_height(20, true, PlayLayout::Side, PROGRESS_HEIGHT),
            0
        );
        assert_eq!(
            spectrum_band_height(56, true, PlayLayout::Side, PROGRESS_HEIGHT),
            0
        );
        assert_eq!(
            spectrum_band_height(20, true, PlayLayout::Stacked, PROGRESS_HEIGHT),
            0
        );
        assert_eq!(
            spectrum_band_height(36, true, PlayLayout::Stacked, PROGRESS_HEIGHT),
            7
        );
        // Zen frees the progress rows, letting tight terminals keep the band.
        assert_eq!(spectrum_band_height(22, true, PlayLayout::Stacked, 0), 4);
        assert_eq!(
            spectrum_band_height(22, true, PlayLayout::Stacked, PROGRESS_HEIGHT),
            0
        );
        assert_eq!(
            spectrum_band_height(56, true, PlayLayout::Stacked, PROGRESS_HEIGHT),
            8
        );
        assert_eq!(
            spectrum_band_height(56, false, PlayLayout::Stacked, PROGRESS_HEIGHT),
            0
        );

        let config = Config {
            spectrum_enabled: true,
            spectrum_style: SpectrumKind::Reflect,
            ..Config::default()
        };
        let mut state = AppState::new(&config);
        state.layout = PlayLayout::Stacked;
        state.now = Some(NowPlaying {
            title: "Title".into(),
            artist: "Artist".into(),
            album: String::new(),
            artist_id: None,
            album_id: None,
        });
        state
            .spectrum
            .tick(&SampleBuffer::default(), false, true, true, false, true);
        let backend = TestBackend::new(80, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| super::draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        let rows = mirror_rows(terminal.backend().buffer(), 0..80, 40);
        assert_eq!(rows.len(), 1, "one full-width reflect mirror line");
        let band_top = 40
            - PROGRESS_HEIGHT
            - spectrum_band_height(40, true, PlayLayout::Stacked, PROGRESS_HEIGHT);
        assert!(rows[0] >= band_top && rows[0] < 40 - PROGRESS_HEIGHT);
    }

    #[test]
    fn side_layout_spectrum_lives_in_the_panel_and_ends_with_the_cover() {
        let config = Config {
            spectrum_enabled: true,
            spectrum_style: SpectrumKind::Reflect,
            ..Config::default()
        };
        let mut state = AppState::new(&config);
        state.layout = PlayLayout::Side;
        state.now = Some(NowPlaying {
            title: "Title".into(),
            artist: "Artist".into(),
            album: String::new(),
            artist_id: None,
            album_id: None,
        });
        state
            .spectrum
            .tick(&SampleBuffer::default(), false, true, true, false, true);
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| super::draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        // Without a rendered cover the placeholder grid is 26 columns wide,
        // so the panel starts at column 28.
        let buffer = terminal.backend().buffer();
        let panel_rows = mirror_rows(buffer, 28..120, 40);
        assert_eq!(panel_rows.len(), 1, "the mirror line spans only the panel");
        assert!(mirror_rows(buffer, 0..120, 40).is_empty());

        // The group is vertically centered: main = 38 rows, cover grid = 13.
        // The reflect waterline sits level with the cover's bottom edge and
        // the reflection dips below it.
        let group_top = (38 - 13) / 2;
        assert_eq!(panel_rows[0], group_top + 13);
    }

    #[test]
    fn wide_and_zen_spectrum_stays_inside_the_centered_110_column_canvas() {
        for zen in [false, true] {
            let config = Config {
                spectrum_enabled: true,
                spectrum_style: SpectrumKind::Reflect,
                ..Config::default()
            };
            let mut state = AppState::new(&config);
            state.zen = zen;
            state.now = Some(NowPlaying {
                title: "Title".into(),
                artist: "Artist".into(),
                album: String::new(),
                artist_id: None,
                album_id: None,
            });
            state
                .spectrum
                .tick(&SampleBuffer::default(), false, true, true, false, true);
            let backend = TestBackend::new(200, 60);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = Hits::default();

            terminal
                .draw(|frame| crate::ui::draw(frame, &mut state, &mut hits))
                .unwrap();

            // Canvas spans columns 45..155; the placeholder cover grid is 26
            // wide, so the panel spectrum lives in 73..155.
            let buffer = terminal.backend().buffer();
            let rows = mirror_rows(buffer, 73..155, 60);
            assert_eq!(rows.len(), 1, "zen={zen}: one panel mirror line");
            assert_eq!(buffer[(72, rows[0])].symbol(), " ");
            assert_eq!(buffer[(155, rows[0])].symbol(), " ");
        }
    }

    #[test]
    fn short_stacked_layout_hides_spectrum_to_keep_lyrics() {
        let config = Config {
            spectrum_enabled: true,
            spectrum_style: SpectrumKind::Reflect,
            ..Config::default()
        };
        let mut state = AppState::new(&config);
        state.layout = PlayLayout::Stacked;
        state.now = Some(NowPlaying {
            title: "Title".into(),
            artist: "Artist".into(),
            album: String::new(),
            artist_id: None,
            album_id: None,
        });
        state.lyrics = vec![LyricLine {
            time: Duration::ZERO,
            text: "Kept lyric".into(),
            translation: None,
            word_timing: None,
        }];
        state
            .spectrum
            .tick(&SampleBuffer::default(), false, true, true, false, true);
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| super::draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        let symbols = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(symbols.contains("Kept lyric"));
        assert!(!(0..18)
            .any(|y| { (0..80).all(|x| terminal.backend().buffer()[(x, y)].symbol() == "─") }));
    }

    fn word_synced_line() -> LyricLine {
        LyricLine {
            time: Duration::from_secs(1),
            text: "雨🌧️爱".into(),
            translation: Some("Rain love".into()),
            word_timing: Some(YrcLine {
                start: Duration::from_secs(1),
                duration: Duration::from_millis(900),
                words: vec![
                    YrcWord {
                        text: "雨".into(),
                        start: Duration::from_millis(1_000),
                        duration: Duration::from_millis(200),
                    },
                    YrcWord {
                        text: "🌧️".into(),
                        start: Duration::from_millis(1_200),
                        duration: Duration::from_millis(200),
                    },
                    YrcWord {
                        text: "爱".into(),
                        start: Duration::from_millis(1_500),
                        duration: Duration::ZERO,
                    },
                ],
            }),
        }
    }

    #[test]
    fn yrc_words_use_half_open_current_intervals_and_preserve_utf8() {
        let state = AppState::new(&Config::default());
        let lyric = word_synced_line();

        let at_first_start =
            current_lyric_spans(&lyric, Duration::from_millis(1_000), &state.theme);
        assert_eq!(at_first_start[0].style.fg, Some(state.theme.accent));
        assert!(at_first_start[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(at_first_start[1].style.fg, Some(state.theme.fg));

        let at_boundary = current_lyric_spans(&lyric, Duration::from_millis(1_200), &state.theme);
        assert_eq!(at_boundary[0].style.fg, Some(state.theme.accent));
        assert!(!at_boundary[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(at_boundary[1].style.fg, Some(state.theme.accent));
        assert!(at_boundary[1].style.add_modifier.contains(Modifier::BOLD));

        let in_gap = current_lyric_spans(&lyric, Duration::from_millis(1_450), &state.theme);
        assert!(in_gap
            .iter()
            .all(|span| !span.style.add_modifier.contains(Modifier::BOLD)));
        assert_eq!(in_gap[2].style.fg, Some(state.theme.fg));

        let zero_duration = current_lyric_spans(&lyric, Duration::from_millis(1_500), &state.theme);
        assert_eq!(zero_duration[2].style.fg, Some(state.theme.accent));
        assert!(!zero_duration[2].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(
            zero_duration
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "雨🌧️爱"
        );
    }

    #[test]
    fn yrc_start_can_advance_the_current_line_ahead_of_its_lrc_timestamp() {
        let mut upcoming = word_synced_line();
        upcoming.time = Duration::from_secs(2);
        let lines = vec![
            LyricLine {
                time: Duration::from_secs(1),
                text: "Previous".into(),
                translation: None,
                word_timing: None,
            },
            upcoming,
        ];

        assert_eq!(
            current_lyric_index(&lines, Duration::from_millis(1_250)),
            Some(1)
        );

        let mut late_previous = lines.clone();
        late_previous[1].word_timing = None;
        late_previous[0].word_timing = Some(YrcLine {
            start: Duration::from_millis(2_100),
            duration: Duration::from_millis(100),
            words: vec![YrcWord {
                text: "Previous".into(),
                start: Duration::from_millis(2_100),
                duration: Duration::from_millis(100),
            }],
        });
        assert_eq!(
            current_lyric_index(&late_previous, Duration::from_millis(2_150)),
            Some(1),
            "late YRC data must not move the display back to an older line"
        );
    }

    #[test]
    fn zen_mode_renders_word_synced_lyrics_and_the_translation_pair() {
        let mut state = AppState::new(&Config::default());
        state.zen = true;
        state.now = Some(NowPlaying {
            title: "Title".into(),
            artist: "Artist".into(),
            album: String::new(),
            artist_id: None,
            album_id: None,
        });
        state.position = Duration::from_millis(1_250);
        state.lyrics = vec![word_synced_line()];
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| crate::ui::draw(frame, &mut state, &mut hits))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rain = buffer
            .content()
            .iter()
            .find(|cell| cell.symbol() == "雨")
            .expect("word-synchronised original should be visible");
        let emoji = buffer
            .content()
            .iter()
            .find(|cell| cell.symbol() == "🌧️")
            .expect("current word should be visible");
        assert_eq!(rain.fg, state.theme.accent);
        assert!(!rain.modifier.contains(Modifier::BOLD));
        assert_eq!(emoji.fg, state.theme.accent);
        assert!(emoji.modifier.contains(Modifier::BOLD));

        let translation = "Rain love";
        let translation_start = buffer
            .content()
            .windows(translation.len())
            .find(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == translation)
            .expect("translation should remain paired with the current line");
        assert!(translation_start
            .iter()
            .all(|cell| cell.fg == state.theme.fg && cell.modifier.contains(Modifier::ITALIC)));
    }

    #[test]
    fn metadata_and_liked_progress_render_the_new_visual_hierarchy() {
        let mut state = AppState::new(&Config::default());
        state.now = Some(NowPlaying {
            title: "Title".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            artist_id: None,
            album_id: None,
        });
        state.current_track_id = Some(42);
        state.liked.insert(42);
        state.position = Duration::from_secs(30);
        state.duration = Some(Duration::from_secs(120));
        state.status = Some("Resolving".into());
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| {
                draw_meta(
                    frame,
                    &mut state,
                    Rect::new(0, 0, 80, 5),
                    false,
                    &mut Hits::default(),
                );
                draw_progress(frame, &state, Rect::new(0, 6, 80, 1), &mut hits);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(2, 0)].fg, state.theme.fg);
        assert!(buffer[(2, 0)].modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(2, 1)].fg, state.theme.dim);
        assert_eq!(buffer[(2, 2)].fg, state.theme.dim);
        assert_eq!(buffer[(2, 4)].fg, state.theme.dim);
        assert!(buffer
            .content()
            .iter()
            .any(|cell| cell.symbol() == "━" && cell.fg == state.theme.accent));
        for time in ["00:30", "02:00"] {
            let start = (0..=80 - time.len() as u16)
                .find(|x| rect_text(buffer, Rect::new(*x, 6, time.len() as u16, 1)) == time)
                .unwrap();
            assert!(
                (start..start + time.len() as u16).all(|x| buffer[(x, 6)].fg == state.theme.faint)
            );
        }
        let progress = (0..80).map(|x| buffer[(x, 6)].symbol()).collect::<String>();
        assert!(progress.contains('♥'));
        assert!(!progress.contains('♡'));
    }

    #[test]
    fn personal_fm_is_badged_on_the_artist_line_and_only_while_it_feeds_the_queue() {
        let staged = |source| {
            let mut state = AppState::new(&Config::default());
            state.now = Some(NowPlaying {
                title: "Title".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                artist_id: None,
                album_id: None,
            });
            state.queue_source = source;
            state
        };
        let badge = crate::i18n::t(crate::i18n::Key::PersonalFm);
        let render = |state: &mut AppState| {
            let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
            terminal
                .draw(|frame| {
                    draw_meta(
                        frame,
                        state,
                        Rect::new(0, 0, 80, 5),
                        false,
                        &mut Hits::default(),
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let artist_line = (0..80).map(|x| buffer[(x, 1)].symbol()).collect::<String>();
            let accented = (0..80)
                .any(|x| buffer[(x, 1)].fg == state.theme.accent && buffer[(x, 1)].symbol() != " ");
            (artist_line, accented)
        };

        let (line, accented) = render(&mut staged(crate::api::Source::Liked));
        assert!(line.contains("Artist"));
        assert!(!line.contains(badge), "a normal queue carries no FM badge");
        assert!(!accented);

        let (line, accented) = render(&mut staged(crate::api::Source::Fm));
        assert!(line.contains("Artist"));
        assert!(line.contains(badge));
        assert!(accented, "the badge is set apart by the accent colour");
    }

    #[test]
    fn dashboard_leaves_account_status_to_the_global_header() {
        let mut state = AppState::new(&Config::default());
        state.session.nickname = Some("Listener".into());
        state.library_synced = true;
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| super::draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!rendered.contains("Listener"));
        let row = hits.menu[0].0;
        let shortcut = &terminal.backend().buffer()[(row.right() - 2, row.y)];
        let label = &terminal.backend().buffer()[(row.x + 1, row.y)];
        // Accent text, no pill: the badge shares the label's background.
        assert_eq!(shortcut.fg, state.theme.accent);
        assert_eq!(shortcut.bg, label.bg);
    }

    #[test]
    fn progress_heart_uses_color_as_the_only_liked_state_signal() {
        for liked in [false, true] {
            let mut state = AppState::new(&Config::default());
            state.current_track_id = Some(42);
            if liked {
                state.liked.insert(42);
            }
            let backend = TestBackend::new(100, 1);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = Hits::default();

            terminal
                .draw(|frame| draw_progress(frame, &state, frame.area(), &mut hits))
                .unwrap();

            let rect = hits.heart[0].0;
            let buffer = terminal.backend().buffer();
            assert_eq!(rect_text(buffer, rect), "♥");
            assert_eq!(
                buffer[(rect.x, rect.y)].fg,
                if liked {
                    state.theme.accent2
                } else {
                    state.theme.faint
                }
            );
        }
    }

    #[test]
    fn progress_play_button_shows_the_action_and_uses_its_drawn_hit_target() {
        for (paused, expected) in [(true, "▶"), (false, "⏸")] {
            let mut state = AppState::new(&Config::default());
            state.paused = paused;
            let backend = TestBackend::new(100, 1);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = Hits::default();

            terminal
                .draw(|frame| draw_progress(frame, &state, frame.area(), &mut hits))
                .unwrap();

            let rect = hits.play[0].0;
            assert_eq!(rect_text(terminal.backend().buffer(), rect), expected);
            assert!(matches!(
                event::mouse_action(click(rect), &hits, 0),
                Some(Action::TogglePlay)
            ));
        }
    }

    #[test]
    fn progress_projects_repeat_and_shuffle_into_one_clickable_slot() {
        let cases = [
            (false, PlayMode::Off, "→", false),
            (false, PlayMode::List, "↺", true),
            (false, PlayMode::One, "↺¹", true),
            (true, PlayMode::Off, "⇆", true),
            (true, PlayMode::List, "⇆", true),
            (true, PlayMode::One, "⇆", true),
        ];
        for (shuffle, repeat, expected, active) in cases {
            let mut state = AppState::new(&Config::default());
            state.shuffle = shuffle;
            state.play_mode = repeat;
            state.volume = 0.4;
            let backend = TestBackend::new(100, 1);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = Hits::default();

            terminal
                .draw(|frame| draw_progress(frame, &state, frame.area(), &mut hits))
                .unwrap();

            let buffer = terminal.backend().buffer();
            let rect = hits.playback_mode[0].0;
            assert_eq!(rect_text(buffer, rect), expected);
            assert_eq!(
                buffer[(rect.x, rect.y)].fg,
                if active {
                    state.theme.accent
                } else {
                    state.theme.faint
                }
            );
            let rendered = (0..100)
                .map(|x| buffer[(x, 0)].symbol())
                .collect::<String>();
            assert!(!rendered.contains('×'));
            assert!(rendered.contains("●●●●○○○○○○"));
            assert!(matches!(
                event::mouse_action(click(rect), &hits, 0),
                Some(Action::CyclePlaybackMode)
            ));
        }
    }

    #[test]
    fn nerd_setting_switches_the_progress_controls_to_the_nerd_table() {
        let config = Config {
            icons: crate::config::IconStyle::Nerd,
            ..Config::default()
        };
        let mut state = AppState::new(&config);
        state.paused = true;
        state.current_track_id = Some(42);
        state.liked.insert(42);
        state.shuffle = true;
        state.volume = 0.5;
        let icons = crate::icons::for_style(crate::config::IconStyle::Nerd);
        let backend = TestBackend::new(100, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw_progress(frame, &state, frame.area(), &mut hits))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(rect_text(buffer, hits.play[0].0), icons.play);
        assert_eq!(rect_text(buffer, hits.heart[0].0), icons.heart);
        assert_eq!(rect_text(buffer, hits.playback_mode[0].0), icons.shuffle);
        let rendered = (0..100)
            .map(|x| buffer[(x, 0)].symbol())
            .collect::<String>();
        // Half volume reaches the high band of the three-state speaker.
        assert!(rendered.contains(icons.volume_high));
        assert!(rendered.contains(&icons.volume_full.repeat(5)));
        assert!(rendered.contains(&icons.volume_empty.repeat(5)));
    }

    #[test]
    fn dashboard_menu_only_advertises_shortcuts_that_still_exist() {
        let state = AppState::new(&Config::default());
        let entries = menu_entries(&state);

        assert_eq!(
            entries
                .iter()
                .find(|(_, _, entry)| *entry == MenuEntry::Search)
                .map(|(_, key, _)| *key),
            Some("3")
        );
        assert_eq!(
            entries
                .iter()
                .find(|(_, _, entry)| *entry == MenuEntry::Login)
                .map(|(_, key, _)| *key),
            Some("")
        );
    }
}
