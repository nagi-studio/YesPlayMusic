use std::collections::HashSet;
use std::sync::Arc;

use tempfile::TempDir;
use tokio::sync::mpsc;
use yesplaymusic_core::auth::{Session, SessionStore};

use super::*;
use crate::config::Config;
use crate::player;

fn effects(directory: &TempDir) -> Effects {
    let (player, _events) = player::spawn(tokio::runtime::Handle::current());
    let (actions, _receiver) = mpsc::unbounded_channel();
    Effects {
        player,
        ncm: Arc::new(Ncm::new(
            directory.path().join("session.json"),
            yesplaymusic_core::cache::AudioQuality::High320,
            true,
        )),
        store: Arc::new(crate::store::LibraryStore::new(
            directory.path().join("library"),
        )),
        actions,
        cache_root: None,
        covers: None,
        config_path: directory.path().join("config.toml"),
    }
}

fn candidate(name: &str) -> Session {
    Session {
        music_u: format!("{name}-music-u"),
        csrf: format!("{name}-csrf"),
    }
}

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

#[tokio::test]
async fn restored_track_uses_cached_likes_until_live_ids_arrive() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let cached = [row(7), row(9)]
        .iter()
        .map(crate::store::StoredSong::from)
        .collect::<Vec<_>>();
    fx.store.save(42, "liked", &cached).unwrap();

    let mut state = AppState::new(&Config::default());
    state.restore_playback(crate::store::StoredPlayback {
        queue: vec![crate::store::StoredSong::from(&row(7))],
        current: Some(crate::store::StoredSong::from(&row(7))),
        queue_pos: Some(0),
        position_ms: 12_000,
        volume: 0.8,
        volume_before_mute: None,
        play_mode: super::super::PlayMode::Off,
        shuffle: false,
        queue_source: Source::Liked,
    });
    assert_eq!(state.current_track_id, Some(7));
    assert!(state.liked.is_empty());

    let attempt = state.session.begin_login();
    state.apply_login_succeeded(&fx, attempt, candidate("listener"), 42, "listener".into());

    assert_eq!(state.liked, HashSet::from([7, 9]));
    assert!(state
        .current_track_id
        .is_some_and(|id| state.liked.contains(&id)));

    let stamp = state.session.current_stamp().unwrap();
    state.apply_liked_ids(stamp, HashSet::from([9]));
    assert_eq!(state.liked, HashSet::from([9]));
}

#[tokio::test]
async fn only_the_current_login_attempt_can_commit_its_candidate() {
    let directory = tempfile::tempdir().unwrap();
    let session_path = directory.path().join("session.json");
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    state.view = View::Login;
    let stale_attempt = state.session.begin_login();
    let current_attempt = state.session.begin_login();

    state.update(
        Action::LoginQrReady {
            attempt: stale_attempt,
            art: "stale QR".into(),
        },
        &fx,
    );
    assert!(state.session.login_qr.is_none());
    state.update(
        Action::LoginQrReady {
            attempt: current_attempt,
            art: "current QR".into(),
        },
        &fx,
    );
    assert_eq!(state.session.login_qr.as_deref(), Some("current QR"));

    state.update(
        Action::LoginSucceeded {
            attempt: stale_attempt,
            session: candidate("stale"),
            uid: 11,
            nickname: "stale".into(),
        },
        &fx,
    );

    assert!(fx.ncm.session_snapshot().is_none());
    assert!(SessionStore::new(&session_path).load().is_none());
    assert!(state.session.nickname.is_none());

    let current = candidate("current");
    state.update(
        Action::LoginSucceeded {
            attempt: current_attempt,
            session: current.clone(),
            uid: 22,
            nickname: "current".into(),
        },
        &fx,
    );

    assert_eq!(fx.ncm.session_snapshot(), Some(current.clone()));
    assert_eq!(SessionStore::new(session_path).load(), Some(current));
    assert_eq!(state.session.nickname.as_deref(), Some("current"));
    assert_eq!(
        state.session.current_stamp(),
        Some(SessionStamp { epoch: 1, uid: 22 })
    );
}

