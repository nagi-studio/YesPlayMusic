//! Television-style multi-channel search and nested track-detail pages.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::api::{AlbumHit, ArtistHit, PlaylistHit, SearchChannel, SearchChannelTabs, SongRow};
use crate::app::AppState;
use crate::i18n::{self, Key};
use crate::ui::text::{needs_marquee, pad_display, pad_display_right, pad_or_marquee};
use crate::ui::Hits;

use super::cover_preview;

/// Showing a cover keeps a comfortably scannable result panel.
const PREVIEW_MIN_LIST_PANEL_WIDTH: u16 = 62;
/// Full shell width required for the result panel, gap, and framed preview.
pub(crate) const PREVIEW_MIN_TERMINAL_WIDTH: u16 =
    PREVIEW_MIN_LIST_PANEL_WIDTH + super::PANEL_GAP_X + cover_preview::WIDTH;
/// Search input and channel switcher each occupy one terminal row.
const SEARCH_INPUT_HEIGHT: u16 = 1;
const CHANNEL_BAR_HEIGHT: u16 = 1;
/// One blank row separates search controls from their result list.
const SEARCH_CONTROLS_GAP: u16 = 1;
/// Inactive and active channel labels share the same horizontal rhythm.
const CHANNEL_GAP: u16 = 2;
/// The solid heart state occupies one terminal cell.
const HEART_WIDTH: usize = 1;
/// Keep the heart distinct from the primary title.
const HEART_TITLE_GAP: usize = 1;
/// Artist metadata keeps a stable scan width.
const ARTIST_WIDTH: usize = 14;
/// Terminal playback durations always use `mm:ss`.
const DURATION_WIDTH: usize = 5;
/// Metadata columns are separated by one blank terminal cell.
const COLUMN_GAP: usize = 1;
/// Entity rows keep a compact ordinal before their primary label.
const ENTITY_INDEX_WIDTH: usize = 3;
const ENTITY_INDEX_GAP: usize = 2;
const ENTITY_SECONDARY_MAX_WIDTH: usize = 18;
const ENTITY_COUNT_MAX_WIDTH: usize = 20;

/// Visible result rows after the global shell, panel, controls, and table header.
pub(crate) fn page_size(state: &AppState, terminal_rows: u16) -> usize {
    let body = terminal_rows.saturating_sub(
        super::HEADER_HEIGHT + super::FOOTER_HEIGHT + super::PANEL_GAP_Y.saturating_mul(2),
    );
    let panel_inner = body.saturating_sub(2);
    if state.search.is_results() {
        let list = panel_inner
            .saturating_sub(SEARCH_INPUT_HEIGHT + CHANNEL_BAR_HEIGHT + SEARCH_CONTROLS_GAP);
        return usize::from(
            list.saturating_sub(u16::from(state.search.channel == SearchChannel::Songs)),
        );
    }
    usize::from(panel_inner.saturating_sub(1))
}

pub fn draw(frame: &mut Frame, state: &mut AppState, area: Rect, hits: &mut Hits) {
    let has_selected_song = !state.filter.input
        && (!state.search.is_results() || !state.search.input)
        && state
            .search
            .song_rows()
            .is_some_and(|rows| !state.visible_rows(rows).is_empty());
    let (panel_area, preview_area) = if has_selected_song {
        cover_preview::split_preview(area, PREVIEW_MIN_LIST_PANEL_WIDTH)
    } else {
        (area, None)
    };
    draw_panel(frame, state, panel_area, hits);
    if let Some(preview_area) = preview_area {
        cover_preview::draw(frame, state, preview_area);
    }
}

