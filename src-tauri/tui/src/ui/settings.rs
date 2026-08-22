use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::settings::SettingField;
use crate::app::AppState;
use crate::config::IconStyle;
use crate::i18n::{self, Key};

use super::{panel_block, text::display_width, Hits, PANEL_GAP_Y};

/// Border and shared one-cell padding leave the original 60-column settings measure.
const SETTINGS_PANEL_WIDTH: u16 = 64;
/// Border plus eight inner rows make spectrum styles legible before saving.
const SPECTRUM_PREVIEW_HEIGHT: u16 = 10;
/// Borders, hint, status, actions, and one breathing row surround the setting rows.
const SETTINGS_PANEL_CHROME_HEIGHT: u16 = 8;
/// Hint copy may wrap onto a second terminal row.
const SETTINGS_HINT_HEIGHT: u16 = 2;
/// Status feedback occupies exactly one row above the actions.
const SETTINGS_STATUS_HEIGHT: u16 = 1;
/// Actions keep one row of optical space beneath their keycaps.
const SETTINGS_ACTIONS_HEIGHT: u16 = 2;
/// Save and cancel remain visually distinct without drifting apart.
const SETTINGS_ACTION_GAP: u16 = 3;
/// Marker, arrows, and their spaces consume eight columns around label and value.
const SETTINGS_ROW_CHROME_WIDTH: usize = 8;
/// Each adjustment arrow and its adjacent space share a two-cell hit target.
const SETTINGS_ARROW_WIDTH: u16 = 2;