#[tokio::test]
async fn a_login_attempt_supersedes_an_in_flight_session_restore() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let restore_epoch = state.session.begin_restore();
    state.session.begin_login();

    state.update(
        Action::SessionRestored {
            epoch: restore_epoch,
            uid: 11,
            nickname: "restored account".into(),
        },
        &fx,
    );

    assert!(state.session.nickname.is_none());
    assert!(state.session.current_stamp().is_none());
}

#[tokio::test]
async fn offline_restore_reports_a_corrupted_profile_instead_of_adopting_a_snapshot_uid() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    fx.store
        .save(42, "liked", &[crate::store::StoredSong::from(&row(42))])
        .unwrap();
    std::fs::write(directory.path().join("library/profile.json"), b"{broken").unwrap();
    let mut state = AppState::new(&Config::default());
    let epoch = state.session.begin_restore();

    state.apply_session_restore_failed(&fx, epoch, RestoreFailure::Offline);

    assert!(state.session.current_stamp().is_none());
    assert!(state.session.nickname.is_none());
    assert!(state.library.iter().all(|song| song.id != 42));
    let status = state.status.as_deref().unwrap();
    assert!(status.contains("stored profile"));
    assert!(status.contains("invalid"));
}

#[tokio::test]
async fn personal_results_from_the_previous_account_are_ignored_and_not_saved_for_the_next() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let first_attempt = state.session.begin_login();
    let first = state
        .session
        .accept_login(first_attempt, 11, "first".into())
        .unwrap();
    let second_attempt = state.session.begin_login();
    assert!(state.session.current_stamp().is_none());
    state.update(
        Action::LibraryLoaded {
            session: first,
            request: 0,
            source: Source::Liked,
            rows: vec![row(11)],
        },
        &fx,
    );
    assert_ne!(state.library.first().map(|row| row.id), Some(11));
    let second = state
        .session
        .accept_login(second_attempt, 22, "second".into())
        .unwrap();
    state.library = vec![row(99)];
    state.liked = HashSet::from([99]);

    state.update(
        Action::LibraryLoaded {
            session: first,
            request: 0,
            source: Source::Liked,
            rows: vec![row(11)],
        },
        &fx,
    );
    state.update(
        Action::LikedIds {
            session: first,
            ids: HashSet::from([11]),
        },
        &fx,
    );

    assert_eq!(state.library[0].id, 99);
    assert_eq!(state.liked, HashSet::from([99]));
    assert!(fx.store.load(second.uid, "liked").is_none());

    state.update(
        Action::LibraryLoaded {
            session: second,
            request: 0,
            source: Source::Liked,
            rows: vec![row(22)],
        },
        &fx,
    );
    state.update(
        Action::LikedIds {
            session: second,
            ids: HashSet::from([22]),
        },
        &fx,
    );

    assert_eq!(state.library[0].id, 22);
    assert_eq!(state.liked, HashSet::from([22]));
    for _ in 0..50 {
        if fx.store.load(second.uid, "liked").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        fx.store.load(second.uid, "liked").unwrap()[0].id,
        second.uid
    );
}

#[tokio::test]
async fn an_older_visit_cannot_overwrite_the_latest_library_result_or_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let attempt = state.session.begin_login();
    let session = state
        .session
        .accept_login(attempt, 42, "listener".into())
        .unwrap();
    state.library_source = Source::Daily;
    let first = state.begin_library_request();
    let second = state.begin_library_request();

    state.update(
        Action::LibraryLoaded {
            session,
            request: second,
            source: Source::Daily,
            rows: vec![row(2)],
        },
        &fx,
    );
    state.update(
        Action::LibraryLoaded {
            session,
            request: first,
            source: Source::Daily,
            rows: vec![row(1)],
        },
        &fx,
    );
    state.update(
        Action::LibraryFailed {
            session,
            request: first,
            message: "stale failure".into(),
        },
        &fx,
    );

    assert_eq!(state.library[0].id, 2);
    assert_ne!(state.status.as_deref(), Some("stale failure"));
    for _ in 0..50 {
        if fx.store.load(session.uid, "daily").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(fx.store.load(session.uid, "daily").unwrap()[0].id, 2);
}

