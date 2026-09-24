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
// "👍 Like" / "❤ Really Like" buttons append the current artist/title
// to a small memory file at ~/.local/bin/wreckers.data (one line per
// like, tab-separated: unix-timestamp, kind, artist, title). Over time
// this builds up a history of tracks you liked.
//
// A menu bar across the top of the window has a "File" menu (dropdown
// floats over the content) holding the blacklist actions:
//
//   Open Blacklist File…  native "open" dialog (via the `rfd` crate) to
//                         pick an existing file; the app then tracks it.
//   New Blacklist File…   native "save" dialog to choose where a new
//                         blacklist lives; it's created with a header
//                         comment (an existing file is never overwritten)
//                         and then tracked.
//   Edit in Editor        opens the currently tracked file directly in
//                         the OS's default program for it (via the
//                         `open` crate) — no dialog. Created first if
//                         missing.
//   Reload Now            re-reads the file immediately.
//
// Since the editor is an external program, this app can't know the moment
// you hit save in it, so it also reloads the blacklist from disk on every
// 15-second poll — edits take effect automatically within a few seconds.
//
// This app is a single, ordinary `iced::application` window.
//
// Requires `mpv` to be installed and on PATH:
//
//   Linux:  sudo apt install mpv
//   macOS: brew install mpv
//   Windows: winget install mpv (or download from mpv.io and add to PATH)
//
// Blacklist.txt is looked for next to the executable, and failing that,
// in the current working directory. Lines starting with '#' and blank
// lines are ignored. If you open or create a *different* file via the
// File menu, this app switches to tracking that file instead.
//
// wreckers.data is looked for / created at ~/.local/bin/wreckers.data
// (using $HOME, or %USERPROFILE% on Windows).
//
// Cargo.toml must depend on:
//   iced = { version = "0.14", features = ["image"] }
//   rfd = "0.17"
//   open = "5"
// (plus the existing serde / reqwest / tokio dependencies)
//
// Build & run:
//
//   cargo run --release

use iced::widget::{Space, button, column, container, image, mouse_area, row, stack, text};
use iced::{Alignment, Element, Length, Subscription, Task, Theme};

use serde::Deserialize;

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const NOWPLAYING_URL: &str = "https://az.wreckersradio.uk/api/nowplaying/wreckersradio";

// Direct MP3 stream mount.
const STREAM_URL: &str = "https://az.wreckersradio.uk/listen/wreckersradio/radio.mp3";

// The API response is cached for 15 seconds server-side.
const POLL_SECS: u64 = 15;

// Name of the blacklist file, searched for next to the executable and
// in the current working directory.
const BLACKLIST_FILE: &str = "Blacklist.txt";

// Name of the "liked songs" memory file, stored under ~/.local/bin/.
const MEMORY_FILE: &str = "wreckers.data";

