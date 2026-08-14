use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::app::AppState;
use crate::i18n::{self, Key};

use super::{panel_block, scroll_offset, text};

pub(crate) const WIDTH: u16 = 60;
const MAX_HEIGHT: u16 = 20;

/// Scrim behind the modal. Pixel-art cells carry image data in BOTH fg and
/// bg, so dimming must pull each channel toward the theme background;
/// overwriting fg alone shatters covers into speckle noise. Non-RGB colors
/// (e.g. the transparent theme's Reset background) keep the legacy
/// faint-foreground treatment.
pub(crate) fn dim_background(frame: &mut Frame, theme: &crate::theme::Theme) {
    for cell in &mut frame.buffer_mut().content {
        match (mix_toward(cell.fg, theme.bg), mix_toward(cell.bg, theme.bg)) {
            (Some(fg), bg) => {
                cell.fg = fg;
                if let Some(bg) = bg {
                    cell.bg = bg;
                }
            }
            _ => cell.fg = theme.faint,
        }
    }
}

/// Keep 40% of the color, sink 60% into the base.
fn mix_toward(
    color: ratatui::style::Color,
    base: ratatui::style::Color,
) -> Option<ratatui::style::Color> {
    use ratatui::style::Color;
    let (Color::Rgb(red, green, blue), Color::Rgb(base_red, base_green, base_blue)) = (color, base)
    else {
        return None;
    };
    let mix = |channel: u8, base: u8| ((u16::from(channel) * 2 + u16::from(base) * 3) / 5) as u8;
    Some(Color::Rgb(
        mix(red, base_red),
        mix(green, base_green),
        mix(blue, base_blue),
    ))
}

pub(crate) fn draw(frame: &mut Frame, state: &AppState, area: Rect) {
    if area.is_empty() {
        return;
    }
    let theme = &state.theme;
    let commands = state.command_palette.filtered();
    let width = WIDTH.min(area.width);
    let height = MAX_HEIGHT.min(area.height);
    let modal = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );

    frame.render_widget(Clear, modal);
    let block = panel_block(
        theme,
        i18n::t(Key::CommandPalette),
        Some(i18n::t_result_count(commands.len())),
    )
    .style(Style::new().bg(theme.bg));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);
    if inner.is_empty() {
        return;
    }

    let [input_area, _, list_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    draw_input(frame, state, input_area);
    draw_commands(frame, state, &commands, list_area);

    let hint = state
        .command_feedback
        .as_deref()
        .unwrap_or_else(|| i18n::t(Key::CommandPaletteHint));
    let hint_color = if state.command_feedback_error {
        theme.accent2
    } else if state.command_feedback.is_some() {
        theme.accent
    } else {
        theme.dim
    };
    frame.render_widget(
        Paragraph::new(text::pad_display(hint, usize::from(hint_area.width)))
            .style(Style::new().fg(hint_color)),
        hint_area,
    );
}

fn draw_input(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = &state.theme;
    // Anchor IME candidate windows to the caret via the real cursor.
    let caret = 2 + text::display_width(&state.command_palette.query) as u16;
    frame.set_cursor_position((area.x + caret.min(area.width.saturating_sub(1)), area.y));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(": ", Style::new().fg(theme.accent)),
            Span::styled(
                state.command_palette.query.as_str(),
                Style::new().fg(theme.fg),
            ),
            Span::styled("▎", Style::new().fg(theme.accent)),
        ]))
        .style(state.selection_style()),
        area,
    );
}

