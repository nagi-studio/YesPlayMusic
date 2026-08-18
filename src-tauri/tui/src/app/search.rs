use std::{future::Future, time::Duration};

use tokio::task::AbortHandle;

use crate::action::Action;
use crate::api::{
    AlbumHit, ArtistHit, PlaylistHit, SearchChannel, SearchChannelTabs, SearchPage, SearchPayload,
    SongRow,
};
use crate::i18n::{self, Key};

use super::{AppState, Effects};

const SEARCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

impl AppState {
    pub(super) fn handle_search_key(&mut self, fx: &Effects, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('c') {
                self.confirm_quit = true;
            }
            return;
        }
        match key.code {
            KeyCode::Tab => self.cycle_search_channel(
                fx,
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    -1
                } else {
                    1
                },
            ),
            KeyCode::BackTab => self.cycle_search_channel(fx, -1),
            KeyCode::Char(c) => {
                self.search.push(c);
                self.selected = 0;
            }
            KeyCode::Backspace => {
                self.search.pop();
                self.selected = 0;
            }
            KeyCode::Enter => {
                if let Some(request) = self.search.submit() {
                    let (seq, channel) = (request.seq, request.channel);
                    let task = spawn_search(fx, request);
                    self.search.attach_search_task(seq, channel, task);
                }
            }
            KeyCode::Esc => {
                // Esc walks outward one layer at a time: focus the result
                // list first (number keys switch views there), then clear
                // the draft query, then leave the view.
                if self.search.current_len() > 0 {
                    self.search.input = false;
                    self.selected = self.search.saved_selection();
                } else if !self.search.query.is_empty() {
                    self.search.clear();
                    self.selected = 0;
                } else {
                    self.navigate_back(fx);
                }
            }
            KeyCode::Down if self.search.current_len() > 0 => {
                self.search.input = false;
                self.selected = self.search.saved_selection();
            }
            _ => {}
        }
    }

    pub(super) fn handle_search_channel_key(
        &mut self,
        fx: &Effects,
        key: crossterm::event::KeyEvent,
    ) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        if !self.search.is_results() || self.filter.input {
            return false;
        }
        let delta = match key.code {
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    -1
                } else {
                    1
                }
            }
            KeyCode::BackTab => -1,
            KeyCode::Right if !key.modifiers.contains(KeyModifiers::SHIFT) => 1,
            KeyCode::Left if !key.modifiers.contains(KeyModifiers::SHIFT) => -1,
            _ => return false,
        };
        self.cycle_search_channel(fx, delta);
        true
    }

    fn cycle_search_channel(&mut self, fx: &Effects, delta: i32) {
        let channel = self.search.channel.cycle(delta);
        self.select_search_channel(fx, channel);
    }

    pub(super) fn select_search_channel(&mut self, fx: &Effects, channel: SearchChannel) {
        let selected = self
            .visible_row(self.selected)
            .map_or(self.selected, |(underlying, _)| underlying);
        self.filter.clear();
        let (selected, request) = self.search.select_channel(channel, selected);
        self.selected = selected;
        if let Some(request) = request {
            let (seq, channel) = (request.seq, request.channel);
            let task = spawn_search(fx, request);
            self.search.attach_search_task(seq, channel, task);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SearchRequest {
    pub seq: u64,
    pub query: String,
    pub channel: SearchChannel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DetailRequest {
    pub seq: u64,
    pub channel: SearchChannel,
    pub id: i64,
}

pub struct SearchBucket<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub error: Option<String>,
    pub searching: bool,
    loaded_query: Option<String>,
    active: Option<SearchRequest>,
}

impl<T> Default for SearchBucket<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            total: 0,
            error: None,
            searching: false,
            loaded_query: None,
            active: None,
        }
    }
}

impl<T> SearchBucket<T> {
    fn invalidate(&mut self) {
        self.items.clear();
        self.total = 0;
        self.error = None;
        self.searching = false;
        self.loaded_query = None;
        self.active = None;
    }

