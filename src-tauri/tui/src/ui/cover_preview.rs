use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::AppState;
use crate::i18n::{self, Key};
use crate::ui::text::{needs_marquee, pad_or_marquee};

const FRAME_COLUMNS: u16 = 2;
const FRAME_ROWS: u16 = 2;
const METADATA_ROWS: u16 = 2;
/// Outer width: frame + horizontal panel padding + square half-block image.
pub const WIDTH: u16 = crate::app::PREVIEW_CELLS.0 + super::PANEL_PADDING_X * 2 + FRAME_COLUMNS;
/// Outer height: frame + square half-block image + title and artist rows.
pub const HEIGHT: u16 = crate::app::PREVIEW_CELLS.1 + FRAME_ROWS + METADATA_ROWS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PreviewAreas {
    image: Rect,
    title: Rect,
    artist: Rect,
}

fn content_areas(inner: Rect) -> PreviewAreas {
    let metadata_height = METADATA_ROWS.min(inner.height);
    let image_height = inner.height.saturating_sub(metadata_height);
    let metadata_y = inner.y.saturating_add(image_height);
    PreviewAreas {
        image: Rect {
            height: image_height,
            ..inner
        },
        title: Rect {
            y: metadata_y,
            height: u16::from(metadata_height >= 1),
            ..inner
        },
        artist: Rect {
            y: metadata_y.saturating_add(1),
            height: u16::from(metadata_height >= 2),
            ..inner
        },
    }
}

pub fn split_preview(area: Rect, min_list_width: u16) -> (Rect, Option<Rect>) {
    let required_width = min_list_width
        .saturating_add(super::PANEL_GAP_X)
        .saturating_add(WIDTH);
    if area.width < required_width || area.height < HEIGHT {
        return (area, None);
    }

    let list_width = area.width - super::PANEL_GAP_X - WIDTH;
    let list = Rect {
        width: list_width,
        ..area
    };
    let preview = Rect {
        x: area
            .x
            .saturating_add(list_width)
            .saturating_add(super::PANEL_GAP_X),
        y: area.y,
        width: WIDTH,
        height: HEIGHT,
    };
    (list, Some(preview))
}

pub(crate) fn metadata_needs_marquee(row: &crate::api::SongRow) -> bool {
    let width = usize::from(crate::app::PREVIEW_CELLS.0);
    needs_marquee(&row.title, width) || needs_marquee(&row.artist, width)
}

