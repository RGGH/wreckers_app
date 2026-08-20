// Wreckers Radio desktop player
//
// Polls Wreckers Radio's AzuraCast "now playing" API for the current
// artist/title, displays the current artwork, and streams the live MP3
// by shelling out to `mpv`.
//
// Requires `mpv` to be installed and on PATH:
//
//   Linux:  sudo apt install mpv
//   macOS: brew install mpv
//
// Build & run:
//
//   cargo run --release

use iced::widget::{Space, button, column, container, image, row, text};
use iced::{Alignment, Element, Length, Subscription, Task, Theme};

use serde::Deserialize;

use std::process::{Child, Command, Stdio};
use std::time::Duration;

const NOWPLAYING_URL: &str =
    "https://az.wreckersradio.uk/api/nowplaying/wreckersradio";

// Direct MP3 stream mount.
const STREAM_URL: &str =
    "https://az.wreckersradio.uk/listen/wreckersradio/radio.mp3";

// The API response is cached for 15 seconds server-side.
const POLL_SECS: u64 = 15;

pub fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .theme(|_state: &App| Theme::TokyoNightLight)
        .window_size((420.0, 520.0))
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

    // Currently displayed artwork.
    art: Option<image::Handle>,

    // URL of the artwork currently displayed/requested.
    // Used to avoid downloading the same image every 15 seconds.
    art_url: String,

    listeners: Option<u64>,
    status: Option<String>,

    playback: Playback,

    // Running mpv process.
    player: Option<Child>,
}

#[derive(Debug, Clone)]
enum Message {
    Tick,

    // Only contains the API response.
    NowPlayingFetched(Result<NowPlayingResponse, String>),

    // Sent only when we actually download new artwork.
    ArtworkLoaded(image::Handle),

    ArtworkFailed,

    PlayPressed,
    StopPressed,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let app = App {
            artist: String::new(),
            title: "Loading…".to_string(),

            art: None,
            art_url: String::new(),

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
        iced::time::every(Duration::from_secs(POLL_SECS))
            .map(|_| Message::Tick)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // Poll AzuraCast every 15 seconds.
            Message::Tick => {
                Task::perform(fetch_now_playing(), Message::NowPlayingFetched)
            }

            // API response arrived.
            Message::NowPlayingFetched(Ok(resp)) => {
                self.artist = resp.now_playing.song.artist;

                self.title = if resp.now_playing.song.title.is_empty() {
                    "Wreckers Radio".to_string()
                } else {
                    resp.now_playing.song.title
                };

                self.listeners = Some(resp.listeners.current);

                let new_art_url = resp.now_playing.song.art;

                // IMPORTANT:
                //
                // Only download the artwork if the URL has actually changed.
                //
                // This prevents the image from flashing every 15 seconds.
                if !new_art_url.is_empty() && new_art_url != self.art_url {
                    self.art_url = new_art_url.clone();

                    return Task::perform(
                        download_art(new_art_url),
                        |result| match result {
                            Ok(handle) => Message::ArtworkLoaded(handle),
                            Err(_) => Message::ArtworkFailed,
                        },
                    );
                }

                Task::none()
            }

            Message::NowPlayingFetched(Err(err)) => {
                self.status =
                    Some(format!("Couldn't reach now-playing API: {err}"));

                Task::none()
            }

            // New artwork has finished downloading.
            Message::ArtworkLoaded(handle) => {
                self.art = Some(handle);
                Task::none()
            }

            Message::ArtworkFailed => {
                // Keep displaying the previous artwork if the new one fails.
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
                                "Couldn't start mpv ({err}). \
                                 Is mpv installed and on PATH?"
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

        // Display artwork if we have one.
        //
        // If no artwork has loaded yet, reserve exactly the same amount
        // of space so the GUI doesn't jump around.
        let artwork: Element<'_, Message> = match &self.art {
            Some(handle) => image(handle.clone())
                .width(Length::Fixed(160.0))
                .height(Length::Fixed(160.0))
                .into(),

            None => Space::new()
                .width(Length::Fixed(160.0))
                .height(Length::Fixed(160.0))
                .into(),
        };

        let mut content = column![
            text("Wreckers Radio").size(24),

            Space::new().height(10),

            artwork,

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

// Spawn mpv as a headless audio-only player pointed at the live stream.
fn spawn_player(url: &str) -> std::io::Result<Child> {
    Command::new("mpv")
        .args([
            "--no-video",
            "--really-quiet",
            "--force-seekable=no",
            url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

// Fetch only the now-playing JSON.
//
// IMPORTANT: Artwork is NOT downloaded here.
// That allows us to compare the artwork URL with `self.art_url` first.
async fn fetch_now_playing() -> Result<NowPlayingResponse, String> {
    reqwest::get(NOWPLAYING_URL)
        .await
        .map_err(|e| e.to_string())?
        .json::<NowPlayingResponse>()
        .await
        .map_err(|e| e.to_string())
}

// Download artwork only when the artwork URL changes.
async fn download_art(url: String) -> Result<image::Handle, String> {
    let bytes = reqwest::get(url)
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    Ok(image::Handle::from_bytes(bytes))
}