#[tokio::test]
async fn current_like_failure_rolls_back_only_the_requested_song() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let attempt = state.session.begin_login();
    let session = state
        .session
        .accept_login(attempt, 42, "listener".into())
        .unwrap();
    state.liked = HashSet::from([7, 9]);
    let like_mutation = state.begin_like_mutation(7, true);
    state.begin_like_request_for_test(7, like_mutation);

    state.update(
        Action::LikeFinished {
            session,
            id: 7,
            mutation: like_mutation,
            attempted_like: true,
            error: Some("like failed".into()),
        },
        &fx,
    );

    assert_eq!(state.liked, HashSet::from([9]));
    assert_eq!(state.status.as_deref(), Some("like failed"));

    let unlike_mutation = state.begin_like_mutation(7, false);
    state.begin_like_request_for_test(7, unlike_mutation);
    state.update(
        Action::LikeFinished {
            session,
            id: 7,
            mutation: unlike_mutation,
            attempted_like: false,
            error: Some("unlike failed".into()),
        },
        &fx,
    );

    assert_eq!(state.liked, HashSet::from([7, 9]));
    assert_eq!(state.status.as_deref(), Some("unlike failed"));
}

#[tokio::test]
async fn like_failure_cannot_undo_a_newer_choice_or_another_session() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let first_attempt = state.session.begin_login();
    let old_session = state
        .session
        .accept_login(first_attempt, 11, "first".into())
        .unwrap();
    let current_attempt = state.session.begin_login();
    let current_session = state
        .session
        .accept_login(current_attempt, 22, "second".into())
        .unwrap();
    state.liked.insert(7);
    state.status = Some("current state".into());
    let old_mutation = state.begin_like_mutation(7, true);
    state.begin_like_request_for_test(7, old_mutation);

    state.update(
        Action::LikeFinished {
            session: old_session,
            id: 7,
            mutation: old_mutation,
            attempted_like: true,
            error: Some("old account failure".into()),
        },
        &fx,
    );
    assert!(state.liked.contains(&7));

    let superseding_mutation = state.begin_like_mutation(7, false);
    state.update(
        Action::LikeFinished {
            session: current_session,
            id: 7,
            mutation: old_mutation,
            attempted_like: true,
            error: Some("superseded failure".into()),
        },
        &fx,
    );

    assert!(!state.liked.contains(&7));
    assert_eq!(state.status.as_deref(), Some("current state"));
    assert_ne!(superseding_mutation, old_mutation);
}

#[tokio::test]
async fn only_the_latest_of_three_interleaved_like_mutations_can_roll_back() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let attempt = state.session.begin_login();
    let session = state
        .session
        .accept_login(attempt, 42, "listener".into())
        .unwrap();
    state.status = Some("latest choice".into());

    let first_like = state.begin_like_mutation(7, true);
    let unlike = state.begin_like_mutation(7, false);
    let latest_like = state.begin_like_mutation(7, true);
    state.begin_like_request_for_test(7, first_like);
    assert!(state.liked.contains(&7));

    state.update(
        Action::LikeFinished {
            session,
            id: 7,
            mutation: unlike,
            attempted_like: false,
            error: Some("out-of-order failure".into()),
        },
        &fx,
    );
    assert_eq!(state.like_in_flight.get(&7), Some(&first_like));

    fx.ncm.commit_session(&candidate("listener")).unwrap();
    state.update(
        Action::LikeFinished {
            session,
            id: 7,
            mutation: first_like,
            attempted_like: true,
            error: None,
        },
        &fx,
    );

    assert!(state.liked.contains(&7));
    assert_eq!(state.status.as_deref(), Some("latest choice"));
    assert_eq!(state.like_in_flight.get(&7), Some(&latest_like));

    state.update(
        Action::LikeFinished {
            session,
            id: 7,
            mutation: latest_like,
            attempted_like: true,
            error: Some("latest failure".into()),
        },
        &fx,
    );

    assert!(!state.liked.contains(&7));
    assert_eq!(state.status.as_deref(), Some("latest failure"));
    assert!(!state.like_in_flight.contains_key(&7));
}