    fn is_loaded_for(&self, query: &str) -> bool {
        self.loaded_query.as_deref() == Some(query) && !self.searching
    }

    fn is_loading_for(&self, query: &str) -> bool {
        self.searching
            && self
                .active
                .as_ref()
                .is_some_and(|request| request.query == query)
    }

    fn begin(&mut self, request: SearchRequest) {
        self.items.clear();
        self.total = 0;
        self.searching = true;
        self.error = None;
        self.active = Some(request);
    }

    fn matches(&self, seq: u64, query: &str, channel: SearchChannel) -> bool {
        self.active.as_ref().is_some_and(|request| {
            request.seq == seq && request.query == query && request.channel == channel
        })
    }

    fn accept(&mut self, request: &SearchRequest, page: SearchPage<T>) {
        self.active = None;
        self.searching = false;
        self.loaded_query = Some(request.query.clone());
        self.items = page.items;
        self.total = page.total;
        self.error = None;
    }

    fn fail(&mut self, request: &SearchRequest, message: String) {
        self.active = None;
        self.searching = false;
        self.loaded_query = Some(request.query.clone());
        self.items.clear();
        self.total = 0;
        self.error = Some(message);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchDetail {
    pub channel: SearchChannel,
    pub id: i64,
    pub title: String,
    pub rows: Vec<SongRow>,
    pub error: Option<String>,
    pub searching: bool,
    cover_url: Option<String>,
    seq: u64,
    parent_selected: usize,
    selected: usize,
}

pub struct SearchState {
    pub query: String,
    pub channel: SearchChannel,
    pub songs: SearchBucket<SongRow>,
    pub artists: SearchBucket<ArtistHit>,
    pub albums: SearchBucket<AlbumHit>,
    pub playlists: SearchBucket<PlaylistHit>,
    pub detail: Option<SearchDetail>,
    pub input: bool,
    seq: u64,
    committed_query: Option<String>,
    selections: [usize; 4],
    search_tasks: [Option<AbortHandle>; 4],
    detail_task: Option<AbortHandle>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            channel: SearchChannel::Songs,
            songs: SearchBucket::default(),
            artists: SearchBucket::default(),
            albums: SearchBucket::default(),
            playlists: SearchBucket::default(),
            detail: None,
            input: false,
            seq: 0,
            committed_query: None,
            selections: [0; 4],
            search_tasks: [None, None, None, None],
            detail_task: None,
        }
    }
}

impl SearchState {
    pub(super) fn new() -> Self {
        Self {
            input: true,
            ..Self::default()
        }
    }

    pub fn is_results(&self) -> bool {
        self.detail.is_none()
    }

    pub fn detail_title(&self) -> Option<&str> {
        self.detail.as_ref().map(|detail| detail.title.as_str())
    }

    pub fn song_rows(&self) -> Option<&[SongRow]> {
        if let Some(detail) = &self.detail {
            return Some(&detail.rows);
        }
        (self.channel == SearchChannel::Songs).then_some(self.songs.items.as_slice())
    }

    pub fn artists(&self) -> &[ArtistHit] {
        &self.artists.items
    }

    pub fn albums(&self) -> &[AlbumHit] {
        &self.albums.items
    }

    pub fn playlists(&self) -> &[PlaylistHit] {
        &self.playlists.items
    }