pub fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .theme(|_state: &App| Theme::TokyoNightLight)
        .window_size((420.0, 550.0))
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

    // Brief feedback after pressing Like / Really Like.
    like_status: Option<String>,

    playback: Playback,

    // Running mpv process.
    player: Option<Child>,

    // Path used for mpv's JSON IPC socket / named pipe for this run.
    ipc_path: String,

    // Lower-cased blacklist entries loaded from `blacklist_file`. Used
    // for matching against the currently playing track.
    blacklist: Vec<String>,

    // Whether we've currently muted playback because of a blacklist match.
    muted: bool,

    // Whether the "File" menu dropdown is open.
    file_menu_open: bool,

    // Which file we're currently treating as the blacklist. Normally
    // resolved automatically (next to the executable, or the current
    // working directory), but switches to whatever file the user picks
    // or creates via the File menu.
    blacklist_file: PathBuf,
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

    LikePressed,
    ReallyLikePressed,

    FileMenuToggle,
    FileMenuClose,

    OpenBlacklistPressed,

    NewBlacklistPressed,
    EditBlacklistPressed,
    ReloadBlacklistPressed,

    // Result of the Open / New native file dialogs (None = cancelled).
    BlacklistPicked(Option<PathBuf>),

    // Result of handing the file to the OS's default editor.
    BlacklistEditorOpened(Result<(), String>),
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let blacklist_file = default_blacklist_path();
        let blacklist = load_blacklist(&blacklist_file);

        let app = App {
            artist: String::new(),
            title: "Loading…".to_string(),

            art: None,
            art_url: String::new(),

            listeners: None,
            status: None,
            like_status: None,

            playback: Playback::Stopped,
            player: None,

            ipc_path: ipc_path(),
            blacklist,
            muted: false,

            file_menu_open: false,

            blacklist_file,
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

    // Checks the current artist/title against the blacklist and mutes or
    // unmutes the running mpv instance (via IPC) as needed. No-op if
    // nothing is currently playing.
    fn apply_blacklist(&mut self) {
        if self.playback != Playback::Playing {
            return;
        }

        let blacklisted = is_blacklisted(&self.artist, &self.title, &self.blacklist);

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
                    self.status = Some(format!("Couldn't mute via mpv IPC: {err}"));
                }
            }
        } else if !blacklisted && self.muted {
            self.muted = false;

            match send_ipc_volume(&self.ipc_path, 100) {
                Ok(()) => self.status = None,
                Err(err) => {
                    self.status = Some(format!("Couldn't unmute via mpv IPC: {err}"));
                }
            }
        }
    }

    // Switch to tracking a different blacklist file and re-evaluate the
    // currently playing track against it (this also un-mutes if the new
    // list no longer matches).
    fn use_blacklist_file(&mut self, path: PathBuf) {
        self.blacklist_file = path;
        self.blacklist = load_blacklist(&self.blacklist_file);
        self.apply_blacklist();

        self.status = Some(format!(
            "Using {} ({} entries).",
            self.blacklist_file.display(),
            self.blacklist.len()
        ));
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // Poll AzuraCast every 15 seconds. Also reload the blacklist
            // from disk here, so edits made in an external editor (via
            // File > Edit in Editor) take effect automatically.
            Message::Tick => {
                let reloaded = load_blacklist(&self.blacklist_file);

                if reloaded != self.blacklist {
                    self.blacklist = reloaded;
                    self.apply_blacklist();
                }

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

                // Track changed (or first poll): re-evaluate the blacklist
                // and clear any stale "Liked" feedback from a previous track.
                self.apply_blacklist();
                self.like_status = None;

                let new_art_url = resp.now_playing.song.art;

                // IMPORTANT:
                //
                // Only download the artwork if the URL has actually changed.
                //
                // This prevents the image from flashing every 15 seconds.
                if !new_art_url.is_empty() && new_art_url != self.art_url {
                    self.art_url = new_art_url.clone();

                    return Task::perform(download_art(new_art_url), |result| match result {
                        Ok(handle) => Message::ArtworkLoaded(handle),
                        Err(_) => Message::ArtworkFailed,
                    });
                }

                Task::none()
            }

            Message::NowPlayingFetched(Err(err)) => {
                self.status = Some(format!("Couldn't reach now-playing API: {err}"));

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

            Message::LikePressed => {
                self.record_like("LIKE");
                Task::none()
            }

            Message::ReallyLikePressed => {
                self.record_like("REALLY_LIKE");
                Task::none()
            }

            Message::FileMenuToggle => {
                self.file_menu_open = !self.file_menu_open;
                Task::none()
            }

            Message::FileMenuClose => {
                self.file_menu_open = false;
                Task::none()
            }

            Message::OpenBlacklistPressed => {
                self.file_menu_open = false;
                pick_blacklist_task(self.blacklist_file.clone())
            }

            Message::NewBlacklistPressed => {
                self.file_menu_open = false;
                new_blacklist_task(self.blacklist_file.clone())
            }

            Message::EditBlacklistPressed => {
                self.file_menu_open = false;

                let path = self.blacklist_file.clone();

                Task::future(async move {
                    ensure_blacklist_exists(&path);

                    Message::BlacklistEditorOpened(
                        open::that(&path)
                            .map_err(|e| format!("Couldn't open {}: {e}", path.display())),
                    )
                })
            }

            Message::ReloadBlacklistPressed => {
                self.file_menu_open = false;
                self.blacklist = load_blacklist(&self.blacklist_file);
                self.apply_blacklist();

                self.status = Some(format!(
                    "Reloaded {} ({} entries).",
                    self.blacklist_file.display(),
                    self.blacklist.len()
                ));

                Task::none()
            }

            Message::BlacklistPicked(Some(path)) => {
                self.use_blacklist_file(path);
                Task::none()
            }

            Message::BlacklistPicked(None) => Task::none(), // cancelled

            Message::BlacklistEditorOpened(result) => {
                match result {
                    Ok(()) => {
                        self.status = Some(format!(
                            "Opened {} in your editor. Changes are picked up automatically.",
                            self.blacklist_file.display()
                        ));
                    }
                    Err(err) => self.status = Some(err),
                }

                Task::none()
            }
        }
    }

    // Appends the currently playing track to the "liked songs" memory
    // file, and sets `like_status` with feedback for the UI.
    fn record_like(&mut self, kind: &str) {
        if self.artist.is_empty() && self.title.is_empty() {
            self.like_status = Some("Nothing playing yet.".to_string());
            return;
        }

        match append_like(kind, &self.artist, &self.title) {
            Ok(()) => {
                let label = if kind == "REALLY_LIKE" {
                    "❤ Really liked"
                } else {
                    "👍 Liked"
                };

                self.like_status = Some(format!("{label}: {} — {}", self.artist, self.title));
            }
            Err(err) => {
                self.like_status = Some(format!("Couldn't save like: {err}"));
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

        // Like / Really Like buttons. Only active once we know what's
        // playing.
        let can_like = !self.artist.is_empty() || !self.title.is_empty();

        let like_button: Element<'_, Message> = {
            let b = button(text("👍 Like").size(14)).padding([6, 14]);

            if can_like {
                b.on_press(Message::LikePressed).into()
            } else {
                b.into()
            }
        };

        let really_like_button: Element<'_, Message> = {
            let b = button(text("❤ Really Like").size(14)).padding([6, 14]);

            if can_like {
                b.on_press(Message::ReallyLikePressed).into()
            } else {
                b.into()
            }
        };

        let like_row = row![like_button, really_like_button]
            .spacing(10)
            .align_y(Alignment::Center);

        let like_status: Element<'_, Message> = match &self.like_status {
            Some(msg) => text(msg.clone()).size(12).into(),
            None => Space::new().height(0).into(),
        };

        // --- Menu bar: full-width strip at the very top of the window ---
        const BAR_HEIGHT: f32 = 30.0;

        let menu_bar = container(row![
            button(text("File").size(14))
                .padding([4, 10])
                .style(button::text)
                .on_press(Message::FileMenuToggle)
        ])
        .width(Length::Fill)
        .padding([2, 4])
        .style(|theme: &Theme| {
            container::Style::default()
                .background(theme.extended_palette().background.weak.color)
        });

        // Dropdown, drawn as an overlay just under the bar.
        let dropdown: Element<'_, Message> = if self.file_menu_open {
            let item = |label: &str, msg: Message| {
                button(text(label.to_string()).size(14))
                    .padding([6, 12])
                    .width(Length::Fill)
                    .style(button::text)
                    .on_press(msg)
            };

            let file_name = self
                .blacklist_file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| BLACKLIST_FILE.to_string());

            let menu = column![
                text(format!("{file_name} · {} entries", self.blacklist.len())).size(11),
                item("Open Blacklist File…", Message::OpenBlacklistPressed),
                item("New Blacklist File…", Message::NewBlacklistPressed),
                item("Edit in Editor", Message::EditBlacklistPressed),
                item("Reload Now", Message::ReloadBlacklistPressed),
            ]
            .spacing(2);

            // Closes when the pointer leaves the dropdown, and when
            // clicking anywhere outside it (the full-window overlay).
            let menu_box = mouse_area(
                container(menu)
                    .width(Length::Fixed(220.0))
                    .padding(6)
                    .style(container::bordered_box),
            )
            .on_exit(Message::FileMenuClose);

            mouse_area(
                container(menu_box)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(iced::Padding::ZERO.top(BAR_HEIGHT).left(4.0)),
            )
            .on_press(Message::FileMenuClose)
            .into()
        } else {
            Space::new().into()
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
            Space::new().height(10),
            like_row,
            like_status,
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

        let body = container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill);

        let page = column![menu_bar, body];

        if self.file_menu_open {
            stack![page, dropdown].into()
        } else {
            page.into()
        }
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
    let cmd = format!(r#"{{"command": ["set_property", "volume", {level}]}}"#);

    send_ipc_line(ipc_path, &cmd)
}

