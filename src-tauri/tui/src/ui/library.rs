//! Library view: collapsible sidebar + track list. Sidebar entries become
//! real NCM playlists in the service stage.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::AppState;
use crate::i18n::{self, Key};
use crate::ui::Hits;

use super::cover_preview;
use super::text::{needs_marquee, pad_display, pad_or_marquee};

/// The framed source switcher stays compact while leaving 52 columns for songs.
const SIDEBAR_PANEL_WIDTH: u16 = 18;
/// A framed song list remains useful down to this outer width.
const MIN_LIST_PANEL_WIDTH: u16 = 52;
/// Showing a cover still leaves a 58-column padded list for the album column.
const PREVIEW_MIN_LIST_PANEL_WIDTH: u16 = 62;
/// Full shell width required for sidebar, list, gap, and framed preview.
pub(crate) const PREVIEW_MIN_TERMINAL_WIDTH: u16 = SIDEBAR_PANEL_WIDTH
    + super::PANEL_GAP_X * 2
    + PREVIEW_MIN_LIST_PANEL_WIDTH
    + cover_preview::WIDTH;
/// Hide the sidebar before either adjacent panel becomes too narrow.
pub const COLLAPSE_BELOW: u16 = SIDEBAR_PANEL_WIDTH + super::PANEL_GAP_X + MIN_LIST_PANEL_WIDTH;
/// Width of the right-aligned ordinal column.
const INDEX_WIDTH: usize = 3;
/// Gap between the ordinal and the primary title column.
const INDEX_TITLE_GAP: usize = 2;
/// Compact lists reserve a stable baseline for the song title.
const COMPACT_TITLE_WIDTH: usize = 28;
/// Wide lists compress secondary columns to protect the title.
const WIDE_METADATA_WIDTH: usize = 7;
/// Terminal playback durations always use `mm:ss`.
const DURATION_WIDTH: usize = 5;
/// Metadata columns are separated by one blank terminal cell.
const COLUMN_GAP: usize = 1;
/// The wide profile starts with a 31-column title at this boundary.
const ALBUM_COLUMN_MIN_INNER_WIDTH: usize = 58;

pub fn draw(frame: &mut Frame, state: &mut AppState, area: Rect, hits: &mut Hits) {
    let content = if area.width >= COLLAPSE_BELOW {
        let [sidebar, _, content] = Layout::horizontal([
            Constraint::Length(SIDEBAR_PANEL_WIDTH),
            Constraint::Length(super::PANEL_GAP_X),
            Constraint::Min(0),
        ])
        .areas(area);
        draw_sidebar(frame, state, sidebar, hits);
        content
    } else {
        area
    };

    let has_selected_row = !state.sidebar_focus
        && !state.filter.input
        && state.library.iter().any(|row| state.filter.matches(row));
    let (list, preview) = if has_selected_row {
        cover_preview::split_preview(content, PREVIEW_MIN_LIST_PANEL_WIDTH)
    } else {
        (content, None)
    };
    draw_list(frame, state, list, hits);
    if let Some(preview) = preview {
        cover_preview::draw(frame, state, preview);
    }
}

pub(crate) fn marquee_needed(
    row: &crate::api::SongRow,
    area_width: u16,
    preview_visible: bool,
) -> bool {
    let content_width = if area_width >= COLLAPSE_BELOW {
        area_width.saturating_sub(SIDEBAR_PANEL_WIDTH + super::PANEL_GAP_X)
    } else {
        area_width
    };
    let content = Rect::new(0, 0, content_width, cover_preview::HEIGHT);
    let (list, preview) = if preview_visible {
        cover_preview::split_preview(content, PREVIEW_MIN_LIST_PANEL_WIDTH)
    } else {
        (content, None)
    };
    let columns = SongColumns::for_width(super::panel_inner_width(list.width));
    // Only the title scrolls in a row: it is what identifies the track, and
    // two runs sliding off one frame counter read as the whole row moving.
    // The artist and album columns truncate, as the album already did.
    needs_marquee(&row.title, columns.title)
        || preview.is_some() && cover_preview::metadata_needs_marquee(row)
}

pub const SOURCES: [Key; 4] = [
    Key::LikedSongs,
    Key::DailyRecommendations,
    Key::PersonalFm,
    Key::CloudDrive,
];

