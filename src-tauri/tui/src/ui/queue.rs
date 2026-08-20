//! Queue view: the current listening context; the play glyph marks its row.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::AppState;
use crate::i18n::{self, Key};
use crate::ui::text::{needs_marquee, pad_display, pad_or_marquee};
use crate::ui::Hits;

/// The now-playing marker occupies one terminal cell.
const MARKER_WIDTH: usize = 1;
/// Keep the play marker distinct from the heart state.
const MARKER_HEART_GAP: usize = 1;
/// The solid heart state occupies one terminal cell.
const HEART_WIDTH: usize = 1;
/// Keep the heart distinct from the ordinal column.
const HEART_INDEX_GAP: usize = 1;
/// Minimum width of the right-aligned ordinal column.
const MIN_INDEX_WIDTH: usize = 3;
/// Gap between the ordinal and the primary title column.
const INDEX_TITLE_GAP: usize = 2;
/// Artist metadata keeps a stable scan width.
const ARTIST_WIDTH: usize = 12;
/// Terminal playback durations always use `mm:ss`.
const DURATION_WIDTH: usize = 5;
/// Metadata columns are separated by one blank terminal cell.
const COLUMN_GAP: usize = 1;

pub fn draw(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    let theme = &state.theme;
    let icons = crate::icons::for_style(state.config.icons);
    let rows = state.visible_rows(&state.queue);
    let block = super::panel_block(
        theme,
        i18n::t(Key::Queue),
        Some(i18n::t_track_count(rows.len())),
    )
    .title_bottom(super::filter_title(state));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    if rows.is_empty() {
        let message = if !state.filter.query.is_empty() && !state.queue.is_empty() {
            i18n::t(Key::NoResults)
        } else {
            i18n::t(Key::EmptyQueue)
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
    let max_index = rows
        .iter()
        .map(|(index, _)| index + 1)
        .max()
        .expect("non-empty rows must have a maximum ordinal");
    let columns = QueueColumns::for_width_and_max_index(inner.width as usize, max_index);
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
        let playing = state.queue_pos == Some(*index);
        let selected = visible_index == state.selected && !state.filter.input;
        let marker = if playing { icons.play } else { " " };
        let liked = state.liked.contains(&row.id);
        lines.push(columns.row(
            theme,
            state.selection_style(),
            marker,
            icons.heart,
            index + 1,
            row,
            playing,
            liked,
            selected,
            marquee_frame,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

pub(crate) fn marquee_needed(row: &crate::api::SongRow, area_width: u16, max_index: usize) -> bool {
    let columns =
        QueueColumns::for_width_and_max_index(super::panel_inner_width(area_width), max_index);
    // Same rule as the library and search lists: only the title scrolls.
    needs_marquee(&row.title, columns.title)
}

#[derive(Clone, Copy)]
struct QueueColumns {
    index: usize,
    title: usize,
}

impl QueueColumns {
    #[cfg(test)]
    fn for_width(width: usize) -> Self {
        Self::for_width_and_max_index(width, 1)
    }

    fn for_width_and_max_index(width: usize, max_index: usize) -> Self {
        let index = MIN_INDEX_WIDTH.max(max_index.to_string().len());
        let fixed = MARKER_WIDTH
            + MARKER_HEART_GAP
            + HEART_WIDTH
            + HEART_INDEX_GAP
            + index
            + INDEX_TITLE_GAP
            + ARTIST_WIDTH
            + DURATION_WIDTH
            + COLUMN_GAP * 2;
        Self {
            index,
            title: width.saturating_sub(fixed),
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
        Line::from(vec![
            Span::styled(
                " ".repeat(MARKER_WIDTH + MARKER_HEART_GAP + HEART_WIDTH + HEART_INDEX_GAP),
                style,
            ),
            Span::styled(format!("{:>width$}", "#", width = self.index), style),
            Span::styled(" ".repeat(INDEX_TITLE_GAP), style),
            Span::styled(
                super::text::pad_display(i18n::t(Key::ColumnTitle), self.title),
                style,
            ),
            Span::styled(" ", style),
            Span::styled(
                super::text::pad_display(i18n::t(Key::ColumnArtist), ARTIST_WIDTH),
                style,
            ),
            Span::styled(" ", style),
            Span::styled(
                super::text::pad_display_right(duration_label, DURATION_WIDTH),
                style,
            ),
        ])
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        self,
        theme: &crate::theme::Theme,
        selection_style: Style,
        marker: &'static str,
        heart: &'static str,
        index: usize,
        row: &crate::api::SongRow,
        playing: bool,
        liked: bool,
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
        Line::from(vec![
            Span::styled(
                marker,
                base.fg(if playing { theme.accent } else { theme.faint }),
            ),
            Span::styled(" ", base),
            Span::styled(
                heart,
                base.fg(if liked { theme.accent2 } else { theme.faint }),
            ),
            Span::styled(" ", base),
            Span::styled(
                format!("{index:>width$}", width = self.index),
                base.fg(theme.faint),
            ),
            Span::styled(" ".repeat(INDEX_TITLE_GAP), base),
            Span::styled(
                pad_or_marquee(&row.title, self.title, selected, marquee_frame),
                title_style,
            ),
            Span::styled(" ", base),
            Span::styled(pad_display(&row.artist, ARTIST_WIDTH), base.fg(theme.dim)),
            Span::styled(" ", base),
            Span::styled(
                format!("{:>DURATION_WIDTH$}", super::format_ms(row.duration_ms)),
                base.fg(theme.faint),
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::api::SongRow;
    use crate::config::Config;

    #[test]
    fn cjk_duration_header_fits_every_supported_queue_width() {
        let state = AppState::new(&Config::default());
        let minimum = MARKER_WIDTH
            + MARKER_HEART_GAP
            + HEART_WIDTH
            + HEART_INDEX_GAP
            + MIN_INDEX_WIDTH
            + INDEX_TITLE_GAP
            + ARTIST_WIDTH
            + DURATION_WIDTH
            + COLUMN_GAP * 2;

        for width in minimum..=200 {
            let header = QueueColumns::for_width(width).header_with_duration(&state.theme, "时长");
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
    fn four_digit_ordinals_are_included_in_the_column_budget() {
        let columns = QueueColumns::for_width_and_max_index(80, 1_000);
        let state = AppState::new(&Config::default());
        let row = SongRow {
            id: 1,
            title: "Track".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            duration_ms: 180_000,
            pic_url: None,
            artist_id: None,
            album_id: None,
        };

        assert_eq!(columns.index, 4);
        assert_eq!(columns.header(&state.theme).width(), 80);
        assert_eq!(
            columns
                .row(
                    &state.theme,
                    Style::new(),
                    " ",
                    "♥",
                    1_000,
                    &row,
                    false,
                    false,
                    false,
                    0,
                )
                .width(),
            80
        );
    }

    #[test]
    fn queue_rows_render_solid_hearts_in_state_colors() {
        for liked in [false, true] {
            let backend = TestBackend::new(80, 5);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut state = AppState::new(&Config::default());
            state.queue.push(SongRow {
                id: 1,
                title: "Track".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                duration_ms: 180_000,
                pic_url: None,
                artist_id: None,
                album_id: None,
            });
            if liked {
                state.liked.insert(1);
            }
            let mut hits = Hits::default();

            terminal
                .draw(|frame| draw(frame, &state, frame.area(), &mut hits))
                .unwrap();

            let cell = &terminal.backend().buffer()[(4, 2)];
            assert_eq!(cell.symbol(), "♥");
            assert_eq!(
                cell.fg,
                if liked {
                    state.theme.accent2
                } else {
                    state.theme.faint
                }
            );
        }
    }

    #[test]
    fn queue_panel_and_selected_row_follow_the_skeleton_language() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.queue.push(SongRow {
            id: 1,
            title: "Track".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            duration_ms: 180_000,
            pic_url: None,
            artist_id: None,
            album_id: None,
        });
        state.queue_pos = Some(0);
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw(frame, &state, frame.area(), &mut hits))
            .unwrap();

        let buffer = terminal.backend().buffer();
        for (position, symbol) in [
            ((0, 0), "╭"),
            ((79, 0), "╮"),
            ((0, 23), "╰"),
            ((79, 23), "╯"),
        ] {
            assert_eq!(buffer[position].symbol(), symbol);
        }
        assert_eq!(hits.rows, vec![(Rect::new(2, 2, 76, 1), 0)]);
        assert_eq!(buffer[(2, 2)].fg, state.theme.accent);
        assert_eq!(buffer[(11, 2)].fg, state.theme.fg);
        assert_eq!(buffer[(11, 2)].bg, state.selection_style().bg.unwrap());
        assert!(buffer[(11, 2)].modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(60, 2)].fg, state.theme.dim);
        assert_eq!(buffer[(73, 2)].fg, state.theme.faint);
    }
}
