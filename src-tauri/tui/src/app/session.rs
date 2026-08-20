use std::time::Duration;

use yesplaymusic_core::auth::Session;

use crate::action::{Action, RestoreFailure, SessionStamp, View};
use crate::api::{AccountError, Ncm, QrStatus, SongRow, Source};
use crate::i18n::{self, Key};

use super::{AppState, Effects};

#[derive(Default)]
pub struct SessionState {
    pub nickname: Option<String>,
    pub login_qr: Option<String>,
    pub login_message: Option<String>,
    uid: Option<i64>,
    session_epoch: u64,
    login_attempt: u64,
    active_login_attempt: Option<u64>,
    restoring_epoch: Option<u64>,
}

impl SessionState {
    pub(super) fn begin_restore(&mut self) -> u64 {
        self.session_epoch += 1;
        self.restoring_epoch = Some(self.session_epoch);
        self.session_epoch
    }

    fn accept_restore(&mut self, epoch: u64, uid: i64, nickname: String) -> Option<SessionStamp> {
        if self.restoring_epoch != Some(epoch) || self.active_login_attempt.is_some() {
            return None;
        }
        self.restoring_epoch = None;
        self.uid = Some(uid);
        self.nickname = Some(nickname);
        Some(SessionStamp { epoch, uid })
    }

    fn fail_restore(&mut self, epoch: u64) -> bool {
        if self.restoring_epoch != Some(epoch) || self.active_login_attempt.is_some() {
            return false;
        }
        self.restoring_epoch = None;
        true
    }

    fn begin_login(&mut self) -> u64 {
        self.login_attempt += 1;
        self.active_login_attempt = Some(self.login_attempt);
        self.restoring_epoch = None;
        if self.uid.is_some() {
            self.session_epoch += 1;
        }
        self.uid = None;
        self.nickname = None;
        self.login_qr = None;
        self.login_message = Some(i18n::t(Key::FetchingQr).into());
        self.login_attempt
    }

    fn accepts_login(&self, attempt: u64) -> bool {
        self.active_login_attempt == Some(attempt)
    }

    fn accept_login(&mut self, attempt: u64, uid: i64, nickname: String) -> Option<SessionStamp> {
        if !self.accepts_login(attempt) {
            return None;
        }
        self.active_login_attempt = None;
        self.session_epoch += 1;
        self.uid = Some(uid);
        self.nickname = Some(nickname);
        Some(SessionStamp {
            epoch: self.session_epoch,
            uid,
        })
    }

    fn current_stamp(&self) -> Option<SessionStamp> {
        self.active_login_attempt.is_none().then_some(())?;
        self.uid.map(|uid| SessionStamp {
            epoch: self.session_epoch,
            uid,
        })
    }

    fn matches(&self, stamp: SessionStamp) -> bool {
        self.current_stamp() == Some(stamp)
    }
}

impl AppState {
    pub(super) fn begin_session_restore(&mut self, fx: &Effects, session: Session) {
        let epoch = self.session.begin_restore();
        spawn_restore(fx, epoch, session);
    }

    pub(super) fn start_login(&mut self, fx: &Effects) {
        let attempt = self.session.begin_login();
        self.view = View::Login;
        spawn_login(fx, attempt);
    }

    pub(super) fn apply_login_qr(&mut self, attempt: u64, art: String) {
        if self.session.accepts_login(attempt) && self.view == View::Login {
            self.session.login_qr = Some(art);
            self.session.login_message = Some(i18n::t(Key::ScanQr).into());
        }
    }

    pub(super) fn apply_login_progress(&mut self, attempt: u64, message: String) {
        if self.session.accepts_login(attempt) && self.view == View::Login {
            self.session.login_message = Some(message);
        }
    }

    pub(super) fn apply_login_failed(&mut self, attempt: u64, message: String) {
        if self.session.accepts_login(attempt) {
            self.session.active_login_attempt = None;
            if self.view == View::Login {
                self.session.login_message = Some(message);
            }
        }
    }