fn draw_sidebar(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    let theme = &state.theme;
    let block = super::panel_block(theme, i18n::t(Key::Library), None);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }

    let mut lines = Vec::with_capacity(SOURCES.len());
    for (index, key) in SOURCES.iter().enumerate().take(inner.height as usize) {
        let y = inner.y + index as u16;
        if y < inner.bottom() {
            hits.sidebar.push((
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
                index,
            ));
        }
        let is_current = index == state.source_index();
        let is_cursor = state.sidebar_focus && index == state.sidebar_selected;
        let selected = if state.sidebar_focus {
            is_cursor
        } else {
            is_current
        };
        let marker = if is_current { "▸" } else { " " };
        let mut style = if selected {
            state
                .selection_style()
                .fg(if is_current { theme.accent } else { theme.fg })
                .add_modifier(Modifier::BOLD)
        } else if is_current {
            Style::new().fg(theme.accent)
        } else {
            Style::new().fg(theme.dim)
        };
        if is_current {
            style = style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(Span::styled(
            pad_display(&format!("{marker} {}", i18n::t(*key)), inner.width as usize),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_list(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    let theme = &state.theme;
    let rows = state.visible_rows(&state.library);
    let source_title = SOURCES
        .get(state.source_index())
        .copied()
        .unwrap_or(Key::Library);
    let block = super::panel_block(
        theme,
        i18n::t(source_title),
        Some(i18n::t_track_count(rows.len())),
    )
    .title_bottom(super::filter_title(state));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    if rows.is_empty() {
        let message = if !state.filter.query.is_empty() && !state.library.is_empty() {
            i18n::t(Key::NoResults)
        } else if state.session.nickname.is_some() && !state.library_synced {
            i18n::t(Key::SyncingLibrary)
        } else {
            i18n::t(Key::EmptyLibrary)
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                message,
                Style::new().fg(theme.dim),
            )))
            .centered(),
            inner,
        );
        return;
    }
    let visible = inner.height.saturating_sub(1) as usize; // header row
    let offset = super::scroll_offset(state.selected, rows.len(), visible);
    let marquee_frame = state.marquee_frame();
    let columns = SongColumns::for_width(inner.width as usize);

    let mut lines = Vec::with_capacity(visible + 1);
    lines.push(columns.header(theme));
    for (visible_index, (index, row)) in rows.iter().enumerate().skip(offset).take(visible) {
        hits.rows.push((
            Rect {
                x: inner.x,
                y: inner.y + 1 + (visible_index - offset) as u16,
                width: inner.width,
                height: 1,
            },
            visible_index,
        ));
        let selected =
            visible_index == state.selected && !state.filter.input && !state.sidebar_focus;
        lines.push(columns.row(
            theme,
            state.selection_style(),
            index + 1,
            row,
            selected,
            marquee_frame,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SongColumns {
    title: usize,
    artist: usize,
    album: Option<usize>,
}

impl SongColumns {
    fn for_width(width: usize) -> Self {
        if width >= ALBUM_COLUMN_MIN_INNER_WIDTH {
            let fixed = INDEX_WIDTH
                + INDEX_TITLE_GAP
                + WIDE_METADATA_WIDTH * 2
                + DURATION_WIDTH
                + COLUMN_GAP * 3;
            return Self {
                title: width.saturating_sub(fixed),
                artist: WIDE_METADATA_WIDTH,
                album: Some(WIDE_METADATA_WIDTH),
            };
        }

        let fixed = INDEX_WIDTH + INDEX_TITLE_GAP + DURATION_WIDTH + COLUMN_GAP * 2;
        let available = width.saturating_sub(fixed);
        let title = COMPACT_TITLE_WIDTH.min(available);
        Self {
            title,
            artist: available.saturating_sub(title),
            album: None,
        }
    }

    fn header(self, theme: &crate::theme::Theme) -> Line<'static> {
        self.header_with_duration(theme, i18n::t(Key::ColumnDuration))
    }

    fn header_with_duration(
        self,
        theme: &crate::theme::Theme,
        duration_label: &str,
    ) -> Line<'static> {
        let style = Style::new().fg(theme.faint);
        let mut spans = vec![
            Span::styled(format!("{:>INDEX_WIDTH$}", "#"), style),
            Span::styled(" ".repeat(INDEX_TITLE_GAP), style),
            Span::styled(pad_display(i18n::t(Key::ColumnTitle), self.title), style),
            Span::styled(" ", style),
            Span::styled(pad_display(i18n::t(Key::ColumnArtist), self.artist), style),
        ];
        if let Some(album_width) = self.album {
            spans.push(Span::styled(" ", style));
            spans.push(Span::styled(
                pad_display(i18n::t(Key::ColumnAlbum), album_width),
                style,
            ));
        }
        spans.push(Span::styled(" ", style));
        spans.push(Span::styled(
            super::text::pad_display_right(duration_label, DURATION_WIDTH),
            style,
        ));
        Line::from(spans)
    }

    fn row(
        self,
        theme: &crate::theme::Theme,
        selection_style: Style,
        index: usize,
        row: &crate::api::SongRow,
        selected: bool,
        marquee_frame: u64,
    ) -> Line<'static> {
        let base = if selected {
            selection_style
        } else {
            Style::new()
        };
        let title_style = if selected {
            base.fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            base.fg(theme.fg)
        };
        let mut spans = vec![
            Span::styled(format!("{index:>INDEX_WIDTH$}"), base.fg(theme.faint)),
            Span::styled(" ".repeat(INDEX_TITLE_GAP), base),
            Span::styled(
                pad_or_marquee(&row.title, self.title, selected, marquee_frame),
                title_style,
            ),
            Span::styled(" ", base),
            Span::styled(pad_display(&row.artist, self.artist), base.fg(theme.dim)),
        ];
        if let Some(album_width) = self.album {
            spans.push(Span::styled(" ", base));
            spans.push(Span::styled(
                pad_display(&row.album, album_width),
                base.fg(theme.dim),
            ));
        }
        spans.push(Span::styled(" ", base));
        spans.push(Span::styled(
            format!("{:>DURATION_WIDTH$}", super::format_ms(row.duration_ms)),
            base.fg(theme.faint),
        ));
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::config::Config;

    fn rendered_library(width: u16, height: u16) -> (ratatui::buffer::Buffer, Hits, AppState) {
        rendered_library_in(width, height, Rect::new(0, 0, width, height))
    }

    fn rendered_library_in(
        width: u16,
        height: u16,
        area: Rect,
    ) -> (ratatui::buffer::Buffer, Hits, AppState) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        for (index, row) in state.library.iter_mut().enumerate() {
            row.album = format!("Album {index}");
            row.duration_ms = 180_000;
        }
        let mut hits = Hits::default();
        terminal
            .draw(|frame| draw(frame, &mut state, area, &mut hits))
            .unwrap();
        (terminal.backend().buffer().clone(), hits, state)
    }

    #[test]
    fn product_widths_keep_wide_titles_above_the_compact_baseline() {
        let compact = SongColumns::for_width(56);
        let standard = SongColumns::for_width(68);
        let centered_wide = SongColumns::for_width(58);

        assert_eq!(
            compact,
            SongColumns {
                title: 28,
                artist: 16,
                album: None,
            }
        );
        assert_eq!(
            standard,
            SongColumns {
                title: 41,
                artist: 7,
                album: Some(7),
            }
        );
        assert_eq!(
            centered_wide,
            SongColumns {
                title: 31,
                artist: 7,
                album: Some(7),
            }
        );
        assert!(standard.title >= compact.title);
        assert!(centered_wide.title >= compact.title);
    }

    #[test]
    fn cjk_duration_header_fits_every_supported_library_width() {
        let state = AppState::new(&Config::default());
        let minimum = INDEX_WIDTH + INDEX_TITLE_GAP + DURATION_WIDTH + COLUMN_GAP * 2;

        for width in minimum..=200 {
            let header = SongColumns::for_width(width).header_with_duration(&state.theme, "时长");
            assert_eq!(header.width(), width, "width {width}");

            let backend = TestBackend::new(width as u16, 1);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| frame.render_widget(Paragraph::new(header.clone()), frame.area()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[((width - 4) as u16, 0)].symbol(), "时");
            assert_eq!(buffer[((width - 2) as u16, 0)].symbol(), "长");
        }
    }

    #[test]
    fn eighty_columns_keep_complete_sidebar_and_list_panels() {
        let (buffer, hits, _state) = rendered_library(80, 24);

        assert!(!hits.rows.is_empty());
        assert!(hits
            .rows
            .iter()
            .all(|(area, _)| area.x == 22 && area.width == 56));
        for (position, symbol) in [
            ((0, 0), "╭"),
            ((17, 0), "╮"),
            ((0, 23), "╰"),
            ((17, 23), "╯"),
            ((20, 0), "╭"),
            ((79, 0), "╮"),
            ((20, 23), "╰"),
            ((79, 23), "╯"),
        ] {
            assert_eq!(buffer[position].symbol(), symbol);
        }
    }

    #[test]
    fn a_long_artist_truncates_instead_of_scrolling_the_row() {
        let row = crate::api::SongRow {
            id: 1,
            title: "Short".into(),
            artist: "A deliberately long artist credit that cannot fit".into(),
            album: String::new(),
            duration_ms: 1000,
            pic_url: None,
            artist_id: None,
            album_id: None,
        };
        // Only the title earns the marquee, so an over-long artist alone must
        // not arm it — otherwise the whole selected row slides for a
        // secondary field the album column already truncates.
        assert!(!marquee_needed(&row, 200, false));
        let with_long_title = crate::api::SongRow {
            title: "A deliberately long track title that cannot fit either".into(),
            ..row
        };
        assert!(marquee_needed(&with_long_title, 60, false));
    }

    #[test]
    fn wide_library_adds_album_and_keeps_cover_beside_the_list() {
        let (buffer, hits, state) = rendered_library(120, 40);

        assert!(!hits.rows.is_empty());
        assert!(hits
            .rows
            .iter()
            .all(|(area, _)| area.x == 22 && area.width == 68));
        assert_eq!(buffer[(91, 0)].symbol(), "╮");
        // Anchored to the preview's own height so adding a metadata row
        // moves the assertion with the panel instead of breaking it.
        let preview_bottom = super::cover_preview::HEIGHT - 1;
        for (position, symbol) in [
            ((94, 0), "╭"),
            ((119, 0), "╮"),
            ((94, preview_bottom), "╰"),
            ((119, preview_bottom), "╯"),
            ((96, 1), "▀"),
        ] {
            assert_eq!(buffer[position].symbol(), symbol);
        }

        let album_initial = i18n::t(Key::ColumnAlbum)
            .chars()
            .next()
            .unwrap()
            .to_string();
        assert_eq!(buffer[(77, 1)].symbol(), album_initial);

        let title = &buffer[(27, 2)];
        assert_eq!(title.fg, state.theme.fg);
        assert_eq!(title.bg, state.selection_style().bg.unwrap());
        assert!(title.modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(69, 2)].fg, state.theme.dim);
        assert_eq!(buffer[(85, 2)].fg, state.theme.faint);
    }

    #[test]
    fn two_hundred_columns_keep_album_and_a_wider_title_inside_the_centered_canvas() {
        let area = Rect::new(45, 2, 110, 56);
        let (buffer, hits, state) = rendered_library_in(200, 60, area);

        assert!(hits
            .rows
            .iter()
            .all(|(row, _)| row.x == 67 && row.width == 58));
        assert_eq!(buffer[(126, 2)].symbol(), "╮");
        let preview_bottom = 2 + super::cover_preview::HEIGHT - 1;
        for (position, symbol) in [
            ((129, 2), "╭"),
            ((154, 2), "╮"),
            ((129, preview_bottom), "╰"),
            ((154, preview_bottom), "╯"),
            ((131, 3), "▀"),
        ] {
            assert_eq!(buffer[position].symbol(), symbol);
        }

        let album_initial = i18n::t(Key::ColumnAlbum)
            .chars()
            .next()
            .unwrap()
            .to_string();
        assert_eq!(buffer[(112, 3)].symbol(), album_initial);

        let title = &buffer[(72, 4)];
        assert_eq!(title.fg, state.theme.fg);
        assert_eq!(title.bg, state.selection_style().bg.unwrap());
        assert!(title.modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(104, 4)].fg, state.theme.dim);
        assert_eq!(buffer[(120, 4)].fg, state.theme.faint);
    }

    #[test]
    fn transparent_selection_reverses_when_osc_background_is_unavailable() {
        let config = Config {
            theme: "transparent".into(),
            ..Config::default()
        };
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&config);
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        let title = &terminal.backend().buffer()[(27, 2)];
        assert!(title.modifier.contains(Modifier::REVERSED));
        assert!(title.modifier.contains(Modifier::BOLD));
    }
}