pub fn draw(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let metadata = state
        .visible_row(state.selected)
        .map(|(_, row)| (row.title, row.artist));
    let block = super::panel_block(&state.theme, i18n::t(Key::Cover), None);
    let inner = block.inner(area);
    let areas = content_areas(inner);
    frame.render_widget(block, area);

    if state.selected_original_is_available() && !areas.image.is_empty() {
        // Plain background under the terminal-graphics image, like the
        // playing view: a pixel backdrop peeks out of the last partial row
        // (the placeholder record's bottom edge showed as a grey sliver).
        frame.render_widget(
            ratatui::widgets::Block::new().style(Style::new().bg(state.theme.bg)),
            areas.image,
        );
        state.render_selected_original(frame, areas.image);
    } else if !areas.image.is_empty() {
        if let Some(cover) = state.selected_pixel_cover() {
            frame.render_widget(cover, areas.image);
        } else {
            frame.render_widget(state.preview_placeholder(), areas.image);
        }
    }

    if let Some((title, artist)) = metadata {
        let style = Style::new().fg(state.theme.dim);
        let marquee_frame = state.marquee_frame();
        if !areas.title.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    pad_or_marquee(&title, usize::from(areas.title.width), true, marquee_frame),
                    style,
                ))),
                areas.title,
            );
        }
        if !areas.artist.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    pad_or_marquee(
                        &artist,
                        usize::from(areas.artist.width),
                        true,
                        marquee_frame,
                    ),
                    style,
                ))),
                areas.artist,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::action::View;
    use crate::api::SongRow;
    use crate::config::Config;

    fn line_text(buffer: &ratatui::buffer::Buffer, area: Rect) -> String {
        (area.x..area.right())
            .map(|x| buffer[(x, area.y)].symbol())
            .collect()
    }

    #[test]
    fn preview_split_reserves_the_frame_and_two_metadata_rows() {
        let too_short = Rect::new(7, 3, 90, 14);
        assert_eq!(split_preview(too_short, 62), (too_short, None));

        let area = Rect::new(7, 3, 90, 15);
        assert_eq!(
            split_preview(area, 62),
            (Rect::new(7, 3, 62, 15), Some(Rect::new(71, 3, 26, 15)),)
        );
    }

    #[test]
    fn preview_draws_a_faint_rounded_panel_with_dim_marquee_metadata() {
        let backend = TestBackend::new(26, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.view = View::Library;
        state.library = vec![SongRow {
            id: 7,
            title: "1234567890123456789012345".into(),
            artist: "abcdefghijklmnopqrstuvwxy".into(),
            album: "Album".into(),
            duration_ms: 180_000,
            pic_url: None,
            artist_id: None,
            album_id: None,
        }];
        state.selected = 0;
        let theme = state.theme;

        terminal
            .draw(|frame| draw(frame, &mut state, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();

        assert_eq!(buffer[(0, 0)].symbol(), "╭");
        assert_eq!(buffer[(25, 0)].symbol(), "╮");
        assert_eq!(buffer[(0, 14)].symbol(), "╰");
        assert_eq!(buffer[(25, 14)].symbol(), "╯");
        assert_eq!(buffer[(0, 0)].fg, theme.faint);
        let top = line_text(buffer, Rect::new(0, 0, 26, 1));
        let compact = |value: &str| {
            value
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect::<String>()
        };
        assert!(compact(&top).contains(&compact(i18n::t(Key::Cover))));
        assert!((1..25).any(|x| {
            !buffer[(x, 0)].symbol().trim().is_empty() && buffer[(x, 0)].fg == theme.accent
        }));

        let title = line_text(buffer, Rect::new(2, 12, 22, 1));
        let artist = line_text(buffer, Rect::new(2, 13, 22, 1));
        assert_eq!(title, "1234567890123456789012");
        assert_eq!(artist, "abcdefghijklmnopqrstuv");
        assert!((2..24).all(|x| buffer[(x, 12)].fg == theme.dim));
        assert!((2..24).all(|x| buffer[(x, 13)].fg == theme.dim));
    }

    #[test]
    fn preview_metadata_scrolls_only_past_its_inner_width() {
        let exact = SongRow {
            id: 1,
            title: "1234567890123456789012".into(),
            artist: "short".into(),
            album: String::new(),
            duration_ms: 0,
            pic_url: None,
            artist_id: None,
            album_id: None,
        };
        assert!(!metadata_needs_marquee(&exact));

        let overflowing = SongRow {
            title: "12345678901234567890123".into(),
            ..exact
        };
        assert!(metadata_needs_marquee(&overflowing));
    }

    #[test]
    fn library_preview_appears_at_the_exact_width_and_height_boundary() {
        let too_narrow = Rect::new(7, 3, 79, 15);
        assert_eq!(split_preview(too_narrow, 52), (too_narrow, None));

        let too_short = Rect::new(7, 3, 80, 14);
        assert_eq!(split_preview(too_short, 52), (too_short, None));

        let area = Rect::new(7, 3, 80, 15);
        assert_eq!(
            split_preview(area, 52),
            (
                Rect::new(7, 3, 52, 15),
                Some(Rect::new(61, 3, WIDTH, HEIGHT)),
            )
        );
    }

    #[test]
    fn search_preview_needs_one_more_list_column() {
        let too_narrow = Rect::new(0, 0, 80, 20);
        assert_eq!(split_preview(too_narrow, 53), (too_narrow, None));

        let area = Rect::new(0, 0, 81, 20);
        assert_eq!(
            split_preview(area, 53),
            (
                Rect::new(0, 0, 53, 20),
                Some(Rect::new(55, 0, WIDTH, HEIGHT)),
            )
        );
    }

    #[test]
    fn spare_width_stays_with_the_list() {
        let area = Rect::new(4, 2, 87, 20);
        let (list, preview) = split_preview(area, 52);

        assert_eq!(list, Rect::new(4, 2, 59, 20));
        assert_eq!(preview, Some(Rect::new(65, 2, WIDTH, HEIGHT)));
    }

    #[test]
    fn image_area_excludes_the_frame_and_both_metadata_rows() {
        assert_eq!(
            content_areas(Rect::new(2, 1, 22, 13)),
            PreviewAreas {
                image: Rect::new(2, 1, 22, 11),
                title: Rect::new(2, 12, 22, 1),
                artist: Rect::new(2, 13, 22, 1),
            }
        );
    }

    #[test]
    fn preview_stays_two_columns_after_the_list_at_all_product_widths() {
        for area in [
            Rect::new(20, 2, 60, 20),
            Rect::new(20, 2, 100, 36),
            Rect::new(65, 2, 90, 56),
        ] {
            let (list, preview) = split_preview(area, 62);
            if let Some(preview) = preview {
                assert_eq!(preview.x, list.right() + super::super::PANEL_GAP_X);
                assert_eq!(preview.right(), area.right());
                assert!(preview.bottom() <= area.bottom());
            } else {
                assert_eq!(list, area);
            }
        }
    }
}