#[cfg(unix)]
fn send_ipc_line(ipc_path: &str, line: &str) -> Result<(), String> {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(ipc_path).map_err(|e| e.to_string())?;

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

// Resolves the default Blacklist.txt path: next to the executable if a
// file already exists there, otherwise the current working directory.
// (Overridden at runtime if the user opens or creates a different file
// via the File menu.)
fn default_blacklist_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(BLACKLIST_FILE);

            if candidate.exists() {
                return candidate;
            }
        }
    }

    PathBuf::from(BLACKLIST_FILE)
}

// Load blacklist entries (lower-cased, for matching) from the given
// file. Returns an empty list (i.e. nothing is ever blacklisted) if the
// file doesn't exist or can't be read.
fn load_blacklist(path: &Path) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(content) => content
            .lines()
            .map(|line| line.trim().to_lowercase())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect(),
        Err(_) => Vec::new(),
    }
}

// True if any blacklist entry appears (case-insensitively) in the
// combined "<artist> <title>" string.
fn is_blacklisted(artist: &str, title: &str, blacklist: &[String]) -> bool {
    if blacklist.is_empty() {
        return false;
    }

    let haystack = format!("{artist} {title}").to_lowercase();

    blacklist
        .iter()
        .any(|entry| haystack.contains(entry.as_str()))
}

