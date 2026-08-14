//! Pure view layer: reads AppState, writes the frame. No side effects.

mod command_palette;
pub(crate) mod cover_preview;
pub(crate) mod library;
mod login;
pub(crate) mod mini_player;
pub(crate) mod now_playing;
pub(crate) mod queue;
pub(crate) mod search;
mod settings;
pub(crate) mod text;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};
use ratatui::Frame;
use yesplaymusic_core::cache::AudioQuality;

use crate::action::View;
use crate::app::{AppState, PlaybackModeSlot};
use crate::i18n::{self, Key};
use crate::theme::Theme;

/// Wider terminals use a fixed reading measure instead of stretching every view.
pub(crate) const WIDE_LAYOUT_THRESHOLD: u16 = 120;
/// The shared large-screen canvas keeps related content within one visual group.
pub(crate) const MAX_CONTENT_WIDTH: u16 = 110;
/// The navigation bar occupies one terminal row.
pub(crate) const HEADER_HEIGHT: u16 = 1;
/// The contextual help bar occupies one terminal row.
pub(crate) const FOOTER_HEIGHT: u16 = 1;
/// Separate adjacent panels by two columns so their borders do not merge.
pub(crate) const PANEL_GAP_X: u16 = 2;
/// Separate vertically stacked panels by one row.
pub(crate) const PANEL_GAP_Y: u16 = 1;
/// Panel content keeps one blank cell inside each vertical border.
pub(crate) const PANEL_PADDING_X: u16 = 1;
/// Remove both borders and horizontal padding before sizing table columns.
pub(crate) const fn panel_inner_width(width: u16) -> usize {
    width.saturating_sub(2 + PANEL_PADDING_X * 2) as usize
}
/// Keep the account status visually separate from the final tab.
pub(crate) const HEADER_GAP: u16 = 2;
/// Inactive labels and the active tab chip use the same horizontal rhythm.
pub(crate) const TAB_GAP: u16 = 2;
/// Footer key-and-description groups stay distinct without bracket decoration.
pub(crate) const FOOTER_ITEM_GAP: u16 = 2;
/// Reserve breathing room between contextual hints and right-aligned badges.
pub(crate) const FOOTER_STATUS_GAP: u16 = 2;
/// Help stays readable without stretching across a wide terminal.
const HELP_MODAL_WIDTH: u16 = 72;
/// Border rows plus the closing hint determine help-panel chrome height.
const HELP_CHROME_HEIGHT: u16 = 3;
/// Help keycaps share a stable column before their dim descriptions.
const HELP_KEY_COLUMN_WIDTH: usize = 30;
/// The quit question and its two compact actions fit this focused dialog width.
const QUIT_MODAL_WIDTH: u16 = 34;
/// Two borders, the question, breathing room, and actions form the quit dialog.
const QUIT_MODAL_HEIGHT: u16 = 5;
/// Separate destructive confirmation from its cancel action.
const QUIT_ACTION_GAP: u16 = 3;

/// Shared list-panel language: rounded faint frame, accent title, dim detail.
pub(crate) fn panel_block(
    theme: &Theme,
    title: impl Into<String>,
    detail: Option<String>,
) -> Block<'static> {
    let mut title_spans = vec![Span::styled(
        format!(" {} ", title.into()),
        Style::new().fg(theme.accent),
    )];
    if let Some(detail) = detail.filter(|detail| !detail.is_empty()) {
        title_spans.push(Span::styled(
            format!("· {detail} "),
            Style::new().fg(theme.dim),
        ));
    }
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.faint))
        .padding(Padding::horizontal(PANEL_PADDING_X))
        .title(Line::from(title_spans))
}

/// Television-style filter control embedded in the lower panel border.
pub(crate) fn filter_title(state: &AppState) -> Line<'static> {
    let theme = &state.theme;
    if !state.filter.is_active() {
        return Line::from(Span::styled(
            format!(" / {} ", i18n::t(Key::Filter)),
            Style::new().fg(theme.dim),
        ))
        .right_aligned();
    }

    let cursor = if state.filter.input { "▎" } else { "" };
    let action = if state.filter.input {
        Key::FinishFilter
    } else {
        Key::Play
    };
    Line::from(vec![
        Span::styled(" / ", Style::new().fg(theme.accent)),
        Span::styled(
            format!("{}{cursor}", state.filter.query),
            Style::new().fg(theme.fg),
        ),
        Span::styled(
            format!(
                " · Enter {} · Esc {} ",
                i18n::t(action),
                i18n::t(Key::ClearFilter)
            ),
            Style::new().fg(theme.dim),
        ),
    ])
    .right_aligned()
}

/// Constrain only genuinely wide terminals; 120 columns remains a full-width layout.
pub(crate) const fn centered_content(area: Rect) -> Rect {
    if area.width <= WIDE_LAYOUT_THRESHOLD {
        return area;
    }
    let width = if area.width < MAX_CONTENT_WIDTH {
        area.width
    } else {
        MAX_CONTENT_WIDTH
    };
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        width,
        ..area
    }
}