    pub fn current_len(&self) -> usize {
        if let Some(detail) = &self.detail {
            return detail.rows.len();
        }
        match self.channel {
            SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
                self.songs.items.len()
            }
            SearchChannel::Artists => self.artists.items.len(),
            SearchChannel::Albums => self.albums.items.len(),
            SearchChannel::Playlists => self.playlists.items.len(),
        }
    }

    pub fn current_total(&self) -> usize {
        if let Some(detail) = &self.detail {
            return detail.rows.len();
        }
        match self.channel {
            SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
                self.songs.total
            }
            SearchChannel::Artists => self.artists.total,
            SearchChannel::Albums => self.albums.total,
            SearchChannel::Playlists => self.playlists.total,
        }
    }

    pub fn current_searching(&self) -> bool {
        if let Some(detail) = &self.detail {
            return detail.searching;
        }
        self.bucket_searching(self.channel)
    }

    pub fn current_error(&self) -> Option<&str> {
        if let Some(detail) = &self.detail {
            return detail.error.as_deref();
        }
        match self.channel {
            SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
                self.songs.error.as_deref()
            }
            SearchChannel::Artists => self.artists.error.as_deref(),
            SearchChannel::Albums => self.albums.error.as_deref(),
            SearchChannel::Playlists => self.playlists.error.as_deref(),
        }
    }

    pub fn saved_selection(&self) -> usize {
        self.selections[self.channel.index()].min(self.current_len().saturating_sub(1))
    }

    pub(super) fn remember_selection(&mut self, selected: usize) {
        if let Some(detail) = &mut self.detail {
            detail.selected = selected.min(detail.rows.len().saturating_sub(1));
        } else {
            self.selections[self.channel.index()] = selected;
        }
    }

    pub(super) fn page_selection(&self) -> usize {
        self.detail.as_ref().map_or_else(
            || self.saved_selection(),
            |detail| detail.selected.min(detail.rows.len().saturating_sub(1)),
        )
    }

    pub(super) fn push(&mut self, character: char) {
        self.query.push(character);
        self.invalidate();
    }

    pub(super) fn pop(&mut self) {
        self.query.pop();
        self.invalidate();
    }

    pub(super) fn paste(&mut self, text: &str) {
        self.query.push_str(&text.replace(['\n', '\r'], " "));
        self.invalidate();
    }

    pub(super) fn clear(&mut self) {
        self.query.clear();
        self.invalidate();
    }

    pub(super) fn invalidate(&mut self) {
        self.cancel_search_tasks();
        self.cancel_detail_task();
        self.seq = self.seq.wrapping_add(1);
        self.committed_query = None;
        self.detail = None;
        self.songs.invalidate();
        self.artists.invalidate();
        self.albums.invalidate();
        self.playlists.invalidate();
        self.selections = [0; 4];
    }

    pub(super) fn submit(&mut self) -> Option<SearchRequest> {
        let query = self.query.trim().to_owned();
        if query.is_empty() {
            return None;
        }
        self.committed_query = Some(query.clone());
        if self.bucket_is_loading(self.channel, &query) {
            return None;
        }
        Some(self.begin_request(query, self.channel))
    }

    pub(super) fn select_channel(
        &mut self,
        channel: SearchChannel,
        selected: usize,
    ) -> (usize, Option<SearchRequest>) {
        self.remember_selection(selected);
        self.channel = channel;
        let selected = self.saved_selection();
        let request = self.committed_query.clone().and_then(|query| {
            (!self.bucket_loaded_or_loading(self.channel, &query))
                .then(|| self.begin_request(query, self.channel))
        });
        (selected, request)
    }

    fn begin_request(&mut self, query: String, channel: SearchChannel) -> SearchRequest {
        if let Some(task) = self.search_tasks[channel.index()].take() {
            task.abort();
        }
        self.seq = self.seq.wrapping_add(1);
        let request = SearchRequest {
            seq: self.seq,
            query,
            channel,
        };
        match channel {
            SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
                self.songs.begin(request.clone())
            }
            SearchChannel::Artists => self.artists.begin(request.clone()),
            SearchChannel::Albums => self.albums.begin(request.clone()),
            SearchChannel::Playlists => self.playlists.begin(request.clone()),
        }
        request
    }

    pub(super) fn attach_search_task(
        &mut self,
        seq: u64,
        channel: SearchChannel,
        task: AbortHandle,
    ) {
        let active_seq = match channel {
            SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
                self.songs.active.as_ref().map(|request| request.seq)
            }
            SearchChannel::Artists => self.artists.active.as_ref().map(|request| request.seq),
            SearchChannel::Albums => self.albums.active.as_ref().map(|request| request.seq),
            SearchChannel::Playlists => self.playlists.active.as_ref().map(|request| request.seq),
        };
        if active_seq == Some(seq) {
            if let Some(previous) = self.search_tasks[channel.index()].replace(task) {
                previous.abort();
            }
        } else {
            task.abort();
        }
    }

    fn cancel_search_tasks(&mut self) {
        for task in &mut self.search_tasks {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
    }

    fn bucket_loaded_or_loading(&self, channel: SearchChannel, query: &str) -> bool {
        match channel {
            SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
                self.songs.is_loaded_for(query) || self.songs.is_loading_for(query)
            }
            SearchChannel::Artists => {
                self.artists.is_loaded_for(query) || self.artists.is_loading_for(query)
            }
            SearchChannel::Albums => {
                self.albums.is_loaded_for(query) || self.albums.is_loading_for(query)
            }
            SearchChannel::Playlists => {
                self.playlists.is_loaded_for(query) || self.playlists.is_loading_for(query)
            }
        }
    }

    fn bucket_searching(&self, channel: SearchChannel) -> bool {
        match channel {
            SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
                self.songs.searching
            }
            SearchChannel::Artists => self.artists.searching,
            SearchChannel::Albums => self.albums.searching,
            SearchChannel::Playlists => self.playlists.searching,
        }
    }

    fn bucket_is_loading(&self, channel: SearchChannel, query: &str) -> bool {
        match channel {
            SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
                self.songs.is_loading_for(query)
            }
            SearchChannel::Artists => self.artists.is_loading_for(query),
            SearchChannel::Albums => self.albums.is_loading_for(query),
            SearchChannel::Playlists => self.playlists.is_loading_for(query),
        }
    }

    pub(super) fn accept(
        &mut self,
        seq: u64,
        query: &str,
        channel: SearchChannel,
        payload: SearchPayload,
    ) -> bool {
        let request = SearchRequest {
            seq,
            query: query.to_owned(),
            channel,
        };
        let accepted = match (channel, payload) {
            (SearchChannel::Songs, SearchPayload::Songs(page))
                if self.songs.matches(seq, query, channel) =>
            {
                self.songs.accept(&request, page);
                true
            }
            (SearchChannel::Artists, SearchPayload::Artists(page))
                if self.artists.matches(seq, query, channel) =>
            {
                self.artists.accept(&request, page);
                true
            }
            (SearchChannel::Albums, SearchPayload::Albums(page))
                if self.albums.matches(seq, query, channel) =>
            {
                self.albums.accept(&request, page);
                true
            }
            (SearchChannel::Playlists, SearchPayload::Playlists(page))
                if self.playlists.matches(seq, query, channel) =>
            {
                self.playlists.accept(&request, page);
                true
            }
            _ => false,
        };
        if accepted {
            self.search_tasks[channel.index()] = None;
            self.selections[channel.index()] = 0;
        }
        accepted
    }

    pub(super) fn fail(
        &mut self,
        seq: u64,
        query: &str,
        channel: SearchChannel,
        message: String,
    ) -> bool {
        let request = SearchRequest {
            seq,
            query: query.to_owned(),
            channel,
        };
        let accepted = match channel {
            SearchChannel::Songs if self.songs.matches(seq, query, channel) => {
                self.songs.fail(&request, message);
                true
            }
            SearchChannel::Artists if self.artists.matches(seq, query, channel) => {
                self.artists.fail(&request, message);
                true
            }
            SearchChannel::Albums if self.albums.matches(seq, query, channel) => {
                self.albums.fail(&request, message);
                true
            }
            SearchChannel::Playlists if self.playlists.matches(seq, query, channel) => {
                self.playlists.fail(&request, message);
                true
            }
            _ => false,
        };
        if accepted {
            self.search_tasks[channel.index()] = None;
            self.selections[channel.index()] = 0;
        }
        accepted
    }

    /// Opens a page for an id the caller already has — the playing track's
    /// artist or album — rather than one indexed out of the current results.
    /// It moves `channel` to match: `close_detail` resolves the return index
    /// against the detail's own bucket, so leaving the tab on Songs would
    /// apply an album index to the song list. `parent_selected` parks at 0
    /// because there is no parent row to come back to.
    pub(crate) fn open_detail_for(
        &mut self,
        channel: SearchChannel,
        id: i64,
        title: String,
    ) -> DetailRequest {
        self.cancel_detail_task();
        self.channel = channel;
        self.seq = self.seq.wrapping_add(1);
        let request = DetailRequest {
            seq: self.seq,
            channel,
            id,
        };
        self.detail = Some(SearchDetail {
            channel,
            id,
            title,
            rows: Vec::new(),
            error: None,
            searching: true,
            cover_url: None,
            seq: request.seq,
            parent_selected: 0,
            selected: 0,
        });
        self.input = false;
        request
    }

    pub(crate) fn open_detail(&mut self, selected: usize) -> Option<DetailRequest> {
        let (id, title, cover_url) = match self.channel {
            SearchChannel::Artists => self
                .artists
                .items
                .get(selected)
                .map(|item| (item.id, item.name.clone(), item.pic_url.clone())),
            SearchChannel::Albums => self
                .albums
                .items
                .get(selected)
                .map(|item| (item.id, item.name.clone(), item.pic_url.clone())),
            SearchChannel::Playlists => self
                .playlists
                .items
                .get(selected)
                .map(|item| (item.id, item.name.clone(), item.cover_url.clone())),
            SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => None,
        }?;
        self.cancel_detail_task();
        self.remember_selection(selected);
        self.seq = self.seq.wrapping_add(1);
        let request = DetailRequest {
            seq: self.seq,
            channel: self.channel,
            id,
        };
        self.detail = Some(SearchDetail {
            channel: self.channel,
            id,
            title,
            rows: Vec::new(),
            error: None,
            searching: true,
            cover_url,
            seq: request.seq,
            parent_selected: selected,
            selected: 0,
        });
        self.input = false;
        Some(request)
    }

    pub(super) fn attach_detail_task(&mut self, seq: u64, task: AbortHandle) {
        if self.detail.as_ref().is_some_and(|detail| detail.seq == seq) {
            self.cancel_detail_task();
            self.detail_task = Some(task);
        } else {
            task.abort();
        }
    }

    fn cancel_detail_task(&mut self) {
        if let Some(task) = self.detail_task.take() {
            task.abort();
        }
    }

    pub(super) fn close_detail(&mut self) -> Option<usize> {
        self.cancel_detail_task();
        let detail = self.detail.take()?;
        let (matching, len) = match detail.channel {
            SearchChannel::Artists => (
                self.artists
                    .items
                    .iter()
                    .position(|item| item.id == detail.id),
                self.artists.items.len(),
            ),
            SearchChannel::Albums => (
                self.albums
                    .items
                    .iter()
                    .position(|item| item.id == detail.id),
                self.albums.items.len(),
            ),
            SearchChannel::Playlists => (
                self.playlists
                    .items
                    .iter()
                    .position(|item| item.id == detail.id),
                self.playlists.items.len(),
            ),
            SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
                (None, self.songs.items.len())
            }
        };
        let selected =
            matching.unwrap_or_else(|| detail.parent_selected.min(len.saturating_sub(1)));
        self.selections[detail.channel.index()] = selected;
        Some(selected)
    }

    pub(crate) fn accept_detail(
        &mut self,
        seq: u64,
        channel: SearchChannel,
        id: i64,
        rows: Vec<SongRow>,
    ) -> bool {
        let Some(detail) = &mut self.detail else {
            return false;
        };
        if detail.seq != seq || detail.channel != channel || detail.id != id {
            return false;
        }
        self.detail_task = None;
        let cover_url = detail.cover_url.clone();
        detail.rows = rows
            .into_iter()
            .map(|mut row| {
                if row.pic_url.is_none() {
                    row.pic_url.clone_from(&cover_url);
                }
                row
            })
            .collect();
        detail.searching = false;
        detail.error = None;
        true
    }

    pub(super) fn fail_detail(
        &mut self,
        seq: u64,
        channel: SearchChannel,
        id: i64,
        message: String,
    ) -> bool {
        let Some(detail) = &mut self.detail else {
            return false;
        };
        if detail.seq != seq || detail.channel != channel || detail.id != id {
            return false;
        }
        self.detail_task = None;
        detail.rows.clear();
        detail.searching = false;
        detail.error = Some(message);
        true
    }
}