fn draw_commands(
    frame: &mut Frame,
    state: &AppState,
    commands: &[&crate::app::command_palette::CommandSpec],
    area: Rect,
) {
    let theme = &state.theme;
    if commands.is_empty() {
        frame.render_widget(
            Paragraph::new(i18n::t(Key::CommandNoMatches)).style(Style::new().fg(theme.dim)),
            area,
        );
        return;
    }

    let visible = usize::from(area.height);
    let offset = scroll_offset(state.command_palette.selected, commands.len(), visible);
    for (row, command) in commands.iter().skip(offset).take(visible).enumerate() {
        let index = offset + row;
        let selected = index == state.command_palette.selected;
        let base = if selected {
            state.selection_style()
        } else {
            Style::new().bg(theme.bg)
        };
        let marker_width = 2_usize.min(usize::from(area.width));
        let content_width = usize::from(area.width).saturating_sub(marker_width);
        let alias = i18n::t(command.alias);
        let alias_width = text::display_width(alias).min(18).min(content_width / 2);
        let usage_width = content_width.saturating_sub(alias_width);
        let marker = if selected { "› " } else { "  " };
        let line = Line::from(vec![
            Span::styled(
                text::pad_display(marker, marker_width),
                base.fg(if selected { theme.accent } else { theme.faint }),
            ),
            Span::styled(
                text::pad_display(command.usage, usage_width),
                base.fg(if selected { theme.fg } else { theme.dim }),
            ),
            Span::styled(
                text::pad_display_right(alias, alias_width),
                base.fg(if selected { theme.dim } else { theme.faint }),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, area.y + row as u16, area.width, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::config::Config;
    use crate::ui::{self, Hits};

    #[test]
    fn palette_is_centered_sixty_columns_and_dims_only_the_background() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.command_palette.open();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| ui::draw(frame, &mut state, &mut hits))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let modal_x = (100 - WIDTH) / 2;
        let modal_y = (30 - MAX_HEIGHT) / 2;

        assert_eq!(buffer[(modal_x, modal_y)].symbol(), "╭");
        assert_eq!(buffer[(modal_x + WIDTH - 1, modal_y)].symbol(), "╮");
        assert_ne!(buffer[(0, 0)].fg, state.theme.fg, "shell text is scrimmed");
        let title_start = i18n::t(Key::CommandPalette)
            .chars()
            .next()
            .unwrap()
            .to_string();
        let title_x = (modal_x..modal_x + WIDTH)
            .find(|x| buffer[(*x, modal_y)].symbol() == title_start)
            .unwrap();
        assert_eq!(buffer[(title_x, modal_y)].fg, state.theme.accent);
    }

    #[test]
    fn scrim_dims_pixel_cells_in_both_channels_instead_of_flattening_fg() {
        use ratatui::style::Color;

        let theme = crate::theme::Theme::db16();
        let backend = TestBackend::new(4, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let pixel = frame.buffer_mut().cell_mut((0, 0)).unwrap();
                pixel
                    .set_fg(Color::Rgb(200, 100, 50))
                    .set_bg(Color::Rgb(20, 40, 60));
                dim_background(frame, &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let Color::Rgb(base_red, base_green, base_blue) = theme.bg else {
            panic!("db16 background must be RGB");
        };
        let mix =
            |channel: u8, base: u8| ((u16::from(channel) * 2 + u16::from(base) * 3) / 5) as u8;
        assert_eq!(
            buffer[(0, 0)].fg,
            Color::Rgb(mix(200, base_red), mix(100, base_green), mix(50, base_blue)),
            "pixel foreground blends toward the theme background"
        );
        assert_eq!(
            buffer[(0, 0)].bg,
            Color::Rgb(mix(20, base_red), mix(40, base_green), mix(60, base_blue)),
            "pixel background dims too, keeping the mosaic coherent"
        );
        // Untouched default cells (Reset fg) keep the legacy faint scrim.
        assert_eq!(buffer[(1, 0)].fg, theme.faint);
    }

    #[test]
    fn chinese_filter_and_selected_row_are_projected() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.command_palette.open();
        state.command_palette.paste("主题");
        let mut hits = Hits::default();

        terminal
            .draw(|frame| ui::draw(frame, &mut state, &mut hits))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let compact = rendered
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();

        assert!(rendered.contains("theme <name>"));
        // The alias column follows the active locale.
        let alias_compact = i18n::t(Key::CommandTheme)
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        assert!(compact.contains(&alias_compact));
        assert!(!rendered.contains("volume <0-100>"));
    }

    #[test]
    fn mini_player_still_renders_the_modal_and_footer_feedback_is_temporary_slot_content() {
        let backend = TestBackend::new(80, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.command_palette.open();
        let mut hits = Hits::default();
        terminal
            .draw(|frame| ui::draw(frame, &mut state, &mut hits))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[((80 - WIDTH) / 2, 0)].symbol(), "╭");
        assert_ne!(buffer[(0, 0)].fg, state.theme.fg, "shell text is scrimmed");

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        state.command_palette.close();
        state.command_feedback = Some("Command executed: next".into());
        terminal
            .draw(|frame| ui::draw(frame, &mut state, &mut hits))
            .unwrap();
        let footer = (0..80)
            .map(|x| terminal.backend().buffer()[(x, 23)].symbol())
            .collect::<String>();
        assert!(footer.contains("Command executed: next"));

        state.command_feedback = None;
        terminal
            .draw(|frame| ui::draw(frame, &mut state, &mut hits))
            .unwrap();
        let footer = (0..80)
            .map(|x| terminal.backend().buffer()[(x, 23)].symbol())
            .collect::<String>();
        assert!(footer.contains("Space"));
    }
}