// Path to the "liked songs" memory file: ~/.local/bin/wreckers.data
fn memory_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());

    PathBuf::from(home)
        .join(".local")
        .join("bin")
        .join(MEMORY_FILE)
}

// Appends one line to the memory file:
//   <unix-timestamp>\t<LIKE|REALLY_LIKE>\t<artist>\t<title>
fn append_like(kind: &str, artist: &str, title: &str) -> std::io::Result<()> {
    let path = memory_path();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    writeln!(file, "{timestamp}\t{kind}\t{artist}\t{title}")
}

// Writes a starter blacklist file if one doesn't exist yet. Never
// overwrites an existing file.
fn ensure_blacklist_exists(path: &Path) {
    if path.exists() {
        return;
    }

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let _ = std::fs::write(
        path,
        "# One artist or track name per line.\n\
         # Lines starting with '#' are ignored.\n",
    );
}

// Splits the current blacklist path into a directory (for dialogs to
// start in) and a file name (to pre-fill save dialogs).
fn dialog_dir_and_name(current: &Path) -> (PathBuf, String) {
    let dir = current
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let name = current
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| BLACKLIST_FILE.to_string());

    (dir, name)
}

// Native "open file" dialog: pick an existing blacklist file to track.
fn pick_blacklist_task(current: PathBuf) -> Task<Message> {
    Task::future(async move {
        let (dir, _) = dialog_dir_and_name(&current);

        let picked = rfd::AsyncFileDialog::new()
            .set_title("Open Blacklist File")
            .add_filter("Text files", &["txt"])
            .set_directory(&dir)
            .pick_file()
            .await;

        Message::BlacklistPicked(picked.map(|h| h.path().to_path_buf()))
    })
}

// Native "save file" dialog: choose where a new blacklist file should
// live. An existing file the user overwrites/selects is left untouched.
fn new_blacklist_task(current: PathBuf) -> Task<Message> {
    Task::future(async move {
        let (dir, name) = dialog_dir_and_name(&current);

        let picked = rfd::AsyncFileDialog::new()
            .set_title("New Blacklist File")
            .add_filter("Text files", &["txt"])
            .set_directory(&dir)
            .set_file_name(&name)
            .save_file()
            .await;

        Message::BlacklistPicked(picked.map(|h| {
            let path = h.path().to_path_buf();
            ensure_blacklist_exists(&path);
            path
        }))
    })
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