    pub(super) fn apply_login_succeeded(
        &mut self,
        fx: &Effects,
        attempt: u64,
        session: Session,
        uid: i64,
        nickname: String,
    ) {
        if !self.session.accepts_login(attempt) {
            return;
        }
        if let Err(error) = fx.ncm.commit_session(&session) {
            self.apply_login_failed(attempt, error.to_string());
            return;
        }
        let Some(stamp) = self.session.accept_login(attempt, uid, nickname.clone()) else {
            return;
        };
        self.finish_account(fx, stamp, nickname, session);
    }

    pub(super) fn apply_session_restored(
        &mut self,
        fx: &Effects,
        epoch: u64,
        uid: i64,
        nickname: String,
    ) {
        let Some(stamp) = self.session.accept_restore(epoch, uid, nickname.clone()) else {
            return;
        };
        let Some(session) = fx.ncm.session_snapshot() else {
            return;
        };
        self.finish_account(fx, stamp, nickname, session);
    }

    pub(super) fn apply_session_restore_failed(
        &mut self,
        fx: &Effects,
        epoch: u64,
        failure: RestoreFailure,
    ) {
        match failure {
            RestoreFailure::Expired => {
                if self.session.fail_restore(epoch) {
                    self.status = Some(i18n::t(Key::SessionExpired).into());
                }
            }
            RestoreFailure::Offline => match fx.store.load_profile() {
                Ok(Some(profile)) => {
                    let Some(stamp) =
                        self.session
                            .accept_restore(epoch, profile.uid, profile.nickname)
                    else {
                        return;
                    };
                    self.enter_library(fx, stamp);
                    // Nothing is being fetched: without this the empty-library
                    // hint would claim 「正在同步」 forever while offline.
                    self.library_synced = true;
                    self.status = Some(i18n::t(Key::OfflineLibrary).into());
                }
                Ok(None) => {
                    if self.session.fail_restore(epoch) {
                        self.status = Some(i18n::t(Key::NetworkUnavailable).into());
                    }
                }
                Err(error) => {
                    if self.session.fail_restore(epoch) {
                        self.status = Some(error.to_string());
                    }
                }
            },
        }
    }

    fn finish_account(
        &mut self,
        fx: &Effects,
        stamp: SessionStamp,
        nickname: String,
        session: Session,
    ) {
        self.status = Some(i18n::t_welcome(&nickname));
        self.enter_library(fx, stamp);
        let request = self.begin_library_request();
        spawn_fetch_library(fx, stamp, request, session);
        spawn_save_profile(fx, stamp.uid, nickname);
    }

    /// Adopt an account identity: reset per-account state and show the
    /// on-disk liked snapshot. Callers decide whether a fetch follows.
    fn enter_library(&mut self, fx: &Effects, stamp: SessionStamp) {
        if self.view == View::Login {
            self.view = View::Library;
        }
        self.session.login_qr = None;
        self.selected = 0;
        self.filter.clear();
        self.sidebar_selected = 0;
        self.library_source = Source::Liked;
        self.library_synced = false;
        self.pending_fm_next = false;
        self.fm_request_pending = false;
        self.like_mutations.clear();
        self.like_in_flight.clear();
        self.liked.clear();
        self.library = fx
            .store
            .load(stamp.uid, "liked")
            .map(|rows| rows.into_iter().map(|row| row.into_song_row()).collect())
            .unwrap_or_default();
        self.liked = self.library.iter().map(|row| row.id).collect();
    }

    fn personal_request(&self, fx: &Effects) -> Option<(SessionStamp, Session)> {
        self.session.current_stamp().zip(fx.ncm.session_snapshot())
    }

    pub fn source_index(&self) -> usize {
        match self.library_source {
            Source::Liked => 0,
            Source::Daily => 1,
            Source::Fm => 2,
            Source::Cloud | Source::Search => 3,
        }
    }

    pub(super) fn open_source(&mut self, fx: &Effects, index: usize) {
        self.clear_filter();
        let source = match index {
            1 => Source::Daily,
            2 => Source::Fm,
            3 => Source::Cloud,
            _ => Source::Liked,
        };
        self.library_source = source;
        self.sidebar_selected = index;
        self.sidebar_focus = false;
        self.selected = 0;
        self.library_synced = false;
        self.library = self
            .session
            .current_stamp()
            .zip(cache_name(source))
            .and_then(|(stamp, name)| fx.store.load(stamp.uid, name))
            .map(|rows| rows.into_iter().map(|row| row.into_song_row()).collect())
            .unwrap_or_default();
        if let Some((stamp, session)) = self.personal_request(fx) {
            let request = self.begin_library_request();
            spawn_fetch_source(fx, stamp, request, session, source);
        }
    }