#[tokio::test]
async fn late_liked_snapshot_preserves_locally_mutated_songs() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let attempt = state.session.begin_login();
    let session = state
        .session
        .accept_login(attempt, 42, "listener".into())
        .unwrap();
    state.begin_like_mutation(7, true);

    state.update(
        Action::LikedIds {
            session,
            ids: HashSet::from([9]),
        },
        &fx,
    );

    assert_eq!(state.liked, HashSet::from([7, 9]));
}

#[tokio::test]
async fn a_new_session_does_not_inherit_like_mutations() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let first_attempt = state.session.begin_login();
    state
        .session
        .accept_login(first_attempt, 11, "first".into())
        .unwrap();
    state.begin_like_mutation(7, true);

    let second_attempt = state.session.begin_login();
    let second = state
        .session
        .accept_login(second_attempt, 22, "second".into())
        .unwrap();
    state.finish_account(&fx, second, "second".into(), candidate("second"));
    state.update(
        Action::LikedIds {
            session: second,
            ids: HashSet::new(),
        },
        &fx,
    );

    assert!(!state.liked.contains(&7));
    assert!(state.like_mutations.is_empty());
}

#[tokio::test]
async fn empty_or_failed_fm_page_clears_pending_request_and_allows_retry() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let attempt = state.session.begin_login();
    let session = state
        .session
        .accept_login(attempt, 42, "listener".into())
        .unwrap();
    state.queue_source = Source::Fm;
    state.queue = vec![row(1)];
    state.queue_pos = Some(0);
    state.pending_fm_next = true;
    state.fm_request_pending = true;

    state.update(
        Action::FmMore {
            session,
            rows: Vec::new(),
        },
        &fx,
    );

    assert!(!state.pending_fm_next);
    assert!(!state.fm_request_pending);
    assert_eq!(state.queue.len(), 1);

    state.pending_fm_next = true;
    state.fm_request_pending = true;
    state.update(
        Action::FmLoadFailed {
            session,
            message: "fm failed".into(),
        },
        &fx,
    );

    assert!(!state.pending_fm_next);
    assert!(!state.fm_request_pending);
    assert_eq!(state.status.as_deref(), Some("fm failed"));
}

#[tokio::test]
async fn repeated_fm_next_while_loading_keeps_one_request_pending() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let attempt = state.session.begin_login();
    state
        .session
        .accept_login(attempt, 42, "listener".into())
        .unwrap();
    fx.ncm.commit_session(&candidate("listener")).unwrap();
    state.queue_source = Source::Fm;
    state.queue = vec![row(1)];
    state.queue_pos = Some(0);

    state.update(Action::NextTrack, &fx);
    state.update(Action::NextTrack, &fx);

    assert!(state.pending_fm_next);
    assert!(state.fm_request_pending);
}

#[tokio::test]
async fn browsing_another_library_does_not_cancel_pending_fm_playback() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let attempt = state.session.begin_login();
    let session = state
        .session
        .accept_login(attempt, 42, "listener".into())
        .unwrap();
    state.queue_source = Source::Fm;
    state.queue = vec![row(1)];
    state.queue_pos = Some(0);
    state.pending_fm_next = true;
    state.fm_request_pending = true;

    state.open_source(&fx, 0);
    state.update(
        Action::FmMore {
            session,
            rows: vec![row(2)],
        },
        &fx,
    );

    assert_eq!(state.queue_pos, Some(1));
    assert_eq!(state.queue[1].id, 2);
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Track 2")
    );
    assert!(!state.pending_fm_next);
    assert!(!state.fm_request_pending);
}

