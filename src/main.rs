// Wreckers Radio desktop player
//
// Polls Wreckers Radio's AzuraCast "now playing" API for the current
// artist/title, and streams the live MP3 by shelling out to `mpv`
// (or ffplay/vlc — see `PLAYER_CMD` below). Shelling out to a real
// media player is the simple, robust way to handle a live radio
// stream: it deals with reconnects, variable bitrate, and buffering
// for us, instead of us hand-rolling a network MP3 decoder.
//
// Requires `mpv` to be installed and on PATH:
//   Linux:  sudo apt install mpv   (or your distro's equivalent)
//   macOS:  brew install mpv
//
// Build & run:
//   cargo run --release

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Element, Length, Subscription, Task, Theme};
use serde::Deserialize;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const NOWPLAYING_URL: &str = "https://az.wreckersradio.uk/api/nowplaying/wreckersradio";

/// Direct MP3 stream mount, from Wreckers Radio's own "listen on an app" page.
const STREAM_URL: &str = "https://az.wreckersradio.uk/listen/wreckersradio/radio.mp3";

/// The API response itself is cached for 15s server-side, so there's no
/// point polling more often than that.
const POLL_SECS: u64 = 15;

pub fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .theme(|_state: &App| Theme::TokyoNightLight)
        .window_size((420.0, 260.0))
        .run()
}

#[derive(Debug, Clone, Deserialize, Default)]
struct Song {
    #[serde(default)]
    artist: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    art: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct CurrentSong {
    #[serde(default)]
    song: Song,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct Listeners {
    #[serde(default)]
    current: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct NowPlayingResponse {
    #[serde(default)]
    now_playing: CurrentSong,
    #[serde(default)]
    listeners: Listeners,
}

#[derive(Debug, Clone, PartialEq)]
enum Playback {
    Stopped,
    Playing,
}

struct App {
    artist: String,
    title: String,
    listeners: Option<u64>,
    status: Option<String>,
    playback: Playback,
    // The running `mpv` child process, if any. Kept out of `Message` since
    // `Child` isn't `Clone` — we spawn/kill it synchronously in `update`.
    player: Option<Child>,
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    NowPlayingFetched(Result<NowPlayingResponse, String>),
    PlayPressed,
    StopPressed,
}



impl App {
    fn new() -> (Self, Task<Message>) {
        let app = App {
            artist: String::new(),
            title: "Loading…".to_string(),
            listeners: None,
            status: None,
            playback: Playback::Stopped,
            player: None,
        };
        (
            app,
            Task::perform(fetch_now_playing(), Message::NowPlayingFetched),
        )
    }

    fn title(&self) -> String {
        "Wreckers Radio".to_string()
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(Duration::from_secs(POLL_SECS)).map(|_| Message::Tick)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => Task::perform(fetch_now_playing(), Message::NowPlayingFetched),

            Message::NowPlayingFetched(Ok(resp)) => {
                self.artist = resp.now_playing.song.artist;
                self.title = if resp.now_playing.song.title.is_empty() {
                    "Wreckers Radio".to_string()
                } else {
                    resp.now_playing.song.title
                };
                self.listeners = Some(resp.listeners.current);
                Task::none()
            }
            Message::NowPlayingFetched(Err(err)) => {
                self.status = Some(format!("Couldn't reach now-playing API: {err}"));
                Task::none()
            }

            Message::PlayPressed => {
                if self.player.is_none() {
                    match spawn_player(STREAM_URL) {
                        Ok(child) => {
                            self.player = Some(child);
                            self.playback = Playback::Playing;
                            self.status = None;
                        }
                        Err(err) => {
                            self.status = Some(format!(
                                "Couldn't start mpv ({err}). Is mpv installed and on PATH?"
                            ));
                        }
                    }
                }
                Task::none()
            }
            Message::StopPressed => {
                if let Some(mut child) = self.player.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                self.playback = Playback::Stopped;
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let now_playing = if self.artist.is_empty() {
            text(self.title.clone()).size(20)
        } else {
            text(format!("{} — {}", self.artist, self.title)).size(20)
        };

        let listeners = self
            .listeners
            .map(|n| format!("{n} listening"))
            .unwrap_or_default();

        let transport: Element<'_, Message> = match self.playback {
            Playback::Stopped => button(text("▶  Play").size(18))
                .padding([10, 24])
                .on_press(Message::PlayPressed)
                .into(),
            Playback::Playing => button(text("■  Stop").size(18))
                .padding([10, 24])
                .on_press(Message::StopPressed)
                .into(),
        };

        let mut content = column![
            text("Wreckers Radio").size(24),
            Space::new().height(10),
            now_playing,
            text(listeners).size(14),
            Space::new().height(16),
            row![transport].align_y(Alignment::Center),
        ]
        .spacing(6)
        .align_x(Alignment::Center)
        .padding(24)
        .width(Length::Fill);

        if let Some(status) = &self.status {
            content = content
                .push(Space::new().height(12))
                .push(text(status).size(12));
        }

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(mut child) = self.player.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}


/// Spawn `mpv` as a headless audio-only player pointed at the live stream.
/// Swap the binary/args here if you'd rather use `ffplay` or `vlc --intf dummy`.
fn spawn_player(url: &str) -> std::io::Result<Child> {
    Command::new("mpv")
        .args(["--no-video", "--really-quiet", "--force-seekable=no", url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

async fn fetch_now_playing() -> Result<NowPlayingResponse, String> {
    reqwest::get(NOWPLAYING_URL)
        .await
        .map_err(|e| e.to_string())?
        .json::<NowPlayingResponse>()
        .await
        .map_err(|e| e.to_string())
}