    fn begin_library_request(&mut self) -> u64 {
        self.library_request += 1;
        self.library_request
    }

    pub(super) fn apply_library_loaded(
        &mut self,
        fx: &Effects,
        session: SessionStamp,
        request: u64,
        source: Source,
        rows: Vec<SongRow>,
    ) {
        if !self.session.matches(session) || request != self.library_request {
            return;
        }
        if let Some(name) = cache_name(source) {
            spawn_save_snapshot(fx, session.uid, name, rows.clone());
        }
        if source == self.library_source {
            if source == Source::Liked {
                self.status = Some(i18n::t_liked_songs_count(rows.len()));
            }
            self.library = rows;
            self.selected = 0;
            self.library_synced = true;
        }
    }

    pub(super) fn apply_library_failed(
        &mut self,
        session: SessionStamp,
        request: u64,
        message: String,
    ) {
        if self.session.matches(session) && request == self.library_request {
            self.status = Some(message);
        }
    }

    pub(super) fn apply_liked_ids(
        &mut self,
        session: SessionStamp,
        ids: std::collections::HashSet<i64>,
    ) {
        if !self.session.matches(session) {
            return;
        }
        let touched = self
            .like_mutations
            .keys()
            .map(|id| (*id, self.liked.contains(id)))
            .collect::<Vec<_>>();
        self.liked = ids;
        for (id, liked) in touched {
            if liked {
                self.liked.insert(id);
            } else {
                self.liked.remove(&id);
            }
        }
    }

    pub(super) fn apply_fm_more(
        &mut self,
        fx: &Effects,
        session: SessionStamp,
        rows: Vec<SongRow>,
    ) {
        if !self.session.matches(session) {
            return;
        }
        self.fm_request_pending = false;
        if rows.is_empty() {
            self.pending_fm_next = false;
            self.status = Some(i18n::t(Key::QueueFinished).into());
            return;
        }
        if self.queue_source == Source::Fm {
            self.queue.extend(rows.iter().cloned());
        }
        if self.library_source == Source::Fm {
            self.library.extend(rows.iter().cloned());
            self.library_synced = true;
        }
        if self.pending_fm_next && self.queue_source == Source::Fm {
            self.pending_fm_next = false;
            // The first batch of a fresh FM session has nothing to step from:
            // adopt its head instead of advancing past it.
            match self.queue_pos {
                Some(_) => {
                    self.step_queue(fx, 1, true, true);
                }
                None => self.start_fm_head(fx),
            }
        } else if self.queue_source != Source::Fm {
            self.pending_fm_next = false;
        }
    }

    fn start_fm_head(&mut self, fx: &Effects) {
        if let Some(row) = self.queue.first().cloned() {
            self.queue_pos = Some(0);
            self.reset_shuffle_order();
            self.play_row(fx, row);
        }
    }

    pub(super) fn fetch_fm_more(&mut self, fx: &Effects) {
        if self.fm_request_pending {
            return;
        }
        if let Some((stamp, session)) = self.personal_request(fx) {
            self.fm_request_pending = true;
            spawn_fm_more(fx, stamp, session);
        }
    }

    pub(super) fn apply_fm_load_failed(&mut self, session: SessionStamp, message: String) {
        if self.session.matches(session) {
            self.fm_request_pending = false;
            self.pending_fm_next = false;
            self.status = Some(message);
        }
    }

    /// Enter FM mode: drop the old queue, pull a batch, play its head.
    pub(super) fn start_personal_fm(&mut self, fx: &Effects) {
        if self.personal_request(fx).is_none() {
            self.status = Some(i18n::t(Key::FmSignInRequired).into());
            return;
        }
        self.dashboard_hold = false;
        self.queue.clear();
        self.queue_pos = None;
        self.queue_source = Source::Fm;
        self.reset_shuffle_order();
        self.prefetched = None;
        self.pending_fm_next = true;
        self.status = Some(i18n::t(Key::FmStarting).into());
        self.view = View::NowPlaying;
        self.fetch_fm_more(fx);
    }