async fn search_action_with_timeout<F, E>(
    request: &SearchRequest,
    timeout: Duration,
    future: F,
) -> Action
where
    F: Future<Output = Result<SearchPayload, E>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(payload)) => Action::SearchResults {
            seq: request.seq,
            query: request.query.clone(),
            channel: request.channel,
            payload,
        },
        Ok(Err(_)) | Err(_) => Action::SearchFailed {
            seq: request.seq,
            query: request.query.clone(),
            channel: request.channel,
            message: i18n::t(Key::SearchFailed).into(),
        },
    }
}

async fn detail_action_with_timeout<F, E>(
    request: &DetailRequest,
    timeout: Duration,
    future: F,
) -> Action
where
    F: Future<Output = Result<Vec<SongRow>, E>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(rows)) => Action::SearchDetailLoaded {
            seq: request.seq,
            channel: request.channel,
            id: request.id,
            rows,
        },
        Ok(Err(_)) | Err(_) => Action::SearchDetailFailed {
            seq: request.seq,
            channel: request.channel,
            id: request.id,
            message: i18n::t(Key::SearchFailed).into(),
        },
    }
}

pub(super) fn spawn_search(fx: &Effects, request: SearchRequest) -> AbortHandle {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let action = search_action_with_timeout(
            &request,
            SEARCH_REQUEST_TIMEOUT,
            ncm.search_channel(&request.query, request.channel, 30),
        )
        .await;
        let _ = actions.send(action);
    })
    .abort_handle()
}

