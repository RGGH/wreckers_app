// Wreckers Radio desktop player
//
// Polls Wreckers Radio's AzuraCast "now playing" API for the current
// artist/title, displays the current artwork, and streams the live MP3
// by shelling out to `mpv`.
//
// Also reads a "Blacklist.txt" file (one entry per line, matched
// case-insensitively against "<artist> <title>"). If the currently
// playing track matches a blacklist entry, the mpv volume is set to 0
// via mpv's JSON IPC socket. When the track changes to something that
// no longer matches, volume is restored to 100.
//
// Requires `mpv` to be installed and on PATH:
//
//   Linux:  sudo apt install mpv
//   macOS: brew install mpv
//   Windows: winget install mpv (or download from mpv.io and add to PATH)
//
// Blacklist.txt is looked for next to the executable, and failing that,
// in the current working directory. Lines starting with '#' and blank
// lines are ignored.
//
// Build & run:
//
//   cargo run --release

use iced::widget::{Space, button, column, container, image, row, text};
use iced::{Alignment, Element, Length, Subscription, Task, Theme};

use serde::Deserialize;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const NOWPLAYING_URL: &str =
    "https://az.wreckersradio.uk/api/nowplaying/wreckersradio";

// Direct MP3 stream mount.
const STREAM_URL: &str =
    "https://az.wreckersradio.uk/listen/wreckersradio/radio.mp3";

// The API response is cached for 15 seconds server-side.
const POLL_SECS: u64 = 15;

// Name of the blacklist file, searched for next to the executable and
// in the current working directory.
const BLACKLIST_FILE: &str = "Blacklist.txt";

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

    // Path used for mpv's JSON IPC socket / named pipe for this run.
    ipc_path: String,

    // Lower-cased blacklist entries loaded from Blacklist.txt.
    blacklist: Vec<String>,

    // Whether we've currently muted playback because of a blacklist match.
    muted: bool,
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

            ipc_path: ipc_path(),
            blacklist: load_blacklist(),
            muted: false,
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

    // Checks the current artist/title against the blacklist and mutes or
    // unmutes the running mpv instance (via IPC) as needed. No-op if
    // nothing is currently playing.
    fn apply_blacklist(&mut self) {
        if self.playback != Playback::Playing {
            return;
        }

        let blacklisted =
            is_blacklisted(&self.artist, &self.title, &self.blacklist);

        if blacklisted && !self.muted {
            self.muted = true;

            match send_ipc_volume(&self.ipc_path, 0) {
                Ok(()) => {
                    self.status = Some(format!(
                        "Muted (blacklisted): {} — {}",
                        self.artist, self.title
                    ));
                }
                Err(err) => {
                    self.status =
                        Some(format!("Couldn't mute via mpv IPC: {err}"));
                }
            }
        } else if !blacklisted && self.muted {
            self.muted = false;

            match send_ipc_volume(&self.ipc_path, 100) {
                Ok(()) => self.status = None,
                Err(err) => {
                    self.status =
                        Some(format!("Couldn't unmute via mpv IPC: {err}"));
                }
            }
        }
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

                // Track changed (or first poll): re-evaluate the blacklist.
                self.apply_blacklist();

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
                    match spawn_player(STREAM_URL, &self.ipc_path) {
                        Ok(child) => {
                            self.player = Some(child);
                            self.playback = Playback::Playing;
                            self.muted = false;
                            self.status = None;

                            // Give mpv a brief moment to create its IPC
                            // socket/pipe before we might try to write to
                            // it, then check whether the track that's
                            // about to play is already blacklisted.
                            std::thread::sleep(Duration::from_millis(300));
                            self.apply_blacklist();
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
                self.muted = false;

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

        let mute_label: Element<'_, Message> = if self.muted {
            text("🔇 muted (blacklisted track)").size(12).into()
        } else {
            Space::new().height(0).into()
        };

        let mut content = column![
            text("Wreckers Radio").size(24),

            Space::new().height(10),

            artwork,

            now_playing,

            text(listeners).size(14),

            mute_label,

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

// Spawn mpv as a headless audio-only player pointed at the live stream,
// with a JSON IPC server enabled so we can adjust volume at runtime.
fn spawn_player(url: &str, ipc_path: &str) -> std::io::Result<Child> {
    Command::new("mpv")
        .args([
            "--no-video",
            "--really-quiet",
            "--force-seekable=no",
            "--volume=100",
            &format!("--input-ipc-server={ipc_path}"),
            url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

// Build a per-run path/name for mpv's IPC endpoint.
//
// On Unix this is a filesystem path to a socket in the temp directory.
// On Windows this is a named pipe name under \\.\pipe\.
#[cfg(unix)]
fn ipc_path() -> String {
    std::env::temp_dir()
        .join(format!("wreckersradio-mpv-{}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

#[cfg(windows)]
fn ipc_path() -> String {
    format!(r"\\.\pipe\wreckersradio-mpv-{}", std::process::id())
}

// Send a "set_property volume <level>" command to the running mpv
// instance over its IPC socket/pipe.
fn send_ipc_volume(ipc_path: &str, level: u8) -> Result<(), String> {
    let cmd = format!(
        r#"{{"command": ["set_property", "volume", {level}]}}"#
    );

    send_ipc_line(ipc_path, &cmd)
}

#[cfg(unix)]
fn send_ipc_line(ipc_path: &str, line: &str) -> Result<(), String> {
    use std::os::unix::net::UnixStream;

    let mut stream =
        UnixStream::connect(ipc_path).map_err(|e| e.to_string())?;

    writeln!(stream, "{line}").map_err(|e| e.to_string())
}

#[cfg(windows)]
fn send_ipc_line(ipc_path: &str, line: &str) -> Result<(), String> {
    use std::fs::OpenOptions;

    // mpv's named pipe can be opened like a regular file on Windows once
    // the server side is listening (CreateFile under the hood).
    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(ipc_path)
        .map_err(|e| e.to_string())?;

    writeln!(pipe, "{line}").map_err(|e| e.to_string())
}

// Load blacklist entries from Blacklist.txt, checking next to the
// executable first, then the current working directory. Returns an
// empty list (i.e. nothing is ever blacklisted) if no file is found.
fn load_blacklist() -> Vec<String> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(BLACKLIST_FILE));
        }
    }

    candidates.push(PathBuf::from(BLACKLIST_FILE));

    for candidate in candidates {
        if let Ok(content) = std::fs::read_to_string(&candidate) {
            return content
                .lines()
                .map(|line| line.trim().to_lowercase())
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .collect();
        }
    }

    Vec::new()
}

// True if any blacklist entry appears (case-insensitively) in the
// combined "<artist> <title>" string.
fn is_blacklisted(artist: &str, title: &str, blacklist: &[String]) -> bool {
    if blacklist.is_empty() {
        return false;
    }

    let haystack = format!("{artist} {title}").to_lowercase();

    blacklist.iter().any(|entry| haystack.contains(entry.as_str()))
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