    /// FM trash: tell the server, then move on without awaiting the answer.
    pub(super) fn trash_current_fm_track(&mut self, fx: &Effects) {
        if self.queue_source != Source::Fm {
            self.status = Some(i18n::t(Key::FmOnlyInFm).into());
            return;
        }
        let (Some(id), Some(trashed)) = (self.current_track_id, self.queue_pos) else {
            return;
        };
        let Some((stamp, session)) = self.personal_request(fx) else {
            self.status = Some(i18n::t(Key::FmSignInRequired).into());
            return;
        };
        spawn_fm_trash(fx, stamp, session, id);
        self.dashboard_hold = false;
        // Pick the successor first — shuffle decides it, not the index order.
        self.step_queue(fx, 1, false, false);
        self.drop_trashed_row(fx, trashed);
        // Set last: the skip itself overwrites status with "resolving".
        self.status = Some(i18n::t(Key::FmTrashed).into());
    }

    /// Evict the banned row so no back-step or reshuffle can reach it again.
    /// Called after the skip, so `queue_pos` already names the successor.
    fn drop_trashed_row(&mut self, fx: &Effects, trashed: usize) {
        if trashed >= self.queue.len() {
            return;
        }
        let stayed = self.queue_pos == Some(trashed);
        self.queue.remove(trashed);
        if stayed {
            // Nothing to advance to yet: the tail was trashed while its
            // refill is still in flight. Park the cursor on the row before it
            // so the batch that lands next becomes the successor — and the
            // trashed row, still audible until then, is already unreachable.
            self.queue_pos = trashed.checked_sub(1);
        } else if let Some(position) = self.queue_pos.filter(|position| *position > trashed) {
            self.queue_pos = Some(position - 1);
        }
        // Both orders indexed the pre-removal queue.
        self.prefetched = None;
        self.reset_shuffle_order();
        // Shuffle can land before the trashed row, so the removal — not the
        // skip — is what leaves the cursor on the tail. Re-arm the refill;
        // an in-flight one makes this a no-op.
        if self
            .queue_pos
            .is_some_and(|position| position + 1 >= self.queue.len())
        {
            self.fetch_fm_more(fx);
        }
    }

    pub(super) fn apply_fm_trash_finished(
        &mut self,
        session: SessionStamp,
        message: Option<String>,
    ) {
        if let (true, Some(message)) = (self.session.matches(session), message) {
            self.status = Some(message);
        }
    }

    pub(super) fn toggle_like(&mut self, fx: &Effects) {
        let (Some(id), Some((stamp, session))) = (self.current_track_id, self.personal_request(fx))
        else {
            return;
        };
        let like = !self.liked.contains(&id);
        let mutation = self.begin_like_mutation(id, like);
        self.status = Some(i18n::t(if like { Key::Liked } else { Key::Unliked }).to_owned());
        self.start_like_request(fx, stamp, session, id, mutation, like);
    }

    fn begin_like_mutation(&mut self, id: i64, like: bool) -> u64 {
        let mutation = self.like_mutations.entry(id).or_default();
        *mutation += 1;
        let mutation = *mutation;
        if like {
            self.liked.insert(id);
        } else {
            self.liked.remove(&id);
        }
        mutation
    }

    #[cfg(test)]
    fn begin_like_request_for_test(&mut self, id: i64, mutation: u64) {
        self.like_in_flight.insert(id, mutation);
    }

    fn start_like_request(
        &mut self,
        fx: &Effects,
        session: SessionStamp,
        auth: Session,
        id: i64,
        mutation: u64,
        like: bool,
    ) {
        if self.like_in_flight.contains_key(&id) {
            return;
        }
        self.like_in_flight.insert(id, mutation);
        spawn_toggle_like(fx, session, auth, id, mutation, like);
    }

