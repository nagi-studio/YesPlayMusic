use std::{
    collections::{HashMap, VecDeque},
    future::{poll_fn, Future},
    pin::pin,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    task::{Context, Poll, Waker},
    thread,
    time::Duration,
};

use mpris_server::{
    zbus::{self, proxy, zvariant::Value},
    LoopStatus, Metadata, PlaybackStatus, Player, Time, TrackId,
};
use serde::Deserialize;

const OSD_READY_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const OSD_READY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq)]
pub enum MediaControl {
    Quit,
    Next,
    Previous,
    Play,
    Pause,
    PlayPause,
    SeekBy(f64),
    SeekTo(f64),
    SetRepeat(RepeatMode),
    SetShuffle(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum RepeatMode {
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "one")]
    Track,
    #[serde(rename = "on")]
    Playlist,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaState {
    pub playing: bool,
    pub position_seconds: f64,
    pub repeat_mode: RepeatMode,
    pub shuffle: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaMetadata {
    pub track_id: String,
    pub title: String,
    pub album: String,
    pub artists: Vec<String>,
    pub artwork_url: Option<String>,
    pub media_url: Option<String>,
    pub length_seconds: f64,
    pub lyrics: Option<OsdLyrics>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OsdLyrics {
    pub title: String,
    pub artists: Vec<String>,
    pub content: String,
}

#[derive(Clone)]
pub struct LinuxMedia {
    queue: Arc<UpdateQueue>,
}

impl LinuxMedia {
    pub fn start<F>(control_handler: F) -> Result<Self, String>
    where
        F: Fn(MediaControl) + Send + Sync + 'static,
    {
        let queue = Arc::new(UpdateQueue::default());
        let thread_queue = Arc::clone(&queue);
        let control_handler = Arc::new(control_handler);

        thread::Builder::new()
            .name("yesplaymusic-linux-media".into())
            .spawn(move || {
                futures_lite::future::block_on(run_media_service(thread_queue, control_handler));
            })
            .map_err(|error| format!("failed to start Linux media thread: {error}"))?;

        Ok(Self { queue })
    }

    pub fn set_metadata(&self, metadata: MediaMetadata) {
        self.queue.push(MediaUpdate::Metadata(metadata));
    }

    pub fn set_playback(&self, state: PlaybackState) {
        self.queue.push(MediaUpdate::Playback(state));
    }

    pub fn set_position(&self, seconds: f64, emit_seeked: bool) {
        self.queue.push(MediaUpdate::Position {
            seconds,
            emit_seeked,
        });
    }

    pub fn set_repeat(&self, mode: RepeatMode) {
        self.queue.push(MediaUpdate::Repeat(mode));
    }

    pub fn set_shuffle(&self, enabled: bool) {
        self.queue.push(MediaUpdate::Shuffle(enabled));
    }

    pub fn update_state(&self, state: MediaState) {
        self.set_playback(if state.playing {
            PlaybackState::Playing
        } else {
            PlaybackState::Paused
        });
        self.set_position(state.position_seconds, false);
        self.set_repeat(state.repeat_mode);
        self.set_shuffle(state.shuffle);
    }

    pub fn shutdown(&self) {
        self.queue.push(MediaUpdate::Shutdown);
    }
}

#[derive(Debug)]
enum MediaUpdate {
    Metadata(MediaMetadata),
    LyricsDelivered {
        generation: u64,
        metadata: MediaMetadata,
    },
    Playback(PlaybackState),
    Position {
        seconds: f64,
        emit_seeked: bool,
    },
    Repeat(RepeatMode),
    Shuffle(bool),
    Shutdown,
}

struct OsdLyricsJob {
    generation: u64,
    metadata: MediaMetadata,
    lyrics: OsdLyrics,
}

#[derive(Default)]
struct UpdateQueue {
    values: Mutex<VecDeque<MediaUpdate>>,
    waker: Mutex<Option<Waker>>,
    closed: AtomicBool,
}

impl UpdateQueue {
    fn push(&self, update: MediaUpdate) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }

        self.values
            .lock()
            .expect("media queue poisoned")
            .push_back(update);
        if let Some(waker) = self.waker.lock().expect("media waker poisoned").take() {
            waker.wake();
        }
    }

    fn poll_next(&self, context: &mut Context<'_>) -> Poll<MediaUpdate> {
        let mut values = self.values.lock().expect("media queue poisoned");
        if let Some(update) = values.pop_front() {
            return Poll::Ready(update);
        }

        *self.waker.lock().expect("media waker poisoned") = Some(context.waker().clone());
        Poll::Pending
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.values.lock().expect("media queue poisoned").clear();
    }
}

fn spawn_osd_worker<F>(
    queue: Arc<UpdateQueue>,
    mut deliver: F,
) -> Result<mpsc::Sender<OsdLyricsJob>, String>
where
    F: FnMut(&OsdLyrics) + Send + 'static,
{
    let (sender, receiver) = mpsc::channel::<OsdLyricsJob>();
    thread::Builder::new()
        .name("yesplaymusic-osdlyrics".into())
        .spawn(move || {
            while let Ok(job) = receiver.recv() {
                deliver(&job.lyrics);
                queue.push(MediaUpdate::LyricsDelivered {
                    generation: job.generation,
                    metadata: job.metadata,
                });
            }
        })
        .map_err(|error| format!("failed to start OSDLyrics worker: {error}"))?;
    Ok(sender)
}

fn start_osd_worker(queue: Arc<UpdateQueue>) -> Result<mpsc::Sender<OsdLyricsJob>, String> {
    let mut osd_started = false;
    spawn_osd_worker(queue, move |lyrics| {
        if let Err(error) =
            futures_lite::future::block_on(deliver_osd_lyrics(lyrics, &mut osd_started))
        {
            eprintln!("OSDLyrics update failed: {error}");
        }
    })
}

async fn run_media_service(
    queue: Arc<UpdateQueue>,
    control_handler: Arc<dyn Fn(MediaControl) + Send + Sync>,
) {
    let player = match Player::builder("yesplaymusic")
        .identity("YesPlayMusic")
        .desktop_entry("YesPlayMusic")
        .can_quit(true)
        .can_go_next(true)
        .can_go_previous(true)
        .can_play(true)
        .can_pause(true)
        .can_seek(true)
        .build()
        .await
    {
        Ok(player) => player,
        Err(error) => {
            eprintln!("failed to register MPRIS service: {error}");
            queue.close();
            return;
        }
    };

    connect_controls(&player, control_handler.clone());

    let server = player.run();
    let mut server = pin!(server);
    let osd_sender = start_osd_worker(Arc::clone(&queue))
        .map_err(|error| eprintln!("{error}"))
        .ok();
    let mut metadata_generation = 0_u64;

    loop {
        let next = poll_fn(|context| {
            if server.as_mut().poll(context).is_ready() {
                return Poll::Ready(None);
            }
            queue.poll_next(context).map(Some)
        })
        .await;

        let Some(update) = next else {
            break;
        };
        if matches!(update, MediaUpdate::Shutdown) {
            break;
        }

        if let Err(error) = apply_update(
            &player,
            update,
            osd_sender.as_ref(),
            &mut metadata_generation,
        )
        .await
        {
            eprintln!("Linux media update failed: {error}");
        }
    }

    queue.close();
}

fn connect_controls(player: &Player, control_handler: Arc<dyn Fn(MediaControl) + Send + Sync>) {
    let handler = control_handler.clone();
    player.connect_quit(move |_| handler(MediaControl::Quit));

    let handler = control_handler.clone();
    player.connect_next(move |_| handler(MediaControl::Next));

    let handler = control_handler.clone();
    player.connect_previous(move |_| handler(MediaControl::Previous));

    let handler = control_handler.clone();
    player.connect_play(move |_| handler(MediaControl::Play));

    let handler = control_handler.clone();
    player.connect_pause(move |_| handler(MediaControl::Pause));

    let handler = control_handler.clone();
    player.connect_stop(move |_| handler(MediaControl::Pause));

    let handler = control_handler.clone();
    player.connect_play_pause(move |_| handler(MediaControl::PlayPause));

    let handler = control_handler.clone();
    player.connect_seek(move |_, offset| {
        handler(MediaControl::SeekBy(micros_to_seconds(offset.as_micros())));
    });

    let handler = control_handler.clone();
    player.connect_set_position(move |player, track_id, position| {
        if player.metadata().trackid().as_ref() == Some(track_id) {
            handler(MediaControl::SeekTo(micros_to_seconds(
                position.as_micros(),
            )));
        }
    });

    let handler = control_handler.clone();
    player.connect_set_loop_status(move |_, status| {
        handler(MediaControl::SetRepeat(from_loop_status(status)));
    });

    player.connect_set_shuffle(move |_, enabled| {
        control_handler(MediaControl::SetShuffle(enabled));
    });
}

async fn apply_update(
    player: &Player,
    update: MediaUpdate,
    osd_sender: Option<&mpsc::Sender<OsdLyricsJob>>,
    metadata_generation: &mut u64,
) -> Result<(), zbus::Error> {
    match update {
        MediaUpdate::Metadata(mut metadata) => {
            *metadata_generation = (*metadata_generation).wrapping_add(1);
            if let Some(lyrics) = metadata.lyrics.take() {
                if let Some(sender) = osd_sender {
                    let job = OsdLyricsJob {
                        generation: *metadata_generation,
                        metadata,
                        lyrics,
                    };
                    match sender.send(job) {
                        Ok(()) => return Ok(()),
                        Err(error) => metadata = error.0.metadata,
                    }
                }
            }
            publish_metadata(player, metadata).await?;
        }
        MediaUpdate::LyricsDelivered {
            generation,
            metadata,
        } => {
            if generation == *metadata_generation {
                publish_metadata(player, metadata).await?;
            }
        }
        MediaUpdate::Playback(state) => {
            player
                .set_playback_status(to_playback_status(state))
                .await?;
        }
        MediaUpdate::Position {
            seconds,
            emit_seeked,
        } => {
            let position = seconds_to_time(seconds);
            player.set_position(position);
            if emit_seeked {
                player.seeked(position).await?;
            }
        }
        MediaUpdate::Repeat(mode) => {
            player.set_loop_status(to_loop_status(mode)).await?;
        }
        MediaUpdate::Shuffle(enabled) => {
            player.set_shuffle(enabled).await?;
        }
        MediaUpdate::Shutdown => {}
    }

    Ok(())
}

async fn publish_metadata(player: &Player, metadata: MediaMetadata) -> Result<(), zbus::Error> {
    player.set_position(Time::ZERO);
    player.set_metadata(to_mpris_metadata(metadata)).await
}

fn to_mpris_metadata(metadata: MediaMetadata) -> Metadata {
    let mut builder = Metadata::builder()
        .title(metadata.title)
        .album(metadata.album)
        .artist(metadata.artists)
        .length(seconds_to_time(metadata.length_seconds));

    if let Ok(track_id) = TrackId::try_from(track_object_path(&metadata.track_id)) {
        builder = builder.trackid(track_id);
    }
    if let Some(artwork_url) = metadata.artwork_url {
        builder = builder.art_url(artwork_url);
    }
    if let Some(media_url) = metadata.media_url {
        builder = builder.url(media_url);
    }

    builder.build()
}

fn track_object_path(track_id: &str) -> String {
    let suffix: String = track_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if suffix.is_empty() {
        "/com/yesplaymusic/track/unknown".to_string()
    } else {
        format!("/com/yesplaymusic/track/{suffix}")
    }
}

fn seconds_to_time(seconds: f64) -> Time {
    if !seconds.is_finite() {
        return Time::ZERO;
    }

    let micros = (seconds.max(0.0) * 1_000_000.0).min(i64::MAX as f64);
    Time::from_micros(micros.round() as i64)
}

fn micros_to_seconds(micros: i64) -> f64 {
    micros as f64 / 1_000_000.0
}

fn to_playback_status(state: PlaybackState) -> PlaybackStatus {
    match state {
        PlaybackState::Playing => PlaybackStatus::Playing,
        PlaybackState::Paused => PlaybackStatus::Paused,
    }
}

fn to_loop_status(mode: RepeatMode) -> LoopStatus {
    match mode {
        RepeatMode::Off => LoopStatus::None,
        RepeatMode::Track => LoopStatus::Track,
        RepeatMode::Playlist => LoopStatus::Playlist,
    }
}

fn from_loop_status(status: LoopStatus) -> RepeatMode {
    match status {
        LoopStatus::None => RepeatMode::Off,
        LoopStatus::Track => RepeatMode::Track,
        LoopStatus::Playlist => RepeatMode::Playlist,
    }
}

#[proxy(
    interface = "org.osdlyrics.Lyrics",
    default_service = "org.osdlyrics.Daemon",
    default_path = "/org/osdlyrics/Lyrics"
)]
trait OsdLyricsDaemon {
    #[zbus(name = "SetLyricContent")]
    async fn set_lyric_content(
        &self,
        metadata: HashMap<&str, Value<'_>>,
        content: &[u8],
    ) -> zbus::Result<()>;
}