/// Geometry recorded at draw time so mouse events can be resolved
/// against what is actually on screen.
#[derive(Default)]
pub struct Hits {
    pub tabs: Vec<(Rect, View)>,
    pub search_channels: Vec<(Rect, crate::api::SearchChannel)>,
    pub rows: Vec<(Rect, usize)>,
    pub menu: Vec<(Rect, crate::action::MenuEntry)>,
    pub sidebar: Vec<(Rect, usize)>,
    pub heart: Vec<(Rect, ())>,
    pub play: Vec<(Rect, ())>,
    pub playback_mode: Vec<(Rect, ())>,
    pub volume: Vec<(Rect, ())>,
    /// Quit-confirm buttons: true = 确定退出, false = 点错了.
    pub confirm: Vec<(Rect, bool)>,
    pub settings_rows: Vec<(Rect, usize)>,
    pub settings_adjust: Vec<(Rect, i32)>,
    pub settings_save: Vec<Rect>,
    pub settings_cancel: Vec<Rect>,
}

pub fn draw(frame: &mut Frame, state: &mut AppState, hits: &mut Hits) {
    hits.tabs.clear();
    hits.search_channels.clear();
    hits.rows.clear();
    hits.menu.clear();
    hits.sidebar.clear();
    hits.heart.clear();
    hits.play.clear();
    hits.playback_mode.clear();
    hits.volume.clear();
    hits.confirm.clear();
    hits.settings_rows.clear();
    hits.settings_adjust.clear();
    hits.settings_save.clear();
    hits.settings_cancel.clear();

    let theme = &state.theme;
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::new().bg(theme.bg)), area);

    if area.height < 8 {
        mini_player::draw(frame, state, area, hits);
    } else {
        let content = centered_content(area);
        if state.zen {
            now_playing::draw(frame, state, content, hits);
        } else {
            let [tabs_area, _, body, _, hints_area] = Layout::vertical([
                Constraint::Length(HEADER_HEIGHT),
                Constraint::Length(PANEL_GAP_Y),
                Constraint::Min(0),
                Constraint::Length(PANEL_GAP_Y),
                Constraint::Length(FOOTER_HEIGHT),
            ])
            .areas(content);

            draw_header(frame, state, tabs_area, hits);
            match state.view {
                View::NowPlaying => now_playing::draw(frame, state, body, hits),
                View::Library => library::draw(frame, state, body, hits),
                View::Search => search::draw(frame, state, body, hits),
                View::Queue => queue::draw(frame, state, body, hits),
                View::Login => login::draw(frame, state, body),
                View::Settings => settings::draw(frame, state, body, hits),
            }
            draw_hints(frame, state, hints_area);
        }

        if state.show_help {
            draw_help(frame, state, area);
        }
        if state.confirm_quit {
            draw_quit_confirm(frame, state, area, hits);
        }
    }

    if state.command_feedback.is_some() && (area.height < 8 || state.zen) {
        let feedback_area = if area.height < 8 {
            Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1)
        } else {
            let content = centered_content(area);
            Rect::new(
                content.x,
                content.bottom().saturating_sub(1),
                content.width,
                1,
            )
        };
        draw_command_feedback(frame, state, feedback_area);
    }

    if state.command_palette.open {
        command_palette::dim_background(frame, &state.theme);
        command_palette::draw(frame, state, area);
    }
}

fn draw_command_feedback(frame: &mut Frame, state: &AppState, area: Rect) {
    let Some(feedback) = &state.command_feedback else {
        return;
    };
    frame.render_widget(
        Paragraph::new(text::pad_display(feedback, usize::from(area.width))).style(
            Style::new()
                .fg(if state.command_feedback_error {
                    state.theme.accent2
                } else {
                    state.theme.accent
                })
                .bg(state.theme.bg),
        ),
        area,
    );
}