    pub(super) fn apply_like_finished(
        &mut self,
        fx: &Effects,
        session: SessionStamp,
        id: i64,
        mutation: u64,
        attempted_like: bool,
        error: Option<String>,
    ) {
        if !self.session.matches(session) || self.like_in_flight.get(&id) != Some(&mutation) {
            return;
        }
        self.like_in_flight.remove(&id);
        let latest = self.like_mutations.get(&id).copied();
        if latest != Some(mutation) {
            let Some((stamp, auth)) = self.personal_request(fx) else {
                return;
            };
            let latest = latest.expect("like mutation exists while request is active");
            let like = self.liked.contains(&id);
            self.start_like_request(fx, stamp, auth, id, latest, like);
            return;
        }
        if let Some(message) = error {
            if attempted_like {
                self.liked.remove(&id);
            } else {
                self.liked.insert(id);
            }
            self.status = Some(message);
        }
    }
}

fn spawn_restore(fx: &Effects, epoch: u64, session: Session) {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let action = match ncm.account(Some(&session)).await {
            Ok((uid, nickname)) => Action::SessionRestored {
                epoch,
                uid,
                nickname,
            },
            Err(AccountError::Expired(_)) => Action::SessionRestoreFailed {
                epoch,
                failure: RestoreFailure::Expired,
            },
            Err(AccountError::Unreachable(error)) => {
                tracing::warn!(%error, "session restore unreachable, going offline");
                Action::SessionRestoreFailed {
                    epoch,
                    failure: RestoreFailure::Offline,
                }
            }
        };
        let _ = actions.send(action);
    });
}

fn spawn_save_profile(fx: &Effects, uid: i64, nickname: String) {
    let store = fx.store.clone();
    tokio::spawn(async move {
        let profile = crate::store::StoredProfile { uid, nickname };
        let result = tokio::task::spawn_blocking(move || store.save_profile(&profile)).await;
        if let Ok(Err(error)) = result {
            tracing::warn!(%error, "profile save failed");
        }
    });
}