pub fn draw(frame: &mut Frame, state: &mut AppState, area: Rect, hits: &mut Hits) {
    let theme = state.theme;
    // Every spectrum knob benefits from the live preview, not just the style.
    let show_preview = matches!(
        SettingField::ALL.get(state.settings.selected),
        Some(
            SettingField::SpectrumStyle
                | SettingField::SpectrumEnabled
                | SettingField::SpectrumGlow
                | SettingField::SpectrumFlatten
                | SettingField::SpectrumDb
                | SettingField::SpectrumGradient
                | SettingField::SpectrumBars
                | SettingField::SpectrumSensitivity
                | SettingField::SpectrumStereo
        )
    );
    let preview_height = if show_preview {
        SPECTRUM_PREVIEW_HEIGHT
    } else {
        0
    };
    let preview_gap_height = if show_preview { PANEL_GAP_Y } else { 0 };
    let width = SETTINGS_PANEL_WIDTH.min(area.width);
    let height = (SettingField::ALL.len() as u16
        + SETTINGS_PANEL_CHROME_HEIGHT
        + preview_gap_height
        + preview_height)
        .min(area.height);
    let panel = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let block = panel_block(&theme, i18n::t(Key::Settings), None);
    let inner = block.inner(panel);
    frame.render_widget(block, panel);
    if inner.is_empty() {
        return;
    }

    let [hint_area, rows_area, _, preview_area, status_area, buttons_area] = Layout::vertical([
        Constraint::Length(SETTINGS_HINT_HEIGHT),
        // The row list scrolls; never let it starve the hint/preview/actions.
        Constraint::Fill(1),
        Constraint::Length(preview_gap_height),
        Constraint::Length(preview_height),
        Constraint::Length(SETTINGS_STATUS_HEIGHT),
        Constraint::Length(SETTINGS_ACTIONS_HEIGHT),
    ])
    .areas(inner);
    let hint = match SettingField::ALL.get(state.settings.selected) {
        Some(SettingField::Icons) if state.config.icons == IconStyle::Nerd => {
            match state.nerd_font_status() {
                Some(crate::nerd_font::Status::Detected) => Key::NerdFontDetectedHint,
                Some(crate::nerd_font::Status::Missing) => Key::NerdFontHint,
                Some(crate::nerd_font::Status::Unknown) | None => Key::SettingsHint,
            }
        }
        Some(SettingField::CoverDetail)
            if state.config.cover_detail == crate::pixel::CoverDetail::Octant =>
        {
            Key::OctantFontHint
        }
        Some(SettingField::PixelDetail) => Key::PixelDetailHint,
        _ => Key::SettingsHint,
    };
    // The cache row's hint is live data, not copy: what each store holds
    // against its slice of the budget.
    let hint = match SettingField::ALL.get(state.settings.selected) {
        Some(SettingField::CacheLimit) => match state.settings.cache_usage {
            Some(usage) => i18n::t_cache_usage(usage),
            None => i18n::t(hint).to_owned(),
        },
        _ => i18n::t(hint).to_owned(),
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::new().fg(theme.dim)),
        hint_area,
    );

    let visible_rows = usize::from(rows_area.height).min(SettingField::ALL.len());
    let max_offset = SettingField::ALL.len().saturating_sub(visible_rows);
    let offset = state
        .settings
        .selected
        .saturating_add(1)
        .saturating_sub(visible_rows)
        .min(max_offset);
    for (visible_index, (index, field)) in SettingField::ALL
        .iter()
        .copied()
        .enumerate()
        .skip(offset)
        .take(visible_rows)
        .enumerate()
    {
        let row = Rect {
            y: rows_area.y + visible_index as u16,
            height: 1,
            ..rows_area
        };
        let selected = index == state.settings.selected;
        let row_base = if selected {
            state.selection_style()
        } else {
            Style::new()
        };
        let marker_style = row_base.fg(if selected { theme.accent } else { theme.faint });
        let mut label_style = row_base.fg(theme.fg);
        if selected {
            label_style = label_style.add_modifier(Modifier::BOLD);
        }
        let value_style = row_base.fg(theme.dim);
        let arrow_style = row_base.fg(theme.faint);
        let label = i18n::t(field.label());
        let label_width = display_width(label);
        let value = state.setting_value(field);
        let icon_preview = (field == SettingField::Icons).then(|| {
            let icons = crate::icons::for_style(state.config.icons);
            format!("{} {} ", icons.heart, icons.heart)
        });
        let value_width = (display_width(&value)
            + icon_preview.as_deref().map(display_width).unwrap_or(0))
            as u16;
        let available = row.width as usize;
        let content_width = label_width + usize::from(value_width) + SETTINGS_ROW_CHROME_WIDTH;
        let pad = available.saturating_sub(content_width);
        let mut spans = vec![
            Span::styled(if selected { " › " } else { "   " }, marker_style),
            Span::styled(label, label_style),
            Span::styled(" ".repeat(pad), row_base),
            Span::styled("‹ ", arrow_style),
        ];
        if icon_preview.is_some() {
            let icons = crate::icons::for_style(state.config.icons);
            spans.push(Span::styled(icons.heart, row_base.fg(theme.faint)));
            spans.push(Span::styled(" ", row_base));
            spans.push(Span::styled(icons.heart, row_base.fg(theme.accent2)));
            spans.push(Span::styled(" ", row_base));
        }
        spans.push(Span::styled(value, value_style));
        spans.push(Span::styled(" › ", arrow_style));
        let line = Line::from(spans);
        frame.render_widget(Paragraph::new(line).style(row_base), row);
        hits.settings_rows.push((row, index));
        if selected && available >= content_width {
            let next = Rect::new(
                row.right().saturating_sub(SETTINGS_ARROW_WIDTH),
                row.y,
                SETTINGS_ARROW_WIDTH,
                1,
            );
            let previous_x = next
                .x
                .saturating_sub(value_width.saturating_add(SETTINGS_ARROW_WIDTH + 1));
            hits.settings_adjust
                .push((Rect::new(previous_x, row.y, SETTINGS_ARROW_WIDTH, 1), -1));
            hits.settings_adjust.push((next, 1));
        }
    }

    if show_preview && preview_area.height >= 3 {
        let preview = panel_block(&theme, i18n::t(Key::SpectrumPreview), None);
        let preview_inner = preview.inner(preview_area);
        frame.render_widget(preview, preview_area);
        state.spectrum.render(
            state.config.spectrum_style,
            state.config.spectrum_glow,
            state.spectrum_render_options(),
            preview_inner,
            frame.buffer_mut(),
            &theme,
        );
    }

    if let Some(status) = &state.status {
        let color = if status.starts_with(i18n::t(Key::SettingsSaveFailed)) {
            theme.accent2
        } else {
            theme.dim
        };
        frame.render_widget(
            Paragraph::new(status.as_str()).style(Style::new().fg(color)),
            status_area,
        );
    }

    let save = format!("[ Enter · {} ]", i18n::t(Key::Save));
    let cancel = format!("[ Esc · {} ]", i18n::t(Key::Cancel));
    let save_width = display_width(&save) as u16;
    let cancel_width = display_width(&cancel) as u16;
    let total = save_width
        .saturating_add(SETTINGS_ACTION_GAP)
        .saturating_add(cancel_width);
    let start = buttons_area.x + buttons_area.width.saturating_sub(total) / 2;
    let save_rect = Rect::new(start, buttons_area.y, save_width.min(buttons_area.width), 1);
    let cancel_rect = Rect::new(
        save_rect
            .right()
            .saturating_add(SETTINGS_ACTION_GAP)
            .min(buttons_area.right()),
        buttons_area.y,
        cancel_width.min(
            buttons_area
                .right()
                .saturating_sub(save_rect.right().saturating_add(SETTINGS_ACTION_GAP)),
        ),
        1,
    );
    frame.render_widget(
        Paragraph::new(save).style(Style::new().fg(theme.fg).bg(theme.faint)),
        save_rect,
    );
    frame.render_widget(
        Paragraph::new(cancel).style(Style::new().fg(theme.dim)),
        cancel_rect,
    );
    hits.settings_save.push(save_rect);
    hits.settings_cancel.push(cancel_rect);
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    use super::*;
    use crate::config::Config;
    use crate::spectrum::SampleBuffer;

    const SKELETON_SIZES: [(u16, u16); 3] = [(80, 24), (120, 40), (200, 60)];

    fn rendered_settings(
        width: u16,
        height: u16,
        selected: usize,
    ) -> (ratatui::buffer::Buffer, Hits) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.settings.selected = selected;
        let mut hits = Hits::default();
        terminal
            .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();
        (terminal.backend().buffer().clone(), hits)
    }

    fn symbol_position(buffer: &Buffer, symbol: &str) -> (u16, u16) {
        let area = buffer.area;
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if buffer[(x, y)].symbol() == symbol {
                    return (x, y);
                }
            }
        }
        panic!("missing symbol {symbol}");
    }

    fn all_hits_stay_inside(hits: &Hits, width: u16, height: u16) -> bool {
        hits.settings_rows
            .iter()
            .map(|(area, _)| area)
            .chain(hits.settings_adjust.iter().map(|(area, _)| area))
            .chain(hits.settings_save.iter())
            .chain(hits.settings_cancel.iter())
            .all(|area| area.x <= width && area.y < height && area.right() <= width)
    }

    #[test]
    fn arrow_hits_cover_the_drawn_glyphs() {
        let (buffer, hits) = rendered_settings(80, 24, 0);
        let (previous, _) = hits
            .settings_adjust
            .iter()
            .find(|(_, delta)| *delta < 0)
            .unwrap();
        let (next, _) = hits
            .settings_adjust
            .iter()
            .find(|(_, delta)| *delta > 0)
            .unwrap();

        assert_eq!(buffer[(previous.x, previous.y)].symbol(), "‹");
        assert_eq!(buffer[(next.x, next.y)].symbol(), "›");
    }

    #[test]
    fn settings_panel_is_complete_centered_and_bounded_at_all_skeleton_sizes() {
        for (width, height) in SKELETON_SIZES {
            let (buffer, hits) = rendered_settings(width, height, 0);
            let top_left = symbol_position(&buffer, "╭");
            let top_right = symbol_position(&buffer, "╮");
            let bottom_left = symbol_position(&buffer, "╰");
            let bottom_right = symbol_position(&buffer, "╯");
            let panel_width = SETTINGS_PANEL_WIDTH.min(width);

            assert_eq!(top_right.0 - top_left.0 + 1, panel_width);
            assert_eq!(top_left.1, top_right.1);
            assert_eq!(bottom_left.1, bottom_right.1);
            assert_eq!(top_left.0, bottom_left.0);
            assert_eq!(top_right.0, bottom_right.0);
            assert!(top_left.0.abs_diff(width - top_right.0 - 1) <= 1);
            assert!(top_left.1.abs_diff(height - bottom_left.1 - 1) <= 1);
            assert_eq!(buffer[top_left].fg, crate::theme::Theme::db16().faint);
            assert!((top_left.0..=top_right.0)
                .any(|x| buffer[(x, top_left.1)].fg == crate::theme::Theme::db16().accent));
            assert!(all_hits_stay_inside(&hits, width, height));
            assert!(hits
                .settings_rows
                .iter()
                .all(|(row, _)| { row.x == top_left.0 + 2 && row.right() == top_right.0 - 1 }));
        }
    }

    #[test]
    fn selected_setting_bolds_only_its_label_and_keeps_controls_quiet() {
        let (buffer, hits) = rendered_settings(80, 24, 0);
        let row = hits
            .settings_rows
            .iter()
            .find_map(|(area, index)| (*index == 0).then_some(*area))
            .unwrap();
        let previous = hits
            .settings_adjust
            .iter()
            .find_map(|(area, delta)| (*delta < 0).then_some(*area))
            .unwrap();
        let next = hits
            .settings_adjust
            .iter()
            .find_map(|(area, delta)| (*delta > 0).then_some(*area))
            .unwrap();
        let theme = crate::theme::Theme::db16();
        let selection_bg = theme.selection_style(None).bg.unwrap();
        let label = &buffer[(row.x + 3, row.y)];
        let value = &buffer[(previous.right(), row.y)];

        assert_eq!(label.bg, selection_bg);
        assert_eq!(label.fg, theme.fg);
        assert!(label.modifier.contains(Modifier::BOLD));
        assert_eq!(value.fg, theme.dim);
        assert!(!value.modifier.contains(Modifier::BOLD));
        for arrow in [previous, next] {
            let cell = &buffer[(arrow.x, arrow.y)];
            assert_eq!(cell.fg, theme.faint);
            assert_eq!(cell.bg, selection_bg);
            assert!(!cell.modifier.contains(Modifier::BOLD));
        }
        let mut column = row.x + 3;
        let mut wide_continuations = Vec::new();
        for character in i18n::t(SettingField::Theme.label()).chars() {
            let character_width = display_width(&character.to_string()) as u16;
            wide_continuations.extend(column + 1..column + character_width);
            column += character_width;
        }
        assert!((row.x..row.right())
            .all(|x| { wide_continuations.contains(&x) || buffer[(x, row.y)].bg == selection_bg }));

        let save = hits.settings_save[0];
        assert_eq!(buffer[(save.x, save.y)].bg, theme.faint);
        assert_eq!(buffer[(save.x, save.y)].fg, theme.fg);
    }

    #[test]
    fn settings_status_reserves_accent2_for_errors() {
        let cases = [
            ("✓ saved".to_owned(), "✓", crate::theme::Theme::db16().dim),
            (
                format!("{} §", i18n::t(Key::SettingsSaveFailed)),
                "§",
                crate::theme::Theme::db16().accent2,
            ),
        ];
        for (status, marker, expected) in cases {
            let backend = TestBackend::new(80, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut state = AppState::new(&Config::default());
            state.status = Some(status);
            let mut hits = Hits::default();
            terminal
                .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
                .unwrap();

            let cell = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .find(|cell| cell.symbol() == marker)
                .unwrap();
            assert_eq!(cell.fg, expected);
        }
    }

    #[test]
    fn a_short_terminal_scrolls_the_selected_setting_into_view() {
        let (_buffer, hits) = rendered_settings(60, 12, SettingField::ALL.len() - 1);

        assert!(hits
            .settings_rows
            .iter()
            .any(|(_, index)| *index == SettingField::ALL.len() - 1));
    }

    #[test]
    fn a_clipped_row_does_not_register_invisible_arrow_hits() {
        let (_buffer, hits) = rendered_settings(16, 24, 0);

        assert!(hits.settings_adjust.is_empty());
    }

    #[test]
    fn selected_nerd_icons_show_the_install_hint_when_the_font_is_missing() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let config = Config {
            icons: IconStyle::Nerd,
            ..Config::default()
        };
        let mut state = AppState::new(&config);
        state.apply_nerd_font_probe(crate::nerd_font::Status::Missing);
        state.settings.selected = SettingField::ALL
            .iter()
            .position(|field| *field == SettingField::Icons)
            .unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Nerd Font"));
        assert!(rendered.contains("brew install font-symbols-only-nerd-font"));
    }

    #[test]
    fn selected_nerd_icons_show_the_detected_hint() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let config = Config {
            icons: IconStyle::Nerd,
            ..Config::default()
        };
        let mut state = AppState::new(&config);
        state.apply_nerd_font_probe(crate::nerd_font::Status::Detected);
        state.settings.selected = SettingField::ALL
            .iter()
            .position(|field| *field == SettingField::Icons)
            .unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("✓"));
        assert!(rendered.contains("Nerd Font"));
        assert!(!rendered.contains("brew install"));
    }

    #[test]
    fn an_unknown_nerd_font_status_keeps_the_general_settings_hint() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let config = Config {
            icons: IconStyle::Nerd,
            ..Config::default()
        };
        let mut state = AppState::new(&config);
        state.apply_nerd_font_probe(crate::nerd_font::Status::Unknown);
        state.settings.selected = SettingField::ALL
            .iter()
            .position(|field| *field == SettingField::Icons)
            .unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("j/k"));
        assert!(!rendered.contains("✓"));
        assert!(!rendered.contains("brew install"));
    }

    #[test]
    fn selected_octant_marks_and_explains_font_support() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let config = Config {
            cover_detail: crate::pixel::CoverDetail::Octant,
            ..Config::default()
        };
        let mut state = AppState::new(&config);
        state.settings.selected = SettingField::ALL
            .iter()
            .position(|field| *field == SettingField::CoverDetail)
            .unwrap();
        assert!(state
            .setting_value(SettingField::CoverDetail)
            .contains(i18n::t(Key::OctantFontRequired)));
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("octant"));
        assert!(rendered.contains("Unicode 16"));
        assert!(rendered.contains("sextant"));
    }

    #[test]
    fn icon_setting_previews_unliked_and_liked_with_the_same_glyph() {
        let index = SettingField::ALL
            .iter()
            .position(|field| *field == SettingField::Icons)
            .unwrap();
        let (buffer, hits) = rendered_settings(80, 24, index);
        let row = hits
            .settings_rows
            .iter()
            .find_map(|(area, field)| (*field == index).then_some(*area))
            .unwrap();
        let hearts = (row.x..row.right())
            .filter_map(|x| {
                let cell = &buffer[(x, row.y)];
                (cell.symbol() == "♥").then_some(cell.fg)
            })
            .collect::<Vec<_>>();

        assert_eq!(
            hearts,
            [
                crate::theme::Theme::db16().faint,
                crate::theme::Theme::db16().accent2
            ]
        );
    }

    #[test]
    fn spectrum_style_row_embeds_an_animated_preview() {
        let backend = TestBackend::new(80, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.settings.selected = SettingField::ALL
            .iter()
            .position(|field| *field == SettingField::SpectrumStyle)
            .unwrap();
        state
            .spectrum
            .tick(&SampleBuffer::default(), false, true, true, false, true);
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        assert!(terminal.backend().buffer().content().iter().any(|cell| {
            matches!(cell.symbol(), "▁" | "▂" | "▃" | "▄" | "▅" | "▆" | "▇" | "█")
        }));

        let buffer = terminal.backend().buffer();
        let last_row_y = hits
            .settings_rows
            .iter()
            .map(|(area, _)| area.y)
            .max()
            .unwrap();
        let nested_top_y = (0..buffer.area.height)
            .filter(|y| *y > last_row_y)
            .find(|y| (0..buffer.area.width).any(|x| buffer[(x, *y)].symbol() == "╭"))
            .unwrap();
        let nested_left_x = (0..buffer.area.width)
            .find(|x| buffer[(*x, nested_top_y)].symbol() == "╭")
            .unwrap();
        let nested_right_x = (nested_left_x..buffer.area.width)
            .find(|x| buffer[(*x, nested_top_y)].symbol() == "╮")
            .unwrap();
        let nested_bottom_y = (nested_top_y + 1..buffer.area.height)
            .find(|y| buffer[(nested_left_x, *y)].symbol() == "╰")
            .unwrap();
        assert!(nested_top_y > last_row_y + PANEL_GAP_Y);
        assert_eq!(nested_bottom_y - nested_top_y + 1, SPECTRUM_PREVIEW_HEIGHT);
        assert_eq!(nested_right_x - nested_left_x + 1, SETTINGS_PANEL_WIDTH - 4);
        assert!((0..buffer.area.width).any(|x| {
            let cell = &buffer[(x, nested_top_y)];
            cell.fg == state.theme.accent && !matches!(cell.symbol(), " " | "─" | "╭" | "╮")
        }));
    }

    #[test]
    fn the_cache_row_hint_shows_live_usage_once_probed() {
        let mut state = AppState::new(&Config::default());
        state.view = crate::action::View::Settings;
        let cache_row = SettingField::ALL
            .iter()
            .position(|field| *field == SettingField::CacheLimit)
            .unwrap();
        state.settings.selected = cache_row;
        state.settings.cache_usage = Some(crate::action::CacheUsage {
            audio_used: 6 * 1024 * 1024 * 1024,
            audio_max: 15 * 1024 * 1024 * 1024,
            cover_used: 135 * 1024 * 1024,
            cover_max: 655 * 1024 * 1024,
        });
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();
        terminal
            .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(text.contains("6.0G"), "audio usage must be on screen");
        assert!(text.contains("135M"), "cover usage must be on screen");
    }
}