fn draw_help(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = &state.theme;
    let rows: Vec<(&str, Key)> = vec![
        ("1-5", Key::NowPlaying),
        ("j/k ↑/↓", Key::Select),
        ("PgUp/PgDn", Key::Page),
        ("Home/End · gg/G", Key::TopBottom),
        ("l/Enter", Key::Play),
        ("a", Key::AddToQueue),
        ("h/Esc", Key::Back),
        ("Tab · ⇧Tab/←/→ (Search)", Key::LibraryFocus),
        ("Space", Key::Pause),
        ("n / p", Key::ChangeTrack),
        ("←/→ 5s · Shift+←/→ 30s", Key::Seek),
        ("- / +", Key::Volume),
        ("m", Key::Mute),
        ("s", Key::Shuffle),
        ("r", Key::Repeat),
        ("*", Key::LabelLike),
        ("/", Key::Filter),
        ("z", Key::Zen),
        ("v", Key::Spectrum),
        ("?", Key::LabelHelp),
        (",", Key::Settings),
        ("q", Key::Quit),
    ];
    let height = (rows.len() as u16 + HELP_CHROME_HEIGHT).min(area.height);
    let width = HELP_MODAL_WIDTH.min(area.width);
    let modal = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(ratatui::widgets::Clear, modal);
    let block = panel_block(theme, i18n::t(Key::HelpTitle), None)
        .style(Style::new().bg(theme.bg))
        .border_style(Style::new().fg(theme.faint));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    let mut lines = Vec::new();
    for (keys, label) in rows {
        let keycap = format!(" {keys} ");
        let gap = HELP_KEY_COLUMN_WIDTH.saturating_sub(text::display_width(&keycap));
        lines.push(Line::from(vec![
            Span::styled(keycap, Style::new().fg(theme.fg).bg(theme.faint)),
            Span::raw(" ".repeat(gap)),
            Span::styled(i18n::t(label), Style::new().fg(theme.dim)),
        ]));
    }
    lines.push(Line::from(Span::styled(
        format!("  {}", i18n::t(Key::HelpAnyKey)),
        Style::new().fg(theme.faint),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

const TABS: [(&str, Key, View); 5] = [
    ("1", Key::NowPlaying, View::NowPlaying),
    ("2", Key::Library, View::Library),
    ("3", Key::Search, View::Search),
    ("4", Key::Queue, View::Queue),
    ("5", Key::Settings, View::Settings),
];

fn draw_header(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    let theme = &state.theme;
    let tabs = TABS.map(|(number, label, view)| {
        let active = state.view == view;
        let label = if active {
            format!(" {number} {} ", i18n::t(label))
        } else {
            format!("{number} {}", i18n::t(label))
        };
        (label, view, active)
    });
    let tabs_width = tabs
        .iter()
        .map(|(label, _, _)| text::display_width(label) as u16)
        .sum::<u16>()
        .saturating_add(TAB_GAP.saturating_mul(tabs.len().saturating_sub(1) as u16));

    let account = match &state.session.nickname {
        Some(nickname) => format!("♪ {nickname}"),
        None => format!("♪ {}", i18n::t(Key::SignedOut)),
    };
    let account_available = area
        .width
        .saturating_sub(tabs_width)
        .saturating_sub(HEADER_GAP);
    let account_width = text::display_width(&account).min(usize::from(account_available)) as u16;
    let account_x = area.right().saturating_sub(account_width);
    if account_width > 0 {
        frame.render_widget(
            Paragraph::new(text::pad_display(&account, usize::from(account_width)))
                .style(Style::new().fg(theme.dim)),
            Rect::new(account_x, area.y, account_width, area.height),
        );
    }

    let tabs_right = if account_width > 0 {
        account_x.saturating_sub(HEADER_GAP)
    } else {
        area.right()
    };
    let mut x = area.x;
    for (label, view, active) in tabs {
        let width = text::display_width(&label) as u16;
        if x.saturating_add(width) > tabs_right {
            break;
        }
        let style = if active {
            Style::new().fg(theme.selection_fg()).bg(theme.accent)
        } else {
            Style::new().fg(theme.dim)
        };
        let tab_area = Rect::new(x, area.y, width, area.height);
        frame.render_widget(Paragraph::new(label).style(style), tab_area);
        hits.tabs.push((tab_area, view));
        x = x.saturating_add(width).saturating_add(TAB_GAP);
    }
}

fn draw_quit_confirm(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    let theme = &state.theme;
    let width = QUIT_MODAL_WIDTH.min(area.width);
    let height = QUIT_MODAL_HEIGHT.min(area.height);
    let modal = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(ratatui::widgets::Clear, modal);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .style(Style::new().bg(theme.bg))
        .border_style(Style::new().fg(theme.faint));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            i18n::t(Key::QuitQuestion),
            Style::new().fg(theme.fg),
        )))
        .centered(),
        Rect { height: 1, ..inner },
    );

    let confirm_label = format!("[ y {} ]", i18n::t(Key::Quit));
    let cancel_label = format!("[ n {} ]", i18n::t(Key::Cancel));
    let gap = QUIT_ACTION_GAP;
    let confirm_width = text::display_width(&confirm_label) as u16;
    let cancel_width = text::display_width(&cancel_label) as u16;
    let total = confirm_width + gap + cancel_width;
    let buttons_y = inner.y + inner.height.saturating_sub(1);
    let start_x = inner.x + (inner.width.saturating_sub(total)) / 2;

    let confirm_rect = Rect {
        x: start_x,
        y: buttons_y,
        width: confirm_width,
        height: 1,
    };
    let cancel_rect = Rect {
        x: start_x + confirm_width + gap,
        y: buttons_y,
        width: cancel_width,
        height: 1,
    };
    hits.confirm.push((confirm_rect, true));
    hits.confirm.push((cancel_rect, false));

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            confirm_label,
            Style::new().fg(theme.selection_fg()).bg(theme.accent),
        ))),
        confirm_rect,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            cancel_label,
            Style::new().fg(theme.dim),
        ))),
        cancel_rect,
    );
}

fn draw_hints(frame: &mut Frame, state: &AppState, area: Rect) {
    let (badges, badges_width) = footer_badges(state, area.width);
    let hints_width = if badges_width == 0 {
        area.width
    } else {
        area.width
            .saturating_sub(badges_width)
            .saturating_sub(FOOTER_STATUS_GAP)
    };
    let status = Rect::new(area.x, area.y, hints_width, area.height);
    if state.command_feedback.is_some() {
        draw_command_feedback(frame, state, status);
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(footer_hint_spans(state, hints_width))),
            status,
        );
    }
    if badges_width > 0 {
        frame.render_widget(
            Paragraph::new(badges),
            Rect::new(
                area.right().saturating_sub(badges_width),
                area.y,
                badges_width,
                area.height,
            ),
        );
    }
}

fn footer_hint_spans(state: &AppState, max_width: u16) -> Vec<Span<'static>> {
    let theme = &state.theme;
    let mut spans = Vec::new();
    let mut used = 0_u16;
    for (key, label) in footer_hints(state) {
        let key = format!(" {key} ");
        let description = format!(" {}", i18n::t(*label));
        let gap = if used == 0 { 0 } else { FOOTER_ITEM_GAP };
        let item_width = (text::display_width(&key) + text::display_width(&description)) as u16;
        if used.saturating_add(gap).saturating_add(item_width) > max_width {
            break;
        }
        if gap > 0 {
            spans.push(Span::raw(" ".repeat(usize::from(gap))));
        }
        spans.push(Span::styled(key, Style::new().fg(theme.fg).bg(theme.faint)));
        spans.push(Span::styled(description, Style::new().fg(theme.dim)));
        used = used.saturating_add(gap).saturating_add(item_width);
    }
    spans
}