#[tokio::test]
async fn late_fm_page_cannot_advance_a_replaced_queue() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let attempt = state.session.begin_login();
    let session = state
        .session
        .accept_login(attempt, 42, "listener".into())
        .unwrap();
    state.queue_source = Source::Fm;
    state.queue = vec![row(1)];
    state.queue_pos = Some(0);
    state.pending_fm_next = true;
    state.fm_request_pending = true;
    state.view = View::Search;
    state.search.input = false;
    state.search.songs.items = vec![row(9)];

    state.update(Action::Activate, &fx);
    let generation = state.generation;
    state.update(
        Action::FmMore {
            session,
            rows: vec![row(2)],
        },
        &fx,
    );

    assert_eq!(state.queue_pos, Some(0));
    assert_eq!(state.queue[0].id, 9);
    assert_eq!(state.generation, generation);
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Track 9")
    );
    assert!(!state.pending_fm_next);
    assert!(!state.fm_request_pending);
}

/// Sign in and make the session cookie visible to the API façade, which is
/// what `personal_request` needs before it will spawn anything.
fn signed_in(state: &mut AppState, fx: &Effects) -> SessionStamp {
    let attempt = state.session.begin_login();
    let stamp = state
        .session
        .accept_login(attempt, 42, "listener".into())
        .unwrap();
    fx.ncm.commit_session(&candidate("listener")).unwrap();
    stamp
}

#[tokio::test]
async fn personal_fm_without_a_session_asks_for_a_sign_in_instead_of_failing() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());

    state.update(Action::StartPersonalFm, &fx);

    assert_eq!(
        state.status.as_deref(),
        Some(i18n::t(Key::FmSignInRequired))
    );
    assert_ne!(state.queue_source, Source::Fm);
    assert!(!state.pending_fm_next);
    assert!(!state.fm_request_pending);

    // Trashing is equally explicit rather than silent.
    state.queue_source = Source::Fm;
    state.queue = vec![row(1)];
    state.queue_pos = Some(0);
    state.current_track_id = Some(1);
    state.update(Action::TrashFmTrack, &fx);

    assert_eq!(
        state.status.as_deref(),
        Some(i18n::t(Key::FmSignInRequired))
    );
    assert_eq!(state.queue_pos, Some(0));
}

#[tokio::test]
async fn entering_personal_fm_plays_the_head_of_its_first_batch() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let session = signed_in(&mut state, &fx);
    state.queue = vec![row(90)];
    state.queue_pos = Some(0);

    state.update(Action::StartPersonalFm, &fx);

    assert_eq!(state.queue_source, Source::Fm);
    assert!(state.queue.is_empty(), "the old queue is dropped");
    assert_eq!(state.queue_pos, None);
    assert!(state.pending_fm_next);
    assert!(state.fm_request_pending);
    assert_eq!(state.view, View::NowPlaying);

    state.update(
        Action::FmMore {
            session,
            rows: vec![row(1), row(2)],
        },
        &fx,
    );

    assert_eq!(state.queue_pos, Some(0));
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Track 1")
    );
    assert!(!state.pending_fm_next);
}

#[tokio::test]
async fn the_next_fm_batch_is_pulled_when_the_last_queued_track_starts() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    signed_in(&mut state, &fx);
    state.queue_source = Source::Fm;
    state.queue = vec![row(1), row(2), row(3)];
    state.queue_pos = Some(0);

    // Two tracks still queued behind this one: nothing to prefetch yet.
    state.update(Action::NextTrack, &fx);
    assert_eq!(state.queue_pos, Some(1));
    assert!(!state.fm_request_pending);
    assert!(!state.pending_fm_next);

    // Starting the last one refills ahead of the gap.
    state.update(Action::NextTrack, &fx);
    assert_eq!(state.queue_pos, Some(2));
    assert!(state.fm_request_pending);
    assert!(
        !state.pending_fm_next,
        "the refill is a prefetch, not a queued skip"
    );
}

