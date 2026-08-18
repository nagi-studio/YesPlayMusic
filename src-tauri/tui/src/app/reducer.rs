use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};

use crate::action::{Action, CoverSurface, SelfUpdateOutcome, View};
use crate::api::Source;
use crate::event;
use crate::i18n::Key;
use crate::player::PlayerCommand;

use super::search::spawn_search_detail;
use super::{
    apply_pixel_cover,
    command_palette::{CommandError, CommandInvocation},
    song_row_from_resolved, spawn_cover_prefetch, spawn_render_idle, spawn_resolve, AppState,
    CoverLoad, CoverStyle, Effects, PlaybackModeSlot, SelfUpdate, PREVIEW_CELLS,
};

impl AppState {
    pub(super) fn update(&mut self, action: Action, fx: &Effects) {
        if matches!(action, Action::UiTick) {
            self.advance_marquee();
            self.advance_command_feedback();
            return;
        }
        // Ctrl+L repaints through every modal and input owner: it exists to
        // heal a desynced screen, so nothing may swallow it.
        if let Action::RawKey(key) = &action {
            if matches!(event::key_action(*key), Some(Action::ForceRedraw)) {
                self.force_redraw = true;
                return;
            }
        }
        // The command palette is the topmost input owner. Background async
        // results still reduce normally while keys, paste, and mouse stay modal.
        let action = if self.command_palette.open {
            match action {
                Action::RawKey(key) => {
                    self.handle_command_palette_key(key, fx);
                    return;
                }
                Action::Paste(text) => {
                    self.clear_command_feedback();
                    self.command_palette.paste(&text);
                    return;
                }
                Action::Mouse(_) => return,
                action => action,
            }
        } else {
            action
        };
        // The quit dialog owns keyboard input, while background results
        // keep flowing through the normal reducer.
        let action = if self.confirm_quit {
            match action {
                Action::RawKey(key) => {
                    match event::key_action(key) {
                        Some(Action::ConfirmYes | Action::Quit | Action::Activate) => {
                            self.should_quit = true;
                        }
                        Some(Action::Back | Action::Escape | Action::NextTrack) => {
                            self.confirm_quit = false;
                        }
                        _ => {}
                    }
                    return;
                }
                Action::ConfirmYes | Action::Quit | Action::Activate => {
                    self.should_quit = true;
                    return;
                }
                Action::Back | Action::Escape | Action::NextTrack => {
                    self.confirm_quit = false;
                    return;
                }
                action => action,
            }
        } else {
            action
        };
        if self.terminal_size.1 < 8 {
            if let Action::RawKey(key) = &action {
                if let Some(
                    mapped @ (Action::TogglePlay
                    | Action::NextTrack
                    | Action::PrevTrack
                    | Action::ToggleLike
                    | Action::OpenCommandPalette
                    | Action::Quit),
                ) = event::key_action(*key)
                {
                    self.show_help = false;
                    self.update(mapped, fx);
                    return;
                }
            }
        }
        // The help overlay is modal: any key dismisses it, and so does a
        // click — but never a bare pointer move, which would close the map
        // the moment the hand brushes the mouse.
        if self.show_help {
            let dismissed = match &action {
                Action::RawKey(_) => true,
                Action::Mouse(mouse) => matches!(mouse.kind, MouseEventKind::Down(_)),
                _ => false,
            };
            if dismissed {
                self.show_help = false;
                return;
            }
            if matches!(action, Action::Mouse(_)) {
                return;
            }
        }
        // Text-input mode: the search box owns the keyboard.
        if let Action::RawKey(key) = &action {
            if self.view == View::Settings {
                if let Some(mapped) = event::settings_key_action(*key) {
                    self.update(mapped, fx);
                }
                return;
            }
            if self.view == View::Search && self.search.input && !self.confirm_quit {
                self.handle_search_key(fx, *key);
                return;
            }
            if self.filter.input {
                self.handle_filter_key(*key);
                return;
            }
            if self.view == View::Search && self.handle_search_channel_key(fx, *key) {
                return;
            }
            let Some(mapped) = event::key_action(*key) else {
                return;
            };
            tracing::debug!(?key, ?mapped, "key resolved");
            self.update(mapped, fx);
            return;
        }
        if let Action::Paste(text) = &action {
            if self.view == View::Search && self.search.input {
                self.search.paste(text);
            } else if self.filter.input {
                self.filter.paste(text);
                self.selected = 0;
            }
            return;
        }
        // vim gg: a second bare `g` right after the first jumps to the top.
        let was_pending_g = self.pending_g;
        self.pending_g = false;
        match action {
            Action::GKey => {
                if was_pending_g {
                    if self.view == View::Settings {
                        self.settings.selected = 0;
                    } else if self.view == View::Library && self.sidebar_focus {
                        self.sidebar_selected = 0;
                    } else {
                        self.selected = 0;
                    }
                } else {
                    self.pending_g = true;
                }
            }
            Action::JumpTop => {
                if self.view == View::Settings {
                    self.settings.selected = 0;
                } else if self.view == View::Library && self.sidebar_focus {
                    self.sidebar_selected = 0;
                } else {
                    self.selected = 0;
                }
            }
            Action::JumpBottom => {
                if self.view == View::Settings {
                    let len = super::settings::SettingField::ALL.len();
                    self.settings.selected = len.saturating_sub(1);
                } else if self.view == View::Library && self.sidebar_focus {
                    self.sidebar_selected = crate::ui::library::SOURCES.len() - 1;
                } else {
                    self.selected = self.visible_len().saturating_sub(1);
                }
            }
            Action::ConfirmYes => {}
            Action::ForceRedraw => self.force_redraw = true,
            Action::Quit => self.confirm_quit = true,
            Action::OpenCommandPalette => {
                self.show_help = false;
                self.clear_command_feedback();
                self.command_palette.open();
            }
            Action::CloseCommandPalette => self.command_palette.close(),
            Action::MoveCommandSelection(delta) => self.command_palette.move_selection(delta),
            Action::ExecuteCommand => self.execute_command_palette(fx),
            Action::SwitchView(view) => {
                if view == View::NowPlaying {
                    self.dashboard_hold = false;
                }
                if self.view == View::Search {
                    let selected = self
                        .visible_row(self.selected)
                        .map_or(self.selected, |(underlying, _)| underlying);
                    self.search.remember_selection(selected);
                }
                self.clear_filter();
                if view == View::Settings {
                    self.open_settings(fx);
                } else {
                    if self.view == View::Settings {
                        self.cancel_settings(fx);
                    }
                    self.view = view;
                }
                if view == View::Search {
                    self.search.input = self.search.is_results();
                    self.selected = self.search.page_selection();
                }
            }
            Action::Back => self.navigate_back(fx),
            Action::Escape => {
                if self.filter.is_active() {
                    self.clear_filter();
                } else if self.view == View::Search
                    && self.search.is_results()
                    && !self.search.input
                {
                    // Esc keeps walking outward from the result list;
                    // navigate_back would bounce focus back into the input
                    // and Esc could never leave the view. h/Backspace keep
                    // the step-back-into-input behavior.
                    let selected = self
                        .visible_row(self.selected)
                        .map_or(self.selected, |(underlying, _)| underlying);
                    self.search.remember_selection(selected);
                    self.clear_filter();
                    self.sidebar_focus = false;
                    self.view = View::NowPlaying;
                } else {
                    self.navigate_back(fx);
                }
            }
            Action::ToggleZen => {
                self.zen = !self.zen;
                if self.zen {
                    self.view = View::NowPlaying;
                    self.dashboard_hold = false;
                }
            }
            Action::ToggleSpectrum => {
                if let Err(message) = self.toggle_spectrum(fx) {
                    self.status = Some(message);
                }
            }
            Action::TogglePlay => self.toggle_play(fx),
            Action::SeekBy(seconds) => {
                let step = Duration::from_secs(seconds.unsigned_abs());
                let target = if seconds >= 0 {
                    self.position.saturating_add(step)
                } else {
                    self.position.saturating_sub(step)
                };
                tracing::debug!(
                    seconds,
                    from_ms = self.position.as_millis() as u64,
                    "seek by"
                );
                fx.player.send(PlayerCommand::SeekTo(target));
            }
            Action::SeekToRatio(ratio) => {
                if let Some(duration) = self.duration {
                    let target = duration.mul_f64(ratio.clamp(0.0, 1.0));
                    fx.player.send(PlayerCommand::SeekTo(target));
                    // Show the jump immediately; the player event confirms it.
                    self.position = target;
                }
            }
            Action::VolumeBy(delta) => self.set_volume(fx, self.volume + delta),
            Action::ToggleMute => self.toggle_mute(fx),
            Action::MoveSelection(delta) => {
                if self.view == View::Settings {
                    self.move_setting_selection(delta);
                    return;
                }
                if self.view == View::Library && self.sidebar_focus {
                    let next = (self.sidebar_selected as i32 + delta.signum()).clamp(0, 3);
                    self.sidebar_selected = next as usize;
                    return;
                }
                let len = self.visible_len();
                if len > 0 {
                    let last = len as i32 - 1;
                    let next = (self.selected as i32 + delta).clamp(0, last);
                    self.selected = next as usize;
                }
            }
            Action::MovePage(pages) => {
                if self.view == View::Library && self.sidebar_focus {
                    return;
                }
                let len = self.visible_len();
                if len > 0 {
                    let delta = pages.saturating_mul(self.list_page_size() as i32);
                    let next = (self.selected as i32 + delta).clamp(0, len as i32 - 1);
                    self.selected = next as usize;
                }
            }
            Action::Activate => match self.view {
                View::Settings => self.save_settings(fx),
                View::Library if self.sidebar_focus => {
                    self.open_source(fx, self.sidebar_selected);
                }
                View::Library => {
                    if let Some((_, row)) = self.visible_row(self.selected) {
                        if self.enter_replaces_queue {
                            self.queue = self.visible_rows_owned();
                            self.queue_pos = Some(self.selected);
                        } else {
                            self.queue = vec![row.clone()];
                            self.queue_pos = Some(0);
                        }
                        self.queue_source = self.library_source;
                        if self.queue_source != Source::Fm {
                            self.pending_fm_next = false;
                            self.fm_request_pending = false;
                        }
                        self.reset_shuffle_order();
                        self.play_row(fx, row);
                        self.clear_filter();
                        self.view = View::NowPlaying;
                    }
                }
                View::Queue => {
                    if let Some((underlying, row)) = self.visible_row(self.selected) {
                        self.queue_pos = Some(underlying);
                        self.reset_shuffle_order();
                        self.play_row(fx, row);
                        self.clear_filter();
                        self.view = View::NowPlaying;
                    }
                }
                View::Search if !self.search.input => {
                    if self.search.song_rows().is_some() {
                        let Some((underlying, row)) = self.visible_row(self.selected) else {
                            return;
                        };
                        self.queue = self.visible_rows_owned();
                        self.queue_pos = Some(self.selected);
                        self.queue_source = Source::Search;
                        self.pending_fm_next = false;
                        self.fm_request_pending = false;
                        self.reset_shuffle_order();
                        self.search.remember_selection(underlying);
                        self.play_row(fx, row);
                        self.clear_filter();
                        self.view = View::NowPlaying;
                    } else if let Some(request) = self.search.open_detail(self.selected) {
                        self.clear_filter();
                        let seq = request.seq;
                        let task = spawn_search_detail(fx, request);
                        self.search.attach_detail_task(seq, task);
                    }
                }
                _ => {}
            },
            Action::AddSelectedToQueue => {
                if let Some((_, row)) = self.visible_row(self.selected) {
                    self.queue.push(row);
                    self.shuffle_order.clear();
                    self.status = Some(crate::i18n::t(Key::AddedToQueue).into());
                }
            }
            Action::NextTrack => {
                // Reveal the player even when the queue cannot advance,
                // or the resulting status stays hidden behind the dashboard.
                self.dashboard_hold = false;
                self.step_queue(fx, 1, false, true);
            }
            Action::PrevTrack => {
                self.dashboard_hold = false;
                self.step_queue(fx, -1, false, true);
            }
            Action::ToggleHelp => self.show_help = true,
            Action::ToggleShuffle => {
                self.shuffle = !self.shuffle;
                self.reset_shuffle_order();
            }
            Action::CycleRepeat => {
                self.play_mode = self.play_mode.next();
            }
            Action::CyclePlaybackMode => {
                (self.shuffle, self.play_mode) =
                    PlaybackModeSlot::from_parts(self.shuffle, self.play_mode).next_parts();
                self.reset_shuffle_order();
            }
            Action::StartFilter => {
                if matches!(self.view, View::Library | View::Queue)
                    || self.view == View::Search
                        && !self.search.input
                        && self.search.song_rows().is_some()
                {
                    self.filter.start();
                    self.selected = 0;
                    if self.view == View::Library {
                        self.sidebar_focus = false;
                    }
                }
            }
            Action::SelectSearchChannel(channel) => {
                if self.view == View::Search && self.search.is_results() {
                    self.select_search_channel(fx, channel);
                }
            }
            Action::ToggleLibraryFocus => {
                if self.view == View::Library && self.sidebar_visible() {
                    self.sidebar_focus = !self.sidebar_focus;
                    if self.sidebar_focus {
                        self.sidebar_selected = self.source_index();
                    }
                }
            }
            Action::SetVolumeTo(ratio) => self.set_volume(fx, ratio.clamp(0.0, 1.0)),
            Action::ToggleLike => self.toggle_like(fx),
            Action::StartPersonalFm => self.start_personal_fm(fx),
            Action::TrashFmTrack => self.trash_current_fm_track(fx),
            Action::OpenSource(index) => self.open_source(fx, index),
            Action::SelectSetting(index) => self.select_setting(index),
            Action::AdjustSetting(delta) => self.adjust_setting(fx, delta),
            Action::SaveSettings => self.save_settings(fx),
            Action::CancelSettings => self.cancel_settings(fx),
            Action::NerdFontProbeFinished(status) => self.apply_nerd_font_probe(status),
            Action::UpdateAvailable(tag) => {
                // Also surface as a status toast: a late-arriving result
                // must stay visible after the dashboard has been left.
                self.status = Some(crate::i18n::t_update_available(&tag));
                self.update_available = Some(tag);
            }
            Action::OpenPage(link) => self.open_page(fx, link),
            Action::StartSelfUpdate => self.start_self_update(fx),
            Action::SelfUpdateProgress(line) => {
                self.status = Some(line.clone());
                self.self_update = SelfUpdate::Running(line);
            }
            Action::SelfUpdateFinished(outcome) => match outcome {
                SelfUpdateOutcome::Installed => {
                    self.self_update = SelfUpdate::Installed;
                    self.status = Some(crate::i18n::t_update_restart_now().into());
                }
                SelfUpdateOutcome::UpToDate => {
                    self.self_update = SelfUpdate::Idle;
                    self.update_available = None;
                    self.status = Some(crate::i18n::t_update_up_to_date().into());
                }
                SelfUpdateOutcome::Failed(reason) => {
                    self.self_update = SelfUpdate::Idle;
                    self.status = Some(crate::i18n::t_update_failed(&reason));
                }
            },
            Action::LikedIds { session, ids } => self.apply_liked_ids(session, ids),
            Action::FmMore { session, rows } => self.apply_fm_more(fx, session, rows),
            Action::FmLoadFailed { session, message } => {
                self.apply_fm_load_failed(session, message);
            }
            Action::FmTrashFinished { session, message } => {
                self.apply_fm_trash_finished(session, message);
            }
            Action::SelectIndex(index) => {
                let len = self.visible_len();
                if index < len {
                    self.selected = index;
                    self.filter.input = false;
                    match self.view {
                        View::Library => self.sidebar_focus = false,
                        View::Search => self.search.input = false,
                        _ => {}
                    }
                }
            }
            Action::Mouse(_) => {} // resolved against Hits in the event loop
            Action::RawKey(_) | Action::Paste(_) | Action::UiTick => {} // handled before this match
            Action::StartLogin => self.start_login(fx),
            Action::LoginQrReady { attempt, art } => self.apply_login_qr(attempt, art),
            Action::LoginProgress { attempt, message } => {
                self.apply_login_progress(attempt, message);
            }
            Action::LoginFailed { attempt, message } => {
                self.apply_login_failed(attempt, message);
            }
            Action::LoginSucceeded {
                attempt,
                session,
                uid,
                nickname,
            } => self.apply_login_succeeded(fx, attempt, session, uid, nickname),
            Action::SessionRestored {
                epoch,
                uid,
                nickname,
            } => self.apply_session_restored(fx, epoch, uid, nickname),
            Action::SessionRestoreFailed { epoch, failure } => {
                self.apply_session_restore_failed(fx, epoch, failure);
            }
            Action::LibraryLoaded {
                session,
                request,
                source,
                rows,
            } => self.apply_library_loaded(fx, session, request, source, rows),
            Action::LibraryFailed {
                session,
                request,
                message,
            } => self.apply_library_failed(session, request, message),
            Action::SearchResults {
                seq,
                query,
                channel,
                payload,
            } => {
                let is_current = self.view == View::Search
                    && self.search.is_results()
                    && self.search.channel == channel;
                if self.search.accept(seq, &query, channel, payload)
                    && is_current
                    && self.search.current_len() > 0
                {
                    self.selected = 0;
                }
            }
            Action::SearchFailed {
                seq,
                query,
                channel,
                message,
            } => {
                self.search.fail(seq, &query, channel, message);
            }
            Action::SearchDetailLoaded {
                seq,
                channel,
                id,
                rows,
            } => {
                let is_current = self.view == View::Search && !self.search.is_results();
                if self.search.accept_detail(seq, channel, id, rows) && is_current {
                    self.selected = 0;
                }
            }
            Action::SearchDetailFailed {
                seq,
                channel,
                id,
                message,
            } => {
                self.search.fail_detail(seq, channel, id, message);
            }
            Action::LikeFinished {
                session,
                id,
                mutation,
                attempted_like,
                error,
            } => self.apply_like_finished(fx, session, id, mutation, attempted_like, error),
            Action::LyricsLoaded { generation, lines } => {
                if generation == self.generation {
                    self.lyrics = lines;
                }
            }
            Action::TrackResolved { generation, track } => {
                if generation == self.generation {
                    self.prepare_resolved(fx, generation, track);
                }
            }
            Action::RowCacheReady {
                generation,
                row,
                lease,
            } => {
                if generation == self.generation {
                    if let Some(lease) = lease {
                        self.apply_cached(fx, generation, row, lease);
                    } else {
                        spawn_resolve(fx, generation, row);
                    }
                }
            }
            Action::ResolvedCacheReady {
                generation,
                track,
                lease,
            } => {
                if generation == self.generation {
                    if let Some(lease) = lease {
                        let row = song_row_from_resolved(&track);
                        self.apply_cached(fx, generation, row, lease);
                    } else {
                        self.apply_resolved(fx, generation, track);
                    }
                }
            }
            Action::CacheFallbackResolved { generation, track } => {
                if generation == self.generation {
                    self.apply_resolved(fx, generation, track);
                }
            }
            Action::PrefetchReady { index, track } => {
                // Guard against a rebuilt queue: only keep it if the row
                // at that index is still the same song and the quality
                // request still matches the current setting.
                if self.queue.get(index).is_some_and(|row| row.id == track.id)
                    && track.cache_key.quality == fx.ncm.quality()
                {
                    self.prefetched = Some((index, track));
                }
            }
            Action::ResolveFailed {
                generation,
                message,
            } => {
                if generation == self.generation {
                    self.status = Some(message);
                }
            }
            Action::TrackUnavailable { generation } => {
                if generation == self.generation {
                    self.handle_track_unavailable(fx);
                }
            }
            Action::CoverLoaded { request, cover } => {
                let desired_playing = self.desired_cover_cells();
                match request.surface {
                    CoverSurface::Playing => apply_pixel_cover(
                        &mut self.cover,
                        self.generation,
                        desired_playing,
                        self.style_revision,
                        request,
                        cover,
                    ),
                    CoverSurface::Selection => {
                        if request.generation == self.selected_cover.generation
                            && request.cells == PREVIEW_CELLS
                            && request.style_revision == self.style_revision
                        {
                            let pixel_key = self.pixel_cover_key(&request);
                            self.hot_pixel_covers
                                .insert(pixel_key.clone(), cover.clone());
                            self.selected_cover.pixel = Some(cover);
                            self.selected_cover.pixel_key = Some(pixel_key);
                        }
                    }
                    CoverSurface::Bar => apply_pixel_cover(
                        &mut self.bar_cover,
                        self.generation,
                        crate::ui::now_playing::player_bar_cover_cells(self.terminal_size.1),
                        self.style_revision,
                        request,
                        cover,
                    ),
                }
            }
            Action::CoverLoadFailed {
                surface,
                generation,
            } => match surface {
                CoverSurface::Playing if generation == self.generation => {
                    self.cover = None;
                    if let Some(original) = &mut self.original_cover {
                        original.clear();
                    }
                }
                CoverSurface::Bar if generation == self.generation => {
                    self.bar_cover = None;
                    if let Some(original) = &mut self.bar_original_cover {
                        original.clear();
                    }
                }
                _ => {}
            },
            Action::CoverDecoded {
                surface,
                generation,
                style_revision,
                image,
            } => {
                if style_revision == self.style_revision {
                    match surface {
                        CoverSurface::Playing if generation == self.generation => {
                            if let Some(original) = &mut self.original_cover {
                                original.replace(generation, image);
                            }
                        }
                        CoverSurface::Selection if generation == self.selected_cover.generation => {
                            if let Some(original) = &mut self.selected_original_cover {
                                original.replace(generation, image);
                            }
                        }
                        CoverSurface::Bar if generation == self.generation => {
                            if let Some(original) = &mut self.bar_original_cover {
                                original.replace(generation, image);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Action::SelectionCoverDue {
                generation,
                row,
                neighbors,
                needs_network,
            } => {
                let current = self.selected_cover.key.as_ref();
                if generation == self.selected_cover.generation
                    && current.is_some_and(|key| {
                        key.id == row.id && key.pic_url.as_ref() == row.pic_url.as_ref()
                    })
                {
                    if needs_network && !self.selection_cover_is_ready(&row) {
                        if let Some(pic_url) = row.pic_url.as_deref() {
                            let request = self.cover_request(
                                CoverSurface::Selection,
                                generation,
                                row.id,
                                pic_url,
                                PREVIEW_CELLS,
                            );
                            let load = CoverLoad {
                                request,
                                style: CoverStyle {
                                    pixel: self.pixel_style(),
                                    original: self.uses_original_cover(CoverSurface::Selection),
                                },
                            };
                            super::spawn_cover_download(fx, load, pic_url.to_owned());
                        }
                    }
                    spawn_cover_prefetch(
                        fx,
                        generation,
                        self.style_revision,
                        neighbors,
                        CoverStyle {
                            pixel: self.pixel_style(),
                            original: self.uses_original_cover(CoverSurface::Selection),
                        },
                    );
                }
            }
            Action::SelectionCoverWarmed {
                generation,
                style_revision,
                pixel_key,
                cover,
            } => {
                if generation == self.selected_cover.generation
                    && style_revision == self.style_revision
                {
                    self.hot_pixel_covers.insert(pixel_key, cover);
                }
            }
            Action::IdleArtBytes { bytes } => {
                self.idle_bytes = Some(bytes.clone());
                spawn_render_idle(
                    fx,
                    bytes,
                    self.desired_idle_cells(),
                    self.pixel_style(),
                    self.style_revision,
                );
            }
            Action::IdleArtLoaded {
                cells,
                style_revision,
                cover,
            } => {
                if cells == self.desired_idle_cells() && style_revision == self.style_revision {
                    self.idle_art = cover;
                }
            }
            Action::Player(event) => {
                self.apply_player_event(fx, event);
                if self.pending_auto_next {
                    self.pending_auto_next = false;
                    self.step_queue(fx, 1, true, true);
                }
            }
            Action::Resize { cols, rows } => {
                self.terminal_size = (cols, rows);
                if !self.sidebar_visible() {
                    self.sidebar_focus = false;
                }
                let desired = self.desired_cover_cells();
                let current = self.cover.as_ref().map(|cover| (cover.width, cover.height));
                // The bar cover scales with the terminal too: a height change
                // that crosses a bar-size step needs a re-render as well.
                let bar_desired = crate::ui::now_playing::player_bar_cover_cells(rows);
                let bar_stale = self
                    .bar_cover
                    .as_ref()
                    .is_some_and(|cover| (cover.width, cover.height) != bar_desired);
                if current != Some(desired) || bar_stale {
                    self.cover = None;
                    self.bar_cover = None;
                    if let Some(row) = self.active_row.clone() {
                        self.load_playing_cover(fx, &row);
                    }
                }
                let desired = self.desired_idle_cells();
                if (self.idle_art.width, self.idle_art.height) != desired {
                    if let Some(bytes) = self.idle_bytes.clone() {
                        spawn_render_idle(
                            fx,
                            bytes,
                            desired,
                            self.pixel_style(),
                            self.style_revision,
                        );
                    }
                }
                if self.now.is_some() {
                    self.ensure_placeholder();
                }
            }
        }
    }

    /// Opens the artist or album page a click landed on. The link travels
    /// from the frame that was clicked, so it stays correct even if the track
    /// changed between the click and this reduction.
    fn open_page(&mut self, fx: &Effects, link: crate::ui::PageLink) {
        self.clear_filter();
        // Zen hides every view but the player, so a page opened underneath it
        // would be invisible; leaving zen is the only way the click can mean
        // anything.
        self.zen = false;
        let request = self
            .search
            .open_detail_for(link.channel, link.id, link.title);
        let seq = request.seq;
        let task = spawn_search_detail(fx, request);
        self.search.attach_detail_task(seq, task);
        self.view = View::Search;
    }

    /// Runs the pipeline behind `ypm update` without leaving the app. The
    /// release check repeats here so `U` also works before the startup check
    /// has landed — and so a stale tag can never be installed twice.
    fn start_self_update(&mut self, fx: &Effects) {
        if self.self_update != SelfUpdate::Idle {
            return;
        }
        let brew = self.brew_install;
        if !brew {
            if let Err(error) = crate::self_update::preflight() {
                self.status = Some(crate::i18n::t_update_failed(&error.to_string()));
                return;
            }
        }
        let known = self.update_available.clone();
        let checking = crate::i18n::t_update_checking();
        self.self_update = SelfUpdate::Running(checking.into());
        self.status = Some(checking.into());

        let progress = fx.actions.clone();
        let finished = fx.actions.clone();
        tokio::spawn(async move {
            let tag = match known {
                Some(tag) => Some(tag),
                None => crate::update::check(env!("CARGO_PKG_VERSION")).await,
            };
            let Some(tag) = tag else {
                let _ = finished.send(Action::SelfUpdateFinished(SelfUpdateOutcome::UpToDate));
                return;
            };
            if brew {
                let _ = progress.send(Action::SelfUpdateProgress(
                    crate::i18n::t_update_brew_refreshing().into(),
                ));
                let result = async {
                    crate::self_update::brew_refresh().await?;
                    let _ = progress.send(Action::SelfUpdateProgress(
                        crate::i18n::t_update_brew_upgrading().into(),
                    ));
                    crate::self_update::brew_upgrade().await
                }
                .await;
                let _ = finished.send(Action::SelfUpdateFinished(match result {
                    Ok(()) => SelfUpdateOutcome::Installed,
                    Err(error) => SelfUpdateOutcome::Failed(error.to_string()),
                }));
                return;
            }
            // One redraw per distinct line: a redraw per chunk would repaint
            // the whole TUI thousands of times over a single download.
            let label = crate::i18n::t_update_download_label();
            let mut last: Option<String> = None;
            let result = crate::self_update::install(&tag, &mut |stage| {
                let line = match stage {
                    crate::self_update::Stage::Downloading { done, total } => {
                        let percent = total
                            .filter(|total| *total > 0)
                            .map(|total| done * 100 / total);
                        crate::i18n::t_update_in_progress(label, percent)
                    }
                    crate::self_update::Stage::Verifying => {
                        crate::i18n::t_update_verifying().to_owned()
                    }
                    crate::self_update::Stage::Installing => {
                        crate::i18n::t_update_installing().to_owned()
                    }
                };
                if last.as_deref() == Some(line.as_str()) {
                    return;
                }
                last = Some(line.clone());
                let _ = progress.send(Action::SelfUpdateProgress(line));
            })
            .await;
            let outcome = match result {
                Ok(_) => SelfUpdateOutcome::Installed,
                Err(error) => SelfUpdateOutcome::Failed(error.to_string()),
            };
            let _ = finished.send(Action::SelfUpdateFinished(outcome));
        });
    }

    pub(super) fn navigate_back(&mut self, fx: &Effects) {
        let search_selected = if self.view == View::Search {
            self.visible_row(self.selected)
                .map_or(self.selected, |(underlying, _)| underlying)
        } else {
            0
        };
        self.clear_filter();
        if self.view == View::Settings {
            self.cancel_settings(fx);
        } else if self.view == View::Search && !self.search.is_results() {
            self.selected = self.search.close_detail().unwrap_or_default();
            self.search.input = false;
        } else if self.view == View::Search && !self.search.input {
            self.search.remember_selection(search_selected);
            self.selected = search_selected;
            self.search.input = true;
        } else if self.view == View::Library && !self.sidebar_focus && self.sidebar_visible() {
            self.sidebar_focus = true;
            self.sidebar_selected = self.source_index();
        } else {
            self.sidebar_focus = false;
            self.view = View::NowPlaying;
        }
    }

    fn set_volume(&mut self, fx: &Effects, volume: f32) {
        // Stepping down from any level must land on exact zero, not float dust.
        let volume = volume.clamp(0.0, 1.5);
        let volume = if volume < 0.005 { 0.0 } else { volume };
        if volume == 0.0 && self.volume > 0.0 {
            self.volume_before_mute = Some(self.volume);
        } else if volume > 0.0 {
            self.volume_before_mute = None;
        }
        self.volume = volume;
        fx.player.send(PlayerCommand::SetVolume(volume));
    }

    fn toggle_mute(&mut self, fx: &Effects) {
        if self.volume > 0.0 {
            let previous = self.volume;
            self.set_volume(fx, 0.0);
            self.volume_before_mute = Some(previous);
        } else {
            let volume = self.volume_before_mute.take().unwrap_or(1.0);
            self.set_volume(fx, volume);
        }
    }

    fn handle_command_palette_key(&mut self, key: crossterm::event::KeyEvent, fx: &Effects) {
        if let Some(action) = event::command_palette_key_action(key) {
            self.update(action, fx);
            return;
        }
        match key.code {
            KeyCode::Backspace => {
                self.clear_command_feedback();
                self.command_palette.backspace();
            }
            KeyCode::Home => self.command_palette.select_first(),
            KeyCode::End => self.command_palette.select_last(),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.clear_command_feedback();
                self.command_palette.push(character);
            }
            _ => {}
        }
    }

    fn execute_command_palette(&mut self, fx: &Effects) {
        let invocation = match self.command_palette.invocation() {
            Ok(invocation) => invocation,
            Err(error) => {
                self.set_command_feedback(command_error_message(error), true);
                return;
            }
        };
        let name = invocation.name();
        self.command_palette.close();

        let result = match invocation {
            CommandInvocation::TogglePlay => {
                self.update(Action::TogglePlay, fx);
                Ok(())
            }
            CommandInvocation::Next => {
                self.update(Action::NextTrack, fx);
                Ok(())
            }
            CommandInvocation::Prev => {
                self.update(Action::PrevTrack, fx);
                Ok(())
            }
            CommandInvocation::Shuffle => {
                self.update(Action::ToggleShuffle, fx);
                Ok(())
            }
            CommandInvocation::Repeat => {
                self.update(Action::CycleRepeat, fx);
                Ok(())
            }
            CommandInvocation::Like => {
                self.update(Action::ToggleLike, fx);
                Ok(())
            }
            CommandInvocation::PersonalFm => {
                self.update(Action::StartPersonalFm, fx);
                Ok(())
            }
            CommandInvocation::FmTrash => {
                self.update(Action::TrashFmTrack, fx);
                Ok(())
            }
            CommandInvocation::Zen => {
                self.update(Action::ToggleZen, fx);
                Ok(())
            }
            CommandInvocation::Spectrum => self.toggle_spectrum(fx),
            CommandInvocation::Mute => {
                self.update(Action::ToggleMute, fx);
                Ok(())
            }
            CommandInvocation::Seek(seconds) => {
                self.update(Action::SeekBy(seconds), fx);
                Ok(())
            }
            CommandInvocation::Volume(percent) => {
                self.update(Action::SetVolumeTo(f32::from(percent) / 100.0), fx);
                Ok(())
            }
            CommandInvocation::Mouse => {
                // Releasing capture hands the mouse back to the terminal:
                // native drag-selection and copy-on-select work again.
                self.mouse_captured = !self.mouse_captured;
                let toggled = if self.mouse_captured {
                    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
                } else {
                    crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)
                };
                toggled.map_err(|error| error.to_string())
            }
            CommandInvocation::Theme(theme) => self.set_command_theme(fx, &theme),
            CommandInvocation::Quality(quality) => self.set_command_quality(fx, quality),
            CommandInvocation::Goto(view) => {
                self.zen = false;
                if view == View::Login {
                    // A bare view switch would show an empty frame: only
                    // start_login spawns the QR flow.
                    self.update(Action::StartLogin, fx);
                } else {
                    self.update(Action::SwitchView(view), fx);
                }
                Ok(())
            }
            CommandInvocation::GotoRow(index) => {
                if index >= self.visible_len() {
                    Err(crate::i18n::t(Key::CommandRowOutOfRange).to_owned())
                } else {
                    self.update(Action::SelectIndex(index), fx);
                    Ok(())
                }
            }
            CommandInvocation::Settings => {
                self.zen = false;
                self.update(Action::SwitchView(View::Settings), fx);
                Ok(())
            }
            CommandInvocation::Quit => {
                self.update(Action::Quit, fx);
                Ok(())
            }
        };
        match result {
            Ok(()) => self.set_command_feedback(crate::i18n::t_command_executed(name), false),
            Err(message) => self.set_command_feedback(message, true),
        }
    }
}

fn command_error_message(error: CommandError) -> String {
    match error {
        CommandError::Unknown(command) => crate::i18n::t_command_unknown(&command),
        CommandError::MissingArgument { command, expected } => {
            crate::i18n::t_command_missing_argument(command, expected)
        }
        CommandError::UnexpectedArgument { command } => {
            crate::i18n::t_command_unexpected_argument(command)
        }
        CommandError::InvalidArgument {
            command,
            value,
            expected,
        } => crate::i18n::t_command_invalid_argument(command, &value, expected),
    }
}