fn footer_badges(state: &AppState, max_width: u16) -> (Line<'static>, u16) {
    let theme = &state.theme;
    let icons = crate::icons::for_style(state.config.icons);
    let quality = match state.config.quality {
        AudioQuality::Low128 => "128 kbps",
        AudioQuality::Medium192 => "192 kbps",
        AudioQuality::High320 => "320 kbps",
        AudioQuality::Lossless => "Lossless",
        AudioQuality::HiRes => "Hi-Res",
    };
    let mode = match PlaybackModeSlot::from_parts(state.shuffle, state.play_mode) {
        PlaybackModeSlot::Sequential => icons.sequential,
        PlaybackModeSlot::RepeatList => icons.repeat_list,
        PlaybackModeSlot::RepeatOne => icons.repeat_one,
        PlaybackModeSlot::Shuffle => icons.shuffle,
    };
    let quality = format!(" {quality} ");
    let mode = format!(" {mode} ");
    let quality_width = text::display_width(&quality) as u16;
    let mode_width = text::display_width(&mode) as u16;
    let both_width = quality_width
        .saturating_add(FOOTER_ITEM_GAP)
        .saturating_add(mode_width);
    let badge_style = Style::new().fg(theme.fg).bg(theme.faint);

    if both_width <= max_width {
        return (
            Line::from(vec![
                Span::styled(quality, badge_style),
                Span::raw(" ".repeat(usize::from(FOOTER_ITEM_GAP))),
                Span::styled(mode, badge_style),
            ]),
            both_width,
        );
    }
    if mode_width <= max_width {
        return (Line::from(Span::styled(mode, badge_style)), mode_width);
    }
    (Line::default(), 0)
}

const PLAYBACK_HINTS: &[(&str, Key)] = &[
    ("Space", Key::Pause),
    ("←/→", Key::Seek),
    ("*", Key::LabelLike),
    ("v", Key::Spectrum),
    ("q", Key::Quit),
    ("?", Key::LabelHelp),
];

fn footer_hints(state: &AppState) -> &'static [(&'static str, Key)] {
    match state.view {
        View::Login => &[
            ("h/Esc", Key::Back),
            ("q", Key::Quit),
            ("?", Key::LabelHelp),
        ],
        View::Search if state.search.is_results() && state.search.input => &[
            ("Enter", Key::Search),
            ("Tab/⇧Tab", Key::SearchChannelSwitch),
            ("↓", Key::Select),
            ("Esc", Key::Back),
        ],
        View::Search if !state.search.is_results() => &[
            ("Enter", Key::Play),
            ("a", Key::AddToQueue),
            ("/", Key::Filter),
            ("h/Esc", Key::Back),
            ("q", Key::Quit),
            ("?", Key::LabelHelp),
        ],
        View::Search if state.search.channel == crate::api::SearchChannel::Songs => &[
            ("←/→", Key::SearchChannelSwitch),
            ("Enter", Key::Play),
            ("a", Key::AddToQueue),
            ("/", Key::Filter),
            ("q", Key::Quit),
            ("?", Key::LabelHelp),
        ],
        View::Search => &[
            ("←/→", Key::SearchChannelSwitch),
            ("Enter", Key::Open),
            ("h/Esc", Key::Back),
            ("q", Key::Quit),
            ("?", Key::LabelHelp),
        ],
        View::Settings => &[
            ("j/k", Key::Select),
            ("h/l", Key::SettingAdjust),
            ("v", Key::Spectrum),
            ("Enter", Key::Save),
            ("Esc/q", Key::Cancel),
        ],
        _ => PLAYBACK_HINTS,
    }
}