fn spawn_login(fx: &Effects, attempt: u64) {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let key = match ncm.qr_key().await {
            Ok(key) => key,
            Err(error) => {
                let _ = actions.send(Action::LoginFailed {
                    attempt,
                    message: error.to_string(),
                });
                return;
            }
        };
        let art = match crate::api::qr_unicode(&Ncm::qr_login_url(&key)) {
            Ok(art) => art,
            Err(error) => {
                let _ = actions.send(Action::LoginFailed {
                    attempt,
                    message: error.to_string(),
                });
                return;
            }
        };
        if actions.send(Action::LoginQrReady { attempt, art }).is_err() {
            return;
        }
        let mut consecutive_errors = 0_u32;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match ncm.qr_check(&key).await {
                Ok(QrStatus::Waiting) => consecutive_errors = 0,
                Ok(QrStatus::Scanned) => {
                    consecutive_errors = 0;
                    if actions
                        .send(Action::LoginProgress {
                            attempt,
                            message: i18n::t(Key::QrScannedConfirm).into(),
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(QrStatus::Expired) => {
                    let _ = actions.send(Action::LoginFailed {
                        attempt,
                        message: i18n::t(Key::QrExpired).into(),
                    });
                    return;
                }
                Ok(QrStatus::Success(session)) => {
                    let action = match ncm.account(Some(&session)).await {
                        Ok((uid, nickname)) => Action::LoginSucceeded {
                            attempt,
                            session,
                            uid,
                            nickname,
                        },
                        Err(AccountError::Expired(error)) => Action::LoginFailed {
                            attempt,
                            message: error.to_string(),
                        },
                        Err(AccountError::Unreachable(error)) => {
                            // The QR grant is real even though the profile
                            // read dropped: persist the cookie so the next
                            // start restores instead of demanding a rescan.
                            if let Err(commit_error) = ncm.commit_session(&session) {
                                tracing::warn!(%commit_error, "session persist after QR failed");
                            }
                            tracing::warn!(%error, "account read after QR unreachable");
                            Action::LoginFailed {
                                attempt,
                                message: i18n::t(Key::NetworkUnavailable).into(),
                            }
                        }
                    };
                    let _ = actions.send(action);
                    return;
                }
                Err(error) => {
                    consecutive_errors += 1;
                    if consecutive_errors >= 3 {
                        let _ = actions.send(Action::LoginFailed {
                            attempt,
                            message: i18n::t_login_interrupted(error),
                        });
                        return;
                    }
                    if actions
                        .send(Action::LoginProgress {
                            attempt,
                            message: i18n::t(Key::NetworkRetrying).into(),
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    });
}

fn spawn_fetch_library(fx: &Effects, stamp: SessionStamp, request: u64, session: Session) {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    let liked_session = session.clone();
    tokio::spawn(async move {
        let action = match ncm.liked_songs(stamp.uid, Some(&liked_session)).await {
            Ok(rows) => Action::LibraryLoaded {
                session: stamp,
                request,
                source: Source::Liked,
                rows,
            },
            Err(error) => Action::LibraryFailed {
                session: stamp,
                request,
                message: i18n::t_library_load_failed(error),
            },
        };
        let _ = actions.send(action);
    });
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        match ncm.liked_ids(stamp.uid, Some(&session)).await {
            Ok(ids) => {
                let _ = actions.send(Action::LikedIds {
                    session: stamp,
                    ids,
                });
            }
            Err(_) => tracing::warn!("liked IDs load failed"),
        }
    });
}

fn spawn_fetch_source(
    fx: &Effects,
    stamp: SessionStamp,
    request: u64,
    session: Session,
    source: Source,
) {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let result = match source {
            Source::Liked => ncm.liked_songs(stamp.uid, Some(&session)).await,
            Source::Daily => ncm.daily_songs(Some(&session)).await,
            Source::Fm => ncm.personal_fm(Some(&session)).await,
            Source::Cloud => ncm.cloud_songs(Some(&session)).await,
            Source::Search => unreachable!("search has its own request path"),
        };
        let action = match result {
            Ok(rows) => Action::LibraryLoaded {
                session: stamp,
                request,
                source,
                rows,
            },
            Err(error) => Action::LibraryFailed {
                session: stamp,
                request,
                message: i18n::t_library_load_failed(error),
            },
        };
        let _ = actions.send(action);
    });
}

fn spawn_fm_more(fx: &Effects, stamp: SessionStamp, session: Session) {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let action = match ncm.personal_fm(Some(&session)).await {
            Ok(rows) => Action::FmMore {
                session: stamp,
                rows,
            },
            Err(error) => Action::FmLoadFailed {
                session: stamp,
                message: i18n::t_library_load_failed(error),
            },
        };
        let _ = actions.send(action);
    });
}

fn spawn_fm_trash(fx: &Effects, stamp: SessionStamp, session: Session, id: i64) {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let message = ncm
            .fm_trash(id, Some(&session))
            .await
            .err()
            .map(|error| error.to_string());
        let _ = actions.send(Action::FmTrashFinished {
            session: stamp,
            message,
        });
    });
}

fn spawn_toggle_like(
    fx: &Effects,
    stamp: SessionStamp,
    session: Session,
    id: i64,
    mutation: u64,
    like: bool,
) {
    let ncm = fx.ncm.clone();
    let actions = fx.actions.clone();
    tokio::spawn(async move {
        let error = ncm
            .set_like(id, like, Some(&session))
            .await
            .err()
            .map(|error| error.to_string());
        let _ = actions.send(Action::LikeFinished {
            session: stamp,
            id,
            mutation,
            attempted_like: like,
            error,
        });
    });
}

fn cache_name(source: Source) -> Option<&'static str> {
    match source {
        Source::Liked => Some("liked"),
        Source::Daily => Some("daily"),
        Source::Cloud => Some("cloud"),
        Source::Fm | Source::Search => None,
    }
}

fn spawn_save_snapshot(fx: &Effects, uid: i64, source: &'static str, rows: Vec<SongRow>) {
    let store = fx.store.clone();
    tokio::spawn(async move {
        let stored: Vec<crate::store::StoredSong> =
            rows.iter().map(crate::store::StoredSong::from).collect();
        let _ = tokio::task::spawn_blocking(move || store.save(uid, source, &stored)).await;
    });
}

#[cfg(test)]
mod tests;