async fn deliver_osd_lyrics(lyrics: &OsdLyrics, osd_started: &mut bool) -> Result<(), zbus::Error> {
    let connection = zbus::Connection::session().await?;
    match call_osd_lyrics(&connection, lyrics).await {
        Ok(()) => Ok(()),
        Err(initial_error) if !*osd_started => {
            *osd_started = true;
            if Command::new("osdlyrics").spawn().is_err() {
                return Err(initial_error);
            }

            let attempt_count = retry_attempt_count(OSD_READY_TIMEOUT, OSD_READY_RETRY_INTERVAL);
            retry_with_delays(
                initial_error,
                || call_osd_lyrics(&connection, lyrics),
                std::iter::repeat_n(OSD_READY_RETRY_INTERVAL, attempt_count),
            )
            .await
        }
        Err(error) => Err(error),
    }
}

fn retry_attempt_count(timeout: Duration, interval: Duration) -> usize {
    if interval.is_zero() {
        return 0;
    }
    timeout.as_millis().div_ceil(interval.as_millis()) as usize
}

async fn retry_with_delays<T, E, F, Fut, I>(
    mut last_error: E,
    mut attempt: F,
    delays: I,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    I: IntoIterator<Item = Duration>,
{
    for delay in delays {
        thread::sleep(delay);
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

async fn call_osd_lyrics(
    connection: &zbus::Connection,
    lyrics: &OsdLyrics,
) -> Result<(), zbus::Error> {
    let proxy = OsdLyricsDaemonProxy::new(connection).await?;
    let artists = lyrics.artists.join(", ");
    let metadata = HashMap::from([
        ("title", Value::new(lyrics.title.as_str())),
        ("artist", Value::new(artists.as_str())),
    ]);
    proxy
        .set_lyric_content(metadata, lyrics.content.as_bytes())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata_fixture(track_id: &str) -> MediaMetadata {
        MediaMetadata {
            track_id: track_id.to_string(),
            title: "Title".to_string(),
            album: "Album".to_string(),
            artists: vec!["Artist".to_string()],
            artwork_url: None,
            media_url: None,
            length_seconds: 180.0,
            lyrics: None,
        }
    }

    fn lyrics_fixture() -> OsdLyrics {
        OsdLyrics {
            title: "Title".to_string(),
            artists: vec!["Artist".to_string()],
            content: "[00:00.00]Lyrics".to_string(),
        }
    }

    #[test]
    fn sanitizes_mpris_track_paths() {
        assert_eq!(
            track_object_path("123/a-b"),
            "/com/yesplaymusic/track/123_a_b"
        );
    }

    #[test]
    fn converts_repeat_modes_both_ways() {
        for mode in [RepeatMode::Off, RepeatMode::Track, RepeatMode::Playlist] {
            assert_eq!(from_loop_status(to_loop_status(mode)), mode);
        }
    }

    #[test]
    fn preserves_signed_seek_offsets() {
        assert_eq!(micros_to_seconds(-1_500_000), -1.5);
    }

    #[test]
    fn osd_retry_window_accepts_startup_slower_than_half_a_second() {
        let attempts = retry_attempt_count(OSD_READY_TIMEOUT, OSD_READY_RETRY_INTERVAL);
        assert_eq!(attempts, 50);
        assert!(OSD_READY_RETRY_INTERVAL * attempts as u32 >= Duration::from_millis(600));

        let mut calls = 0;
        let result = futures_lite::future::block_on(retry_with_delays(
            0,
            || {
                calls += 1;
                std::future::ready(if calls == 7 { Ok(calls) } else { Err(calls) })
            },
            std::iter::repeat_n(Duration::ZERO, attempts),
        ));
        assert_eq!(result, Ok(7));
    }

    #[test]
    fn osd_worker_does_not_block_the_media_update_queue() {
        let queue = Arc::new(UpdateQueue::default());
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let sender = spawn_osd_worker(Arc::clone(&queue), move |_| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
        .unwrap();

        sender
            .send(OsdLyricsJob {
                generation: 7,
                metadata: metadata_fixture("7"),
                lyrics: lyrics_fixture(),
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        queue.push(MediaUpdate::Playback(PlaybackState::Playing));

        let update = queue
            .values
            .lock()
            .expect("media queue poisoned")
            .pop_front();
        assert!(matches!(
            update,
            Some(MediaUpdate::Playback(PlaybackState::Playing))
        ));

        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            let delivered = queue
                .values
                .lock()
                .expect("media queue poisoned")
                .pop_front();
            if matches!(
                delivered,
                Some(MediaUpdate::LyricsDelivered { generation: 7, .. })
            ) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
    }
}