#[tokio::test]
async fn trashing_the_current_fm_track_skips_on_without_waiting_for_the_server() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let session = signed_in(&mut state, &fx);
    state.queue_source = Source::Fm;
    state.queue = vec![row(1), row(2), row(3)];
    state.queue_pos = Some(1);
    state.current_track_id = Some(2);

    state.update(Action::TrashFmTrack, &fx);

    assert_eq!(state.current_track_id, Some(3));
    assert_eq!(state.status.as_deref(), Some(i18n::t(Key::FmTrashed)));
    // The banned row is gone and the cursor followed the shift.
    assert_eq!(
        state.queue.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert_eq!(state.queue_pos, Some(1));

    // A failure that arrives afterwards surfaces; a success stays quiet.
    state.update(
        Action::FmTrashFinished {
            session,
            message: None,
        },
        &fx,
    );
    assert_eq!(state.status.as_deref(), Some(i18n::t(Key::FmTrashed)));
    state.update(
        Action::FmTrashFinished {
            session,
            message: Some("fm trash failed".into()),
        },
        &fx,
    );
    assert_eq!(state.status.as_deref(), Some("fm trash failed"));
}

#[tokio::test]
async fn a_trashed_track_cannot_be_reached_again_by_stepping_back() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    signed_in(&mut state, &fx);
    state.queue_source = Source::Fm;
    state.queue = vec![row(1), row(2), row(3)];
    state.queue_pos = Some(1);
    state.current_track_id = Some(2);

    state.update(Action::TrashFmTrack, &fx);
    assert_eq!(state.current_track_id, Some(3));

    // "Never play again" has to survive the one gesture that would undo a skip.
    state.update(Action::PrevTrack, &fx);

    assert_eq!(state.current_track_id, Some(1));
    assert_eq!(state.queue_pos, Some(0));
    assert!(!state.queue.iter().any(|row| row.id == 2));
    assert_eq!(
        state.now.as_ref().map(|now| now.title.as_str()),
        Some("Track 1")
    );

    // And it stays gone when playback walks forward over the gap again.
    state.update(Action::NextTrack, &fx);
    assert_eq!(state.current_track_id, Some(3));
}

#[tokio::test]
async fn shuffled_trashing_rebuilds_an_order_that_cannot_reach_the_banned_row() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    signed_in(&mut state, &fx);
    state.queue_source = Source::Fm;
    state.queue = (1..=5).map(row).collect();
    state.queue_pos = Some(2);
    state.current_track_id = Some(3);
    state.shuffle = true;

    state.update(Action::TrashFmTrack, &fx);

    assert!(!state.queue.iter().any(|row| row.id == 3));
    // The order indexed the old queue; every entry must still be addressable.
    assert_eq!(state.shuffle_order.len(), state.queue.len());
    assert!(state
        .shuffle_order
        .iter()
        .all(|index| *index < state.queue.len()));
    assert_eq!(
        state
            .queue_pos
            .and_then(|position| state.queue.get(position)),
        state.active_row.as_ref(),
        "the cursor still names the row that is playing"
    );

    // Walking the whole shuffled queue never lands on the banned track.
    for _ in 0..state.queue.len() * 2 {
        state.update(Action::NextTrack, &fx);
        assert_ne!(state.current_track_id, Some(3));
    }
}

#[tokio::test]
async fn trashing_the_queue_tail_hands_the_slot_to_the_batch_still_in_flight() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let session = signed_in(&mut state, &fx);
    state.queue_source = Source::Fm;
    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(1);
    state.current_track_id = Some(2);

    state.update(Action::TrashFmTrack, &fx);

    // Nothing left to advance to, so the refill is queued instead.
    assert_eq!(
        state.queue.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(state.queue_pos, Some(0));
    assert!(state.pending_fm_next);
    assert!(state.fm_request_pending);

    state.update(
        Action::FmMore {
            session,
            rows: vec![row(8), row(9)],
        },
        &fx,
    );

    // The batch head takes the trashed row's slot rather than being skipped.
    assert_eq!(state.queue_pos, Some(1));
    assert_eq!(state.current_track_id, Some(8));
    assert!(!state.pending_fm_next);
    assert!(!state.queue.iter().any(|row| row.id == 2));
}