pub(crate) fn marquee_needed(row: &SongRow, area_width: u16, preview_visible: bool) -> bool {
    let area = Rect::new(0, 0, area_width, cover_preview::HEIGHT);
    let (panel, preview) = if preview_visible {
        cover_preview::split_preview(area, PREVIEW_MIN_LIST_PANEL_WIDTH)
    } else {
        (area, None)
    };
    let columns = SearchColumns::for_width(super::panel_inner_width(panel.width));
    // Matches the library list: the title scrolls, the artist truncates.
    needs_marquee(&row.title, columns.title)
        || preview.is_some() && cover_preview::metadata_needs_marquee(row)
}

fn draw_panel(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    let theme = &state.theme;
    let is_results = state.search.is_results();
    let title = state
        .search
        .detail_title()
        .unwrap_or_else(|| i18n::t(Key::Search));
    let count = if !is_results {
        i18n::t_track_count(
            state
                .search
                .song_rows()
                .map_or(0, |rows| state.visible_rows(rows).len()),
        )
    } else if state.search.channel == SearchChannel::Songs {
        i18n::t_track_count(state.search.current_total())
    } else {
        i18n::t_result_count(state.search.current_total())
    };
    let mut block = super::panel_block(theme, title, Some(count));
    if !is_results || state.search.channel == SearchChannel::Songs {
        block = block.title_bottom(super::filter_title(state));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }

    if is_results {
        let [input_area, channels_area, _, list_area] = Layout::vertical([
            Constraint::Length(SEARCH_INPUT_HEIGHT),
            Constraint::Length(CHANNEL_BAR_HEIGHT),
            Constraint::Length(SEARCH_CONTROLS_GAP),
            Constraint::Min(0),
        ])
        .areas(inner);
        draw_input(frame, state, input_area);
        draw_channel_bar(frame, state, channels_area, hits);
        draw_results(frame, state, list_area, hits);
    } else {
        draw_detail(frame, state, inner, hits);
    }
}

fn draw_input(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = &state.theme;
    let icons = crate::icons::for_style(state.config.icons);
    if state.search.input {
        // The terminal's own cursor is the caret: drawing one here too
        // painted two colours into the same cell. Parking the real cursor
        // also keeps IME candidate windows anchored to the input point.
        let prefix = super::text::display_width(icons.search)
            + 1
            + super::text::display_width(&state.search.query);
        let x = area.x + (prefix as u16).min(area.width.saturating_sub(1));
        frame.set_cursor_position((x, area.y));
    }
    let query_style = if state.search.input {
        Style::new().fg(theme.fg)
    } else {
        Style::new().fg(theme.dim)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{} ", icons.search), Style::new().fg(theme.faint)),
            Span::styled(state.search.query.clone(), query_style),
            Span::styled(
                if state.search.query.is_empty() && state.search.input {
                    format!("  {}", i18n::t(Key::TypeToSearch))
                } else {
                    String::new()
                },
                Style::new().fg(theme.faint),
            ),
        ])),
        area,
    );
}

fn draw_channel_bar(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    let widths = SearchChannel::TABS.map(|channel| {
        super::text::display_width(channel_label(channel))
            + usize::from(channel == state.search.channel) * 2
    });
    let labels_width: usize = widths.iter().sum();
    let gap = if labels_width + CHANNEL_GAP as usize * (SearchChannel::TABS.len() - 1)
        <= area.width as usize
    {
        CHANNEL_GAP
    } else if labels_width + SearchChannel::TABS.len() - 1 <= area.width as usize {
        1
    } else {
        0
    };
    if labels_width > area.width as usize {
        draw_channel_chip(frame, state, area, state.search.channel, true, hits);
        return;
    }

    let mut x = area.x;
    for (channel, width) in SearchChannel::TABS.into_iter().zip(widths) {
        let width = width as u16;
        if x.saturating_add(width) > area.right() {
            break;
        }
        let active = channel == state.search.channel;
        let channel_area = Rect::new(x, area.y, width, CHANNEL_BAR_HEIGHT);
        draw_channel_chip(frame, state, channel_area, channel, active, hits);
        x = x.saturating_add(width).saturating_add(gap);
    }
}

fn draw_channel_chip(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    channel: SearchChannel,
    active: bool,
    hits: &mut Hits,
) {
    let label = if active {
        format!(" {} ", channel_label(channel))
    } else {
        channel_label(channel).to_owned()
    };
    let style = if active {
        Style::new()
            .fg(state.theme.selection_fg())
            .bg(state.theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(state.theme.dim)
    };
    let width = area.width.min(super::text::display_width(&label) as u16);
    if width == 0 {
        return;
    }
    let target = Rect::new(area.x, area.y, width, CHANNEL_BAR_HEIGHT);
    frame.render_widget(Paragraph::new(label).style(style), target);
    hits.search_channels.push((target, channel));
}

fn channel_label(channel: SearchChannel) -> &'static str {
    i18n::t(match channel {
        SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
            Key::SearchSongs
        }
        SearchChannel::Artists => Key::SearchArtists,
        SearchChannel::Albums => Key::SearchAlbums,
        SearchChannel::Playlists => Key::SearchPlaylists,
    })
}

fn draw_results(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    if draw_pending_or_empty(frame, state, area) {
        return;
    }

    match state.search.channel {
        SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
            if let Some(rows) = state.search.song_rows() {
                draw_song_rows(frame, state, area, rows, !state.search.input, hits);
            }
        }
        SearchChannel::Artists => draw_artist_rows(frame, state, area, hits),
        SearchChannel::Albums => draw_album_rows(frame, state, area, hits),
        SearchChannel::Playlists => draw_playlist_rows(frame, state, area, hits),
    }
}

fn draw_detail(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    if draw_pending_or_empty(frame, state, area) {
        return;
    }
    if let Some(rows) = state.search.song_rows() {
        draw_song_rows(frame, state, area, rows, true, hits);
    }
}

/// Returns true when the list area has been consumed by a state message.
fn draw_pending_or_empty(frame: &mut Frame, state: &AppState, area: Rect) -> bool {
    let theme = &state.theme;
    let message = if state.search.current_searching() {
        Some((i18n::t(Key::Searching), theme.dim))
    } else if let Some(error) = state.search.current_error() {
        Some((error, theme.accent2))
    } else if state.search.current_len() == 0 {
        Some((
            if state.search.query.is_empty() && state.search.is_results() {
                i18n::t(Key::SearchPrompt)
            } else {
                i18n::t(Key::NoResults)
            },
            theme.faint,
        ))
    } else {
        None
    };
    let Some((message, color)) = message else {
        return false;
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(message, Style::new().fg(color)))).centered(),
        area,
    );
    true
}