pub(super) fn spawn_search_detail(fx: &Effects, request: DetailRequest) -> AbortHandle {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let action = detail_action_with_timeout(&request, SEARCH_REQUEST_TIMEOUT, async {
            match request.channel {
                SearchChannel::Artists => ncm.artist_top_songs(request.id).await,
                SearchChannel::Albums => ncm.album_songs(request.id).await,
                SearchChannel::Playlists => ncm.playlist_detail_songs(request.id).await,
                SearchChannel::Songs | SearchChannel::MusicVideos | SearchChannel::Users => {
                    Ok(Vec::new())
                }
            }
        })
        .await;
        let _ = actions.send(action);
    })
    .abort_handle()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn channel_results_are_independent_and_loaded_lazily() {
        let mut state = SearchState::new();
        state.query = "周杰伦".into();
        let songs = state.submit().unwrap();
        assert_eq!(songs.channel, SearchChannel::Songs);
        assert!(state.accept(
            songs.seq,
            &songs.query,
            songs.channel,
            SearchPayload::Songs(SearchPage {
                items: vec![row(1)],
                total: 42,
            }),
        ));

        let (_, artists) = state.select_channel(SearchChannel::Artists, 0);
        let artists = artists.expect("first visit starts the artist search");
        assert_eq!(artists.channel, SearchChannel::Artists);
        assert_eq!(state.songs.items[0].id, 1);
        assert!(state.select_channel(SearchChannel::Artists, 0).1.is_none());
        assert!(state.select_channel(SearchChannel::Songs, 0).1.is_none());
        assert_eq!(state.current_len(), 1);
    }

    #[test]
    fn refreshing_a_channel_hides_stale_rows_until_the_response_arrives() {
        let mut state = SearchState::new();
        state.query = "query".into();
        let first = state.submit().unwrap();
        assert!(state.accept(
            first.seq,
            &first.query,
            first.channel,
            SearchPayload::Songs(SearchPage {
                items: vec![row(1)],
                total: 1,
            }),
        ));

        let refresh = state.submit().expect("same query can be refreshed");

        assert!(state.current_searching());
        assert_eq!(refresh.query, "query");
        assert_eq!(state.current_len(), 0);
        assert!(state.songs.items.is_empty());
    }

    #[test]
    fn stale_channel_and_detail_results_are_ignored() {
        let mut state = SearchState::new();
        state.query = "query".into();
        let songs = state.submit().unwrap();
        state.push('!');
        assert!(!state.accept(
            songs.seq,
            &songs.query,
            songs.channel,
            SearchPayload::Songs(SearchPage {
                items: vec![row(1)],
                total: 1,
            }),
        ));

        state.query = "artist".into();
        let (_, request) = state.select_channel(SearchChannel::Artists, 0);
        assert!(request.is_none());
        state.artists.items.push(ArtistHit {
            img1v1_url: None,
            id: 7,
            name: "Artist".into(),
            pic_url: None,
            album_count: 1,
            song_count: 2,
        });
        let detail = state.open_detail(0).unwrap();
        assert!(!state.accept_detail(detail.seq + 1, detail.channel, detail.id, vec![row(2)]));
        assert!(state.accept_detail(detail.seq, detail.channel, detail.id, vec![row(3)]));
        assert_eq!(state.song_rows().unwrap()[0].id, 3);
    }

    #[test]
    fn detail_back_restores_the_parent_selection() {
        let mut state = SearchState::new();
        state.channel = SearchChannel::Albums;
        state.albums.items = vec![
            AlbumHit {
                mark: 0,
                id: 1,
                name: "First".into(),
                artist: yesplaymusic_core::ncm::ArtistRef {
                    id: 0,
                    name: "Artist".into(),
                },
                pic_url: None,
                song_count: 1,
            },
            AlbumHit {
                mark: 0,
                id: 2,
                name: "Second".into(),
                artist: yesplaymusic_core::ncm::ArtistRef {
                    id: 0,
                    name: "Artist".into(),
                },
                pic_url: None,
                song_count: 2,
            },
        ];
        let request = state.open_detail(1).unwrap();
        assert_eq!(request.id, 2);
        assert_eq!(state.detail_title(), Some("Second"));
        assert_eq!(state.close_detail(), Some(1));
        assert!(state.is_results());
    }

    #[test]
    fn detail_back_tracks_the_entity_across_a_refreshed_parent_page() {
        let mut state = SearchState::new();
        state.channel = SearchChannel::Artists;
        state.artists.items = vec![
            ArtistHit {
                img1v1_url: None,
                id: 1,
                name: "First".into(),
                pic_url: None,
                album_count: 1,
                song_count: 1,
            },
            ArtistHit {
                img1v1_url: None,
                id: 2,
                name: "Second".into(),
                pic_url: None,
                album_count: 1,
                song_count: 1,
            },
        ];
        state.open_detail(1).unwrap();
        state.artists.items = vec![
            ArtistHit {
                img1v1_url: None,
                id: 2,
                name: "Second".into(),
                pic_url: None,
                album_count: 1,
                song_count: 1,
            },
            ArtistHit {
                img1v1_url: None,
                id: 3,
                name: "Third".into(),
                pic_url: None,
                album_count: 1,
                song_count: 1,
            },
        ];

        assert_eq!(state.close_detail(), Some(0));

        state.open_detail(0).unwrap();
        state.artists.items.clear();
        assert_eq!(state.close_detail(), Some(0));
    }

    #[test]
    fn a_detail_response_cannot_cross_a_back_and_new_push() {
        let mut state = SearchState::new();
        state.channel = SearchChannel::Artists;
        state.artists.items = vec![
            ArtistHit {
                img1v1_url: None,
                id: 1,
                name: "First".into(),
                pic_url: None,
                album_count: 1,
                song_count: 1,
            },
            ArtistHit {
                img1v1_url: None,
                id: 2,
                name: "Second".into(),
                pic_url: Some("https://example.test/cover.jpg".into()),
                album_count: 1,
                song_count: 1,
            },
        ];
        let first = state.open_detail(0).unwrap();
        state.close_detail();
        let second = state.open_detail(1).unwrap();

        assert!(!state.accept_detail(first.seq, first.channel, first.id, vec![row(1)]));
        assert_eq!(state.detail_title(), Some("Second"));
        assert!(state.accept_detail(second.seq, second.channel, second.id, vec![row(2)]));
        assert_eq!(state.song_rows().unwrap()[0].id, 2);
        assert_eq!(
            state.song_rows().unwrap()[0].pic_url.as_deref(),
            Some("https://example.test/cover.jpg")
        );
    }

    #[tokio::test]
    async fn leaving_a_detail_page_aborts_its_in_flight_request() {
        let mut state = SearchState::new();
        state.channel = SearchChannel::Artists;
        state.artists.items.push(ArtistHit {
            img1v1_url: None,
            id: 7,
            name: "Artist".into(),
            pic_url: None,
            album_count: 1,
            song_count: 2,
        });
        let request = state.open_detail(0).unwrap();
        let task = tokio::spawn(std::future::pending::<()>());
        state.attach_detail_task(request.seq, task.abort_handle());

        state.close_detail();

        assert!(task.await.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn editing_the_query_aborts_obsolete_search_requests() {
        let mut state = SearchState::new();
        state.query = "first".into();
        let request = state.submit().unwrap();
        let task = tokio::spawn(std::future::pending::<()>());
        state.attach_search_task(request.seq, request.channel, task.abort_handle());

        state.push('!');

        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!state.current_searching());
    }

    #[tokio::test]
    async fn pending_search_maps_a_short_timeout_to_search_failed() {
        let request = SearchRequest {
            seq: 17,
            query: "pending".into(),
            channel: SearchChannel::Playlists,
        };

        let action = search_action_with_timeout(
            &request,
            Duration::from_millis(1),
            std::future::pending::<Result<SearchPayload, ()>>(),
        )
        .await;

        let Action::SearchFailed {
            seq,
            query,
            channel,
            message,
        } = action
        else {
            panic!("search timeout returned the wrong action");
        };
        assert_eq!(seq, request.seq);
        assert_eq!(query, request.query);
        assert_eq!(channel, request.channel);
        assert_eq!(message, i18n::t(Key::SearchFailed));
    }

    #[tokio::test]
    async fn pending_detail_maps_a_short_timeout_to_detail_failed() {
        let request = DetailRequest {
            seq: 23,
            channel: SearchChannel::Albums,
            id: 42,
        };

        let action = detail_action_with_timeout(
            &request,
            Duration::from_millis(1),
            std::future::pending::<Result<Vec<SongRow>, ()>>(),
        )
        .await;

        let Action::SearchDetailFailed {
            seq,
            channel,
            id,
            message,
        } = action
        else {
            panic!("detail timeout returned the wrong action");
        };
        assert_eq!(seq, request.seq);
        assert_eq!(channel, request.channel);
        assert_eq!(id, request.id);
        assert_eq!(message, i18n::t(Key::SearchFailed));
    }
}