#[tokio::test]
async fn trashing_the_only_queued_track_restarts_the_station_from_empty() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let session = signed_in(&mut state, &fx);
    state.queue_source = Source::Fm;
    state.queue = vec![row(1)];
    state.queue_pos = Some(0);
    state.current_track_id = Some(1);

    state.update(Action::TrashFmTrack, &fx);

    assert!(state.queue.is_empty());
    assert_eq!(state.queue_pos, None, "an empty queue has no cursor");
    assert!(state.pending_fm_next);

    state.update(
        Action::FmMore {
            session,
            rows: vec![row(8), row(9)],
        },
        &fx,
    );

    assert_eq!(state.queue_pos, Some(0));
    assert_eq!(state.current_track_id, Some(8));
    assert!(!state.pending_fm_next);
}

#[tokio::test]
async fn trashing_outside_fm_mode_explains_itself_and_keeps_playing() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    signed_in(&mut state, &fx);
    state.queue_source = Source::Liked;
    state.queue = vec![row(1), row(2)];
    state.queue_pos = Some(0);
    state.current_track_id = Some(1);

    state.update(Action::TrashFmTrack, &fx);

    assert_eq!(state.status.as_deref(), Some(i18n::t(Key::FmOnlyInFm)));
    assert_eq!(state.queue_pos, Some(0));
    assert_eq!(state.current_track_id, Some(1));
}

#[tokio::test]
async fn fm_replies_from_a_replaced_session_are_dropped_by_the_epoch_guard() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let stale = signed_in(&mut state, &fx);
    state.queue_source = Source::Fm;
    state.queue = vec![row(1)];
    state.queue_pos = Some(0);
    state.pending_fm_next = true;
    state.fm_request_pending = true;
    // Someone else signs in while the batch is still in flight.
    let current = signed_in(&mut state, &fx);
    assert_ne!(stale, current);

    state.update(
        Action::FmMore {
            session: stale,
            rows: vec![row(2)],
        },
        &fx,
    );

    assert_eq!(state.queue.len(), 1, "a late batch cannot extend the queue");
    assert_eq!(state.queue_pos, Some(0));
    assert!(state.pending_fm_next);
    assert!(state.fm_request_pending);

    state.status = None;
    state.update(
        Action::FmLoadFailed {
            session: stale,
            message: "stale failure".into(),
        },
        &fx,
    );
    state.update(
        Action::FmTrashFinished {
            session: stale,
            message: Some("stale trash failure".into()),
        },
        &fx,
    );
    assert!(state.status.is_none(), "late errors stay off screen");
    assert!(state.pending_fm_next);

    // The live stamp is still accepted.
    state.update(
        Action::FmMore {
            session: current,
            rows: vec![row(3)],
        },
        &fx,
    );
    assert_eq!(state.queue_pos, Some(1));
    assert_eq!(state.queue[1].id, 3);
}

#[tokio::test]
async fn playing_a_library_track_leaves_fm_mode_and_its_auto_refill_behind() {
    let directory = tempfile::tempdir().unwrap();
    let fx = effects(&directory);
    let mut state = AppState::new(&Config::default());
    let session = signed_in(&mut state, &fx);
    state.queue_source = Source::Fm;
    state.queue = vec![row(1)];
    state.queue_pos = Some(0);
    state.pending_fm_next = true;
    state.fm_request_pending = true;
    state.view = View::Library;
    state.library_source = Source::Liked;
    state.library = vec![row(9)];
    state.selected = 0;

    state.update(Action::Activate, &fx);

    assert_eq!(state.queue_source, Source::Liked);
    assert!(!state.pending_fm_next);
    assert!(!state.fm_request_pending);

    // Reaching the end of this queue must not pull another FM batch.
    state.update(Action::NextTrack, &fx);
    assert!(!state.pending_fm_next);
    assert!(!state.fm_request_pending);

    // Nor may the batch that was already in flight hijack playback.
    state.update(
        Action::FmMore {
            session,
            rows: vec![row(2)],
        },
        &fx,
    );
    assert_eq!(state.queue.len(), 1);
    assert_eq!(state.queue[0].id, 9);
    assert!(!state.pending_fm_next);
}