pub fn format_duration(duration: std::time::Duration) -> String {
    let total = duration.as_secs();
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// First visible index for a list viewport that keeps the selection
/// centered where possible.
pub fn scroll_offset(selected: usize, len: usize, visible: usize) -> usize {
    if visible == 0 || len <= visible {
        return 0;
    }
    selected.saturating_sub(visible / 2).min(len - visible)
}

pub fn format_ms(ms: i64) -> String {
    if ms <= 0 {
        return "--:--".into();
    }
    format_duration(std::time::Duration::from_millis(ms as u64))
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Alignment, Rect};
    use ratatui::Terminal;

    use super::{
        centered_content, draw, draw_help, draw_hints, draw_quit_confirm, filter_title,
        footer_hint_spans, footer_hints, panel_block, text, Hits, FOOTER_HEIGHT, HEADER_HEIGHT,
        HELP_MODAL_WIDTH, MAX_CONTENT_WIDTH, PANEL_GAP_Y, QUIT_MODAL_HEIGHT, QUIT_MODAL_WIDTH,
    };
    use crate::action::View;
    use crate::api::{ArtistHit, SearchChannel, SongRow};
    use crate::app::{AppState, NowPlaying};
    use crate::config::Config;
    use crate::i18n::{self, Key};

    fn rect_text(buffer: &Buffer, area: Rect) -> String {
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn matrix_state(view: View) -> AppState {
        let mut state = AppState::new(&Config::default());
        let row = SongRow {
            id: 1,
            title: "Matrix Track".into(),
            artist: "Matrix Artist".into(),
            album: "Matrix Album".into(),
            duration_ms: 180_000,
            pic_url: None,
        };
        state.view = view;
        state.session.nickname = Some("Matrix User".into());
        state.session.login_qr = Some("▀▀  ▀▀\n▄▄  ▄▄".into());
        state.session.login_message = Some(i18n::t(Key::ScanQr).into());
        state.library = vec![row.clone()];
        state.library_synced = true;
        state.search.input = false;
        state.search.query = "matrix".into();
        state.search.songs.items = vec![row.clone()];
        state.search.songs.total = 1;
        state.queue = vec![row];
        state.queue_pos = Some(0);
        state.current_track_id = Some(1);
        state.now = Some(NowPlaying {
            title: "Matrix Track".into(),
            artist: "Matrix Artist".into(),
            album: "Matrix Album".into(),
        });
        state.duration = Some(std::time::Duration::from_secs(180));
        state.position = std::time::Duration::from_secs(42);
        state
    }

    fn all_hit_rects(hits: &Hits) -> Vec<Rect> {
        let mut areas = Vec::new();
        areas.extend(hits.tabs.iter().map(|(area, _)| *area));
        areas.extend(hits.search_channels.iter().map(|(area, _)| *area));
        areas.extend(hits.rows.iter().map(|(area, _)| *area));
        areas.extend(hits.menu.iter().map(|(area, _)| *area));
        areas.extend(hits.sidebar.iter().map(|(area, _)| *area));
        areas.extend(hits.heart.iter().map(|(area, _)| *area));
        areas.extend(hits.play.iter().map(|(area, _)| *area));
        areas.extend(hits.playback_mode.iter().map(|(area, _)| *area));
        areas.extend(hits.volume.iter().map(|(area, _)| *area));
        areas.extend(hits.confirm.iter().map(|(area, _)| *area));
        areas.extend(hits.settings_rows.iter().map(|(area, _)| *area));
        areas.extend(hits.settings_adjust.iter().map(|(area, _)| *area));
        areas.extend(hits.settings_save.iter().copied());
        areas.extend(hits.settings_cancel.iter().copied());
        areas
    }

    fn rect_is_inside(inner: Rect, outer: Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.right() <= outer.right()
            && inner.bottom() <= outer.bottom()
    }

    fn shell_body(shell: Rect) -> Rect {
        let chrome = HEADER_HEIGHT
            .saturating_add(FOOTER_HEIGHT)
            .saturating_add(PANEL_GAP_Y.saturating_mul(2));
        Rect::new(
            shell.x,
            shell.y + HEADER_HEIGHT + PANEL_GAP_Y,
            shell.width,
            shell.height.saturating_sub(chrome),
        )
    }

    fn complete_rounded_frames(buffer: &Buffer, area: Rect) -> Vec<Rect> {
        let mut frames = Vec::new();
        for top in area.y..area.bottom() {
            for left in area.x..area.right() {
                if buffer[(left, top)].symbol() != "╭" {
                    continue;
                }
                let Some(right) =
                    (left + 1..area.right()).find(|x| buffer[(*x, top)].symbol() == "╮")
                else {
                    continue;
                };
                let Some(bottom) = (top + 1..area.bottom()).find(|y| {
                    buffer[(left, *y)].symbol() == "╰" && buffer[(right, *y)].symbol() == "╯"
                }) else {
                    continue;
                };
                if (top + 1..bottom).all(|y| {
                    buffer[(left, y)].symbol() == "│" && buffer[(right, y)].symbol() == "│"
                }) {
                    frames.push(Rect::new(left, top, right - left + 1, bottom - top + 1));
                }
            }
        }
        frames
    }

    #[test]
    fn panel_helper_owns_the_rounded_frame_padding_and_title_roles() {
        let state = AppState::new(&Config::default());
        let area = Rect::new(0, 0, 30, 5);
        assert_eq!(
            panel_block(&state.theme, "Tracks", Some("12 songs".into())).inner(area),
            Rect::new(2, 1, 26, 3)
        );

        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    panel_block(&state.theme, "Tracks", Some("12 songs".into())),
                    area,
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        assert_eq!(buffer[(0, 0)].symbol(), "╭");
        assert_eq!(buffer[(29, 0)].symbol(), "╮");
        assert_eq!(buffer[(0, 4)].symbol(), "╰");
        assert_eq!(buffer[(29, 4)].symbol(), "╯");
        assert_eq!(buffer[(0, 0)].fg, state.theme.faint);
        let title_x = (0..area.width)
            .find(|x| buffer[(*x, 0)].symbol() == "T")
            .unwrap();
        let detail_x = (0..area.width)
            .find(|x| buffer[(*x, 0)].symbol() == "·")
            .unwrap();
        assert_eq!(buffer[(title_x, 0)].fg, state.theme.accent);
        assert_eq!(buffer[(detail_x, 0)].fg, state.theme.dim);
    }

    #[test]
    fn filter_title_is_always_present_and_projects_the_active_input_state() {
        let mut state = AppState::new(&Config::default());
        let inactive = filter_title(&state);
        assert_eq!(inactive.alignment, Some(Alignment::Right));
        assert!(inactive.spans[0].content.contains(i18n::t(Key::Filter)));
        assert_eq!(inactive.spans[0].style.fg, Some(state.theme.dim));

        state.filter.query = "night".into();
        state.filter.input = true;
        let active = filter_title(&state);
        let rendered = active
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(active.alignment, Some(Alignment::Right));
        assert!(rendered.contains("night▎"));
        assert!(rendered.contains("Enter"));
        assert!(rendered.contains("Esc"));
        assert_eq!(active.spans[0].style.fg, Some(state.theme.accent));
        assert_eq!(active.spans[1].style.fg, Some(state.theme.fg));
        assert_eq!(active.spans[2].style.fg, Some(state.theme.dim));
    }

    #[test]
    fn footer_clips_before_a_hint_instead_of_drawing_a_partial_item() {
        let state = AppState::new(&Config::default());
        let hints = footer_hints(&state);
        let item_width = |(key, label): (&str, Key)| {
            text::display_width(&format!(" {key} "))
                + text::display_width(&format!(" {}", i18n::t(label)))
        };
        let first_width = item_width(hints[0]);
        let almost_two =
            first_width + usize::from(super::FOOTER_ITEM_GAP) + item_width(hints[1]) - 1;
        let spans = footer_hint_spans(&state, almost_two as u16);
        let rendered = spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains(hints[0].0));
        assert!(rendered.contains(i18n::t(hints[0].1)));
        assert!(!rendered.contains(hints[1].0));
        assert_eq!(text::display_width(&rendered), first_width);
    }

    #[test]
    fn centered_content_changes_only_above_the_wide_threshold() {
        assert_eq!(
            centered_content(Rect::new(7, 3, 120, 40)),
            Rect::new(7, 3, 120, 40)
        );
        assert_eq!(
            centered_content(Rect::new(7, 3, 121, 40)),
            Rect::new(12, 3, MAX_CONTENT_WIDTH, 40)
        );
    }

    #[test]
    fn header_right_edge_always_reports_the_account_state() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.view = View::Queue;
        state.session.nickname = None;
        let mut hits = Hits::default();
        terminal
            .draw(|frame| draw(frame, &mut state, &mut hits))
            .unwrap();

        let account = format!("♪ {}", i18n::t(Key::SignedOut));
        let account_width = text::display_width(&account) as u16;
        let rendered = rect_text(
            terminal.backend().buffer(),
            Rect::new(80 - account_width, 0, account_width, HEADER_HEIGHT),
        );
        let compact = |value: &str| {
            value
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect::<String>()
        };
        assert_eq!(compact(&rendered), compact(&account));
    }

    #[test]
    fn global_shell_snapshots_cover_compact_standard_and_wide_terminals() {
        for (width, height, expected) in [
            (80, 24, Rect::new(0, 0, 80, 24)),
            (120, 40, Rect::new(0, 0, 120, 40)),
            (200, 60, Rect::new(45, 0, MAX_CONTENT_WIDTH, 60)),
        ] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut state = AppState::new(&Config::default());
            state.view = View::Queue;
            state.session.nickname = Some("Jesse".into());
            let mut hits = Hits::default();
            terminal
                .draw(|frame| draw(frame, &mut state, &mut hits))
                .unwrap();
            let buffer = terminal.backend().buffer();

            assert_eq!(centered_content(Rect::new(0, 0, width, height)), expected);
            assert_eq!(hits.tabs.len(), 5, "{width}x{height}");
            for (tab, _) in &hits.tabs {
                assert!(tab.x >= expected.x, "{width}x{height}: {tab:?}");
                assert!(tab.right() <= expected.right(), "{width}x{height}: {tab:?}");
                assert_eq!(tab.y, expected.y);
                assert_eq!(tab.height, HEADER_HEIGHT);
            }
            let active = hits
                .tabs
                .iter()
                .find_map(|(area, view)| (*view == View::Queue).then_some(*area))
                .unwrap();
            assert_eq!(buffer[(active.x, active.y)].bg, state.theme.accent);
            assert_eq!(buffer[(active.x, active.y)].fg, state.theme.selection_fg());

            let header = Rect::new(expected.x, expected.y, expected.width, HEADER_HEIGHT);
            let footer = Rect::new(
                expected.x,
                expected.bottom() - FOOTER_HEIGHT,
                expected.width,
                FOOTER_HEIGHT,
            );
            let header_text = rect_text(buffer, header);
            let footer_text = rect_text(buffer, footer);
            assert!(
                header_text.ends_with("♪ Jesse"),
                "{width}x{height}: {header_text}"
            );
            assert!(
                footer_text.contains("320 kbps"),
                "{width}x{height}: {footer_text}"
            );
            assert!(!header_text.contains('…'));
            assert!(!footer_text.contains('…'));
            assert_eq!(buffer[(expected.x, footer.y)].bg, state.theme.faint);
            assert_eq!(
                buffer[(expected.right() - 1, footer.y)].bg,
                state.theme.faint
            );
            assert_eq!(
                buffer[(expected.right() - 2, footer.y)].symbol(),
                crate::icons::for_style(state.config.icons).sequential
            );

            let panel_top = expected.y + HEADER_HEIGHT + PANEL_GAP_Y;
            let panel_bottom = expected.bottom() - FOOTER_HEIGHT - PANEL_GAP_Y - 1;
            for (x, y, corner) in [
                (expected.x, panel_top, "╭"),
                (expected.right() - 1, panel_top, "╮"),
                (expected.x, panel_bottom, "╰"),
                (expected.right() - 1, panel_bottom, "╯"),
            ] {
                assert_eq!(buffer[(x, y)].symbol(), corner, "{width}x{height}");
                assert_eq!(buffer[(x, y)].fg, state.theme.faint, "{width}x{height}");
            }
            for y in panel_top + 1..panel_bottom {
                assert_eq!(buffer[(expected.x, y)].symbol(), "│", "{width}x{height}");
                assert_eq!(
                    buffer[(expected.right() - 1, y)].symbol(),
                    "│",
                    "{width}x{height}"
                );
            }

            for gap_y in [
                expected.y + HEADER_HEIGHT,
                expected.bottom() - FOOTER_HEIGHT - PANEL_GAP_Y,
            ] {
                assert!(
                    (expected.x..expected.right()).all(|x| buffer[(x, gap_y)].symbol() == " "),
                    "{width}x{height}: shell gap row {gap_y} is not empty"
                );
            }
            for y in 0..height {
                for x in 0..width {
                    if x < expected.x || x >= expected.right() {
                        assert_eq!(
                            buffer[(x, y)].symbol(),
                            " ",
                            "{width}x{height}: content escaped at ({x}, {y})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn three_size_skeleton_matrix_covers_every_product_view() {
        const SIZES: [(u16, u16, Rect); 3] = [
            (80, 24, Rect::new(0, 0, 80, 24)),
            (120, 40, Rect::new(0, 0, 120, 40)),
            (200, 60, Rect::new(45, 0, MAX_CONTENT_WIDTH, 60)),
        ];
        const VIEWS: [View; 6] = [
            View::Library,
            View::Search,
            View::Queue,
            View::Settings,
            View::Login,
            View::NowPlaying,
        ];

        for (width, height, expected_shell) in SIZES {
            for view in VIEWS {
                let backend = TestBackend::new(width, height);
                let mut terminal = Terminal::new(backend).unwrap();
                let mut state = matrix_state(view);
                let theme = state.theme;
                let mut hits = Hits::default();
                terminal
                    .draw(|frame| draw(frame, &mut state, &mut hits))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let frame_area = Rect::new(0, 0, width, height);
                let shell = centered_content(frame_area);
                let body = shell_body(shell);
                let context = format!("{width}x{height} {view:?}");

                assert_eq!(shell, expected_shell, "{context}");
                for area in all_hit_rects(&hits) {
                    assert!(area.width > 0 && area.height > 0, "{context}: {area:?}");
                    assert!(rect_is_inside(area, frame_area), "{context}: {area:?}");
                    assert!(rect_is_inside(area, shell), "{context}: {area:?}");
                }

                for y in 0..height {
                    for x in 0..width {
                        if x < shell.x || x >= shell.right() {
                            let cell = &buffer[(x, y)];
                            assert_eq!(cell.symbol(), " ", "{context}: escaped at ({x}, {y})");
                            assert_eq!(
                                cell.bg, theme.bg,
                                "{context}: background gap at ({x}, {y})"
                            );
                        }
                    }
                }

                let frames = complete_rounded_frames(buffer, body);
                let corner_count = |corner: &str| {
                    (body.y..body.bottom())
                        .flat_map(|y| (body.x..body.right()).map(move |x| (x, y)))
                        .filter(|(x, y)| buffer[(*x, *y)].symbol() == corner)
                        .count()
                };
                for frame in &frames {
                    assert!(rect_is_inside(*frame, body), "{context}: {frame:?}");
                    assert_eq!(buffer[(frame.x, frame.y)].fg, theme.faint, "{context}");
                }

                if view == View::NowPlaying {
                    assert!(
                        frames.is_empty(),
                        "{context}: unexpected list frame {frames:?}"
                    );
                    for corner in ["╭", "╮", "╰", "╯"] {
                        assert_eq!(corner_count(corner), 0, "{context}: {corner}");
                    }
                    assert!(
                        rect_text(buffer, body).contains("Matrix Track"),
                        "{context}"
                    );
                } else {
                    let expected_minimum = if view == View::Library { 2 } else { 1 };
                    assert!(
                        frames.len() >= expected_minimum,
                        "{context}: incomplete rounded frames {frames:?}"
                    );
                    for corner in ["╭", "╮", "╰", "╯"] {
                        assert_eq!(
                            corner_count(corner),
                            frames.len(),
                            "{context}: orphaned {corner}; frames={frames:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn zen_mode_hides_the_shell_bars_but_keeps_the_wide_canvas_centered() {
        let backend = TestBackend::new(200, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.zen = true;
        let mut hits = Hits::default();
        terminal
            .draw(|frame| draw(frame, &mut state, &mut hits))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let content = centered_content(Rect::new(0, 0, 200, 60));

        assert!(hits.tabs.is_empty());
        for y in 0..60 {
            for x in 0..200 {
                if x < content.x || x >= content.right() {
                    assert_eq!(buffer[(x, y)].symbol(), " ", "escaped at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn help_overlay_renders_the_refactored_key_map() {
        let state = AppState::new(&Config::default());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_help(frame, &state, frame.area()))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = (0..24)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        for keys in [
            "PgUp/PgDn",
            "Home/End · gg/G",
            "a",
            "Tab · ⇧Tab/←/→ (Search)",
            "←/→ 5s · Shift+←/→ 30s",
            "m",
            "s",
            "r",
            "v",
            "*",
            "/",
        ] {
            assert!(rendered.contains(keys), "{keys}");
        }
        let modal_x = (80 - HELP_MODAL_WIDTH) / 2;
        assert_eq!(buffer[(modal_x, 0)].symbol(), "╭");
        assert_eq!(buffer[(modal_x + HELP_MODAL_WIDTH - 1, 0)].symbol(), "╮");
        assert_eq!(buffer[(modal_x, 0)].fg, state.theme.faint);
        let key_y = (0..24)
            .find(|y| rect_text(buffer, Rect::new(0, *y, 80, 1)).contains("PgUp/PgDn"))
            .unwrap();
        let key_x = (0..80)
            .find(|x| buffer[(*x, key_y)].symbol() == "P")
            .unwrap();
        assert_eq!(buffer[(key_x, key_y)].fg, state.theme.fg);
        assert_eq!(buffer[(key_x, key_y)].bg, state.theme.faint);
    }

    #[test]
    fn quit_overlay_uses_a_faint_rounded_frame_and_keeps_the_confirm_chip() {
        let state = AppState::new(&Config::default());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();
        terminal
            .draw(|frame| draw_quit_confirm(frame, &state, frame.area(), &mut hits))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let x = (80 - QUIT_MODAL_WIDTH) / 2;
        let y = (24 - QUIT_MODAL_HEIGHT) / 2;

        assert_eq!(buffer[(x, y)].symbol(), "╭");
        assert_eq!(buffer[(x + QUIT_MODAL_WIDTH - 1, y)].symbol(), "╮");
        assert_eq!(buffer[(x, y + QUIT_MODAL_HEIGHT - 1)].symbol(), "╰");
        assert_eq!(buffer[(x, y)].fg, state.theme.faint);
        let confirm = hits
            .confirm
            .iter()
            .find(|(_, accepted)| *accepted)
            .unwrap()
            .0;
        assert_eq!(buffer[(confirm.x, confirm.y)].bg, state.theme.accent);
    }

    #[test]
    fn footer_keeps_only_six_high_frequency_playback_hints() {
        let mut state = AppState::new(&Config::default());
        for view in [View::NowPlaying, View::Library, View::Search, View::Queue] {
            state.view = view;
            state.search.input = false;
            assert_eq!(footer_hints(&state).len(), 6, "{view:?}");
        }

        let backend = TestBackend::new(160, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_hints(frame, &state, frame.area()))
            .unwrap();
        let rendered = (0..160)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect::<String>();

        for key in ["Space", "←/→", "*", "v", "q", "?"] {
            assert!(
                rendered.contains(key),
                "missing footer key {key}: {rendered}"
            );
        }
        for removed in [
            "-/+",
            "n/p",
            "PgUp/PgDn",
            " Mute ",
            " 静音 ",
            " ミュート ",
            " Shuffle ",
            " 随机开 / 关 ",
            " シャッフル ",
        ] {
            assert!(!rendered.contains(removed), "stale footer key {removed}");
        }
    }

    #[test]
    fn every_contextual_footer_stays_within_six_items() {
        let mut state = AppState::new(&Config::default());
        for view in [View::NowPlaying, View::Library, View::Queue, View::Login] {
            state.view = view;
            assert!(footer_hints(&state).len() <= 6, "{view:?}");
        }

        state.view = View::Search;
        state.search.input = true;
        assert!(footer_hints(&state).len() <= 6);
        state.view = View::Settings;
        assert!(footer_hints(&state).len() <= 6);
    }

    #[test]
    fn search_footer_distinguishes_input_results_and_detail_navigation() {
        let mut state = AppState::new(&Config::default());
        state.view = View::Search;

        let input = footer_hints(&state);
        assert_eq!(input[1], ("Tab/⇧Tab", Key::SearchChannelSwitch));
        assert_eq!(input[2], ("↓", Key::Select));
        assert!(!input.iter().any(|(key, _)| matches!(*key, "q" | "?")));

        state.search.input = false;
        state.search.channel = SearchChannel::Artists;
        let results = footer_hints(&state);
        assert_eq!(results[0], ("←/→", Key::SearchChannelSwitch));
        assert_eq!(results[1], ("Enter", Key::Open));
        assert!(!results.iter().any(|(key, _)| *key == "/"));

        state.search.artists.items.push(ArtistHit {
            id: 1,
            name: "Artist".into(),
            pic_url: None,
            album_count: 1,
            song_count: 1,
        });
        let _ = state.search.open_detail(0);
        let detail = footer_hints(&state);
        assert_eq!(detail[0], ("Enter", Key::Play));
        assert!(detail.iter().any(|(key, _)| *key == "/"));
        assert!(detail.iter().any(|(key, _)| *key == "h/Esc"));
    }

    #[test]
    fn a_six_line_terminal_uses_the_mini_player_instead_of_the_full_ui() {
        let mut state = AppState::new(&Config::default());
        state.now = Some(NowPlaying {
            title: "Mini title".into(),
            artist: "Mini artist".into(),
            album: String::new(),
        });
        let backend = TestBackend::new(80, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = super::Hits::default();

        terminal
            .draw(|frame| draw(frame, &mut state, &mut hits))
            .unwrap();

        let rendered = (0..80)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect::<String>();
        assert!(rendered.contains("Mini title — Mini artist"));
        assert_eq!(hits.play.len(), 1);
        assert_eq!(hits.heart.len(), 1);
    }
}