fn draw_song_rows(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    rows: &[SongRow],
    selection_enabled: bool,
    hits: &mut Hits,
) {
    let rows = state.visible_rows(rows);
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                i18n::t(Key::NoResults),
                Style::new().fg(state.theme.faint),
            )))
            .centered(),
            area,
        );
        return;
    }
    let visible = area.height.saturating_sub(1) as usize;
    let offset = super::scroll_offset(state.selected, rows.len(), visible);
    let marquee_frame = state.marquee_frame();
    let columns = SearchColumns::for_width(area.width as usize);
    let icons = crate::icons::for_style(state.config.icons);
    let mut lines = Vec::with_capacity(visible + 1);
    lines.push(columns.header(&state.theme));
    for (visible_index, (_, row)) in rows.iter().enumerate().skip(offset).take(visible) {
        hits.rows.push((
            Rect {
                x: area.x,
                y: area.y + 1 + (visible_index - offset) as u16,
                width: area.width,
                height: 1,
            },
            visible_index,
        ));
        let selected = visible_index == state.selected && selection_enabled && !state.filter.input;
        lines.push(columns.row(
            state,
            icons.heart,
            row,
            state.liked.contains(&row.id),
            selected,
            marquee_frame,
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_artist_rows(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    draw_entities(
        frame,
        state,
        area,
        state.search.artists().iter().map(EntityRow::from_artist),
        hits,
    );
}

fn draw_album_rows(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    draw_entities(
        frame,
        state,
        area,
        state.search.albums().iter().map(EntityRow::from_album),
        hits,
    );
}

fn draw_playlist_rows(frame: &mut Frame, state: &AppState, area: Rect, hits: &mut Hits) {
    draw_entities(
        frame,
        state,
        area,
        state
            .search
            .playlists()
            .iter()
            .map(EntityRow::from_playlist),
        hits,
    );
}

fn draw_entities<'a>(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    rows: impl Iterator<Item = EntityRow<'a>>,
    hits: &mut Hits,
) {
    let rows: Vec<_> = rows.collect();
    let visible = area.height as usize;
    let offset = super::scroll_offset(state.selected, rows.len(), visible);
    let columns = EntityColumns::for_width(area.width as usize);
    let mut lines = Vec::with_capacity(visible);
    for (visible_index, row) in rows.iter().enumerate().skip(offset).take(visible) {
        hits.rows.push((
            Rect {
                x: area.x,
                y: area.y + (visible_index - offset) as u16,
                width: area.width,
                height: 1,
            },
            visible_index,
        ));
        lines.push(columns.row(
            &state.theme,
            state.selection_style(),
            visible_index + 1,
            row,
            visible_index == state.selected && !state.search.input,
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

struct EntityRow<'a> {
    name: &'a str,
    secondary: &'a str,
    count: String,
}

impl<'a> EntityRow<'a> {
    fn from_artist(artist: &'a ArtistHit) -> Self {
        Self {
            name: &artist.name,
            secondary: "",
            count: format!(
                "{} {} · {} {}",
                artist.song_count,
                channel_label(SearchChannel::Songs),
                artist.album_count,
                channel_label(SearchChannel::Albums)
            ),
        }
    }

    fn from_album(album: &'a AlbumHit) -> Self {
        Self {
            name: &album.name,
            secondary: &album.artist.name,
            count: format!(
                "{} {}",
                album.song_count,
                channel_label(SearchChannel::Songs)
            ),
        }
    }

    fn from_playlist(playlist: &'a PlaylistHit) -> Self {
        Self {
            name: &playlist.name,
            secondary: &playlist.creator,
            count: format!(
                "{} {}",
                playlist.track_count,
                channel_label(SearchChannel::Songs)
            ),
        }
    }
}

#[derive(Clone, Copy)]
struct EntityColumns {
    name: usize,
    secondary: usize,
    count: usize,
}

impl EntityColumns {
    fn for_width(width: usize) -> Self {
        let content = width.saturating_sub(ENTITY_INDEX_WIDTH + ENTITY_INDEX_GAP);
        let count = ENTITY_COUNT_MAX_WIDTH.min(content / 3);
        let remainder = content.saturating_sub(count);
        let secondary = ENTITY_SECONDARY_MAX_WIDTH.min(remainder / 3);
        let gaps = usize::from(count > 0) + usize::from(secondary > 0);
        Self {
            name: remainder.saturating_sub(secondary).saturating_sub(gaps),
            secondary,
            count,
        }
    }

    fn row(
        self,
        theme: &crate::theme::Theme,
        selection_style: Style,
        index: usize,
        row: &EntityRow<'_>,
        selected: bool,
    ) -> Line<'static> {
        let base = if selected {
            selection_style
        } else {
            Style::new()
        };
        let name_style = if selected {
            base.fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            base.fg(theme.fg)
        };
        let mut spans = vec![
            Span::styled(
                format!("{index:>ENTITY_INDEX_WIDTH$}"),
                base.fg(theme.faint),
            ),
            Span::styled(" ".repeat(ENTITY_INDEX_GAP), base),
            Span::styled(pad_display(row.name, self.name), name_style),
        ];
        if self.secondary > 0 {
            spans.push(Span::styled(" ", base));
            spans.push(Span::styled(
                pad_display(row.secondary, self.secondary),
                base.fg(theme.dim),
            ));
        }
        if self.count > 0 {
            spans.push(Span::styled(" ", base));
            spans.push(Span::styled(
                pad_display_right(&row.count, self.count),
                base.fg(theme.faint),
            ));
        }
        Line::from(spans)
    }
}

#[derive(Clone, Copy)]
struct SearchColumns {
    title: usize,
}

impl SearchColumns {
    fn for_width(width: usize) -> Self {
        let fixed = HEART_WIDTH + HEART_TITLE_GAP + ARTIST_WIDTH + DURATION_WIDTH + COLUMN_GAP * 2;
        Self {
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
            Span::styled(" ".repeat(HEART_WIDTH + HEART_TITLE_GAP), style),
            Span::styled(pad_display(i18n::t(Key::ColumnTitle), self.title), style),
            Span::styled(" ", style),
            Span::styled(pad_display(i18n::t(Key::ColumnArtist), ARTIST_WIDTH), style),
            Span::styled(" ", style),
            Span::styled(pad_display_right(duration_label, DURATION_WIDTH), style),
        ])
    }

    fn row(
        self,
        state: &AppState,
        heart: &'static str,
        row: &SongRow,
        liked: bool,
        selected: bool,
        marquee_frame: u64,
    ) -> Line<'static> {
        let theme = &state.theme;
        let base = if selected {
            state.selection_style()
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
                heart,
                base.fg(if liked { theme.accent2 } else { theme.faint }),
            ),
            Span::styled(" ", base),
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
    use crate::action::View;
    use crate::config::Config;

    fn song() -> SongRow {
        SongRow {
            id: 7,
            title: "Matrix Track".into(),
            artist: "Matrix Artist".into(),
            album: "Matrix Album".into(),
            duration_ms: 180_000,
            pic_url: None,
            artist_id: None,
            album_id: None,
        }
    }

    fn rendered_search_shell(width: u16, height: u16) -> (ratatui::buffer::Buffer, Hits, AppState) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.view = View::Search;
        state.search.input = false;
        state.search.query = "matrix".into();
        state.search.songs.items = vec![song()];
        state.search.songs.total = 1;
        let mut hits = Hits::default();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut state, &mut hits))
            .unwrap();
        (terminal.backend().buffer().clone(), hits, state)
    }

    fn rendered_detail_shell(width: u16, height: u16) -> (ratatui::buffer::Buffer, Hits, AppState) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.view = View::Search;
        state.search.input = false;
        state.search.query = "matrix".into();
        state.search.channel = SearchChannel::Albums;
        state.search.albums.items = vec![AlbumHit {
            mark: 0,
            id: 42,
            name: "Matrix Album".into(),
            artist: yesplaymusic_core::ncm::ArtistRef {
                id: 0,
                name: "Matrix Artist".into(),
            },
            pic_url: None,
            song_count: 1,
        }];
        let _ = state.search.open_detail(0);
        let detail = state.search.detail.as_mut().unwrap();
        detail.rows = vec![song()];
        detail.searching = false;
        let mut hits = Hits::default();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut state, &mut hits))
            .unwrap();
        (terminal.backend().buffer().clone(), hits, state)
    }

    #[test]
    fn cjk_duration_header_fits_every_supported_search_width() {
        let state = AppState::new(&Config::default());
        let minimum =
            HEART_WIDTH + HEART_TITLE_GAP + ARTIST_WIDTH + DURATION_WIDTH + COLUMN_GAP * 2;

        for width in minimum..=200 {
            let header = SearchColumns::for_width(width).header_with_duration(&state.theme, "时长");
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
    fn page_size_accounts_for_channel_controls_and_song_header() {
        let mut state = AppState::new(&Config::default());
        state.search.channel = SearchChannel::Songs;
        assert_eq!(page_size(&state, 24), 14);

        state.search.channel = SearchChannel::Artists;
        assert_eq!(page_size(&state, 24), 15);
        assert_eq!(page_size(&state, 6), 0);

        state.search.artists.items.push(ArtistHit {
            img1v1_url: None,
            id: 1,
            name: "Artist".into(),
            pic_url: None,
            album_count: 1,
            song_count: 1,
        });
        let _ = state.search.open_detail(0);
        assert_eq!(page_size(&state, 24), 17);
    }

    #[test]
    fn active_search_channel_is_an_accent_chip() {
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.search.channel = SearchChannel::Albums;
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw_channel_bar(frame, &state, frame.area(), &mut hits))
            .unwrap();

        assert_eq!(hits.search_channels.len(), SearchChannel::TABS.len());
        let album = hits
            .search_channels
            .iter()
            .find(|(_, channel)| *channel == SearchChannel::Albums)
            .unwrap()
            .0;
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(album.x, album.y)].bg, state.theme.accent);
        assert_eq!(buffer[(album.x, album.y)].fg, state.theme.selection_fg());
        assert!(buffer[(album.x, album.y)].modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(hits.search_channels[0].0.x, 0)].fg, state.theme.dim);
    }

    #[test]
    fn narrow_channel_bar_keeps_the_active_channel_visible() {
        let mut state = AppState::new(&Config::default());
        state.search.channel = SearchChannel::Playlists;
        let active_width =
            (super::super::text::display_width(channel_label(SearchChannel::Playlists)) + 2) as u16;
        let backend = TestBackend::new(active_width, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw_channel_bar(frame, &state, frame.area(), &mut hits))
            .unwrap();

        assert_eq!(hits.search_channels.len(), 1);
        assert_eq!(hits.search_channels[0].1, SearchChannel::Playlists);
        assert_eq!(terminal.backend().buffer()[(0, 0)].bg, state.theme.accent);
    }

    #[test]
    fn selected_entity_row_obeys_the_three_text_tiers() {
        let state = AppState::new(&Config::default());
        let columns = EntityColumns::for_width(76);
        let row = EntityRow {
            name: "Album",
            secondary: "Artist",
            count: "12 Songs".into(),
        };
        let line = columns.row(&state.theme, state.selection_style(), 1, &row, true);
        assert_eq!(line.width(), 76);
        assert_eq!(line.spans[0].style.fg, Some(state.theme.faint));
        assert_eq!(line.spans[2].style.fg, Some(state.theme.fg));
        assert!(line.spans[2].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[4].style.fg, Some(state.theme.dim));
        assert_eq!(line.spans[6].style.fg, Some(state.theme.faint));
        assert!(line
            .spans
            .iter()
            .all(|span| span.style.bg == state.selection_style().bg));
    }

    #[test]
    fn search_shell_obeys_the_three_size_layout_matrix() {
        for (width, height, shell_x, panel_right, row) in [
            (80, 24, 0, 79, Rect::new(2, 7, 76, 1)),
            (120, 40, 0, 91, Rect::new(2, 7, 88, 1)),
            (200, 60, 30, 141, Rect::new(32, 7, 108, 1)),
        ] {
            let (buffer, hits, state) = rendered_search_shell(width, height);
            let context = format!("{width}x{height}");

            assert_eq!(buffer[(shell_x, 2)].symbol(), "╭", "{context}");
            assert_eq!(buffer[(panel_right, 2)].symbol(), "╮", "{context}");
            assert_eq!(hits.rows, vec![(row, 0)], "{context}");
            assert_eq!(hits.search_channels.len(), 4, "{context}");
            assert!(hits
                .search_channels
                .iter()
                .all(|(area, _)| area.y == 4 && area.right() <= panel_right));
            let songs = hits.search_channels[0].0;
            assert_eq!(buffer[(songs.x, songs.y)].bg, state.theme.accent);
            assert!((shell_x..=panel_right).any(|x| buffer[(x, height - 3)].symbol() == "/"));

            if width >= 120 {
                let preview_x = panel_right + super::super::PANEL_GAP_X + 1;
                assert_eq!(buffer[(preview_x, 2)].symbol(), "╭", "{context}");
            }
        }
    }

    #[test]
    fn detail_shell_obeys_the_three_size_layout_matrix() {
        for (width, height, shell_x, panel_right, row) in [
            (80, 24, 0, 79, Rect::new(2, 4, 76, 1)),
            (120, 40, 0, 91, Rect::new(2, 4, 88, 1)),
            (200, 60, 30, 141, Rect::new(32, 4, 108, 1)),
        ] {
            let (buffer, hits, state) = rendered_detail_shell(width, height);
            let context = format!("{width}x{height}");

            assert_eq!(buffer[(shell_x, 2)].symbol(), "╭", "{context}");
            assert_eq!(buffer[(panel_right, 2)].symbol(), "╮", "{context}");
            assert_eq!(hits.rows, vec![(row, 0)], "{context}");
            assert!(hits.search_channels.is_empty(), "{context}");
            let title = (shell_x..=panel_right).find(|x| {
                buffer[(*x, 2)].symbol() == "M" && buffer[(*x, 2)].fg == state.theme.accent
            });
            assert!(title.is_some(), "{context}");
            assert!((shell_x..=panel_right).any(|x| buffer[(x, height - 3)].symbol() == "/"));

            if width >= 120 {
                let preview_x = panel_right + super::super::PANEL_GAP_X + 1;
                assert_eq!(buffer[(preview_x, 2)].symbol(), "╭", "{context}");
            }
        }
    }

    #[test]
    fn entity_channel_uses_the_full_panel_and_records_rows() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.search.channel = SearchChannel::Artists;
        state.search.input = false;
        state.search.query = "jay".into();
        state.search.artists.items = vec![ArtistHit {
            img1v1_url: None,
            id: 1,
            name: "Jay Chou".into(),
            pic_url: None,
            album_count: 15,
            song_count: 220,
        }];
        state.search.artists.total = 1;
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        assert_eq!(hits.rows, vec![(Rect::new(2, 4, 76, 1), 0)]);
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "╭");
        assert_eq!(terminal.backend().buffer()[(79, 0)].symbol(), "╮");
        assert!(!(0..80).any(|x| terminal.backend().buffer()[(x, 23)].symbol() == "/"));
        let selected = &terminal.backend().buffer()[(7, 4)];
        assert_eq!(selected.bg, state.selection_style().bg.unwrap());
        assert_eq!(selected.fg, state.theme.fg);
    }

    #[test]
    fn selected_song_result_uses_the_three_text_tiers() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(&Config::default());
        state.search.input = false;
        state.search.songs.items = vec![song()];
        state.search.songs.total = 1;
        let mut hits = Hits::default();

        terminal
            .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let title = &buffer[(4, 5)];
        assert_eq!(title.fg, state.theme.fg);
        assert_eq!(title.bg, state.selection_style().bg.unwrap());
        assert!(title.modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(58, 5)].fg, state.theme.dim);
        assert_eq!(buffer[(73, 5)].fg, state.theme.faint);
    }

    #[test]
    fn song_results_render_solid_hearts_in_state_colors() {
        for liked in [false, true] {
            let backend = TestBackend::new(80, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut state = AppState::new(&Config::default());
            state.search.input = false;
            state.search.songs.items = vec![song()];
            state.search.songs.total = 1;
            if liked {
                state.liked.insert(7);
            }
            let mut hits = Hits::default();

            terminal
                .draw(|frame| draw(frame, &mut state, frame.area(), &mut hits))
                .unwrap();

            let cell = &terminal.backend().buffer()[(2, 5)];
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
}
