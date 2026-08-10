# Wreckers Radio Desktop Player

A lightweight desktop player for **Wreckers Radio**.

Donate to help the station : https://wreckers.radio/ - link on the homepage.

It displays the currently playing artist/title and listener count, and lets you play the live radio stream using [`mpv`](https://mpv.io/) under the hood.

Built with **Rust + Iced**.

## Features

* 🎵 Live Wreckers Radio stream
* 📻 Displays the currently playing artist and title
* 👥 Shows current listener count
* ▶️ Play / stop controls
* 🔄 Automatically refreshes now-playing information
* 🖥️ Works on Linux, macOS and Windows
* 🔇 Uses `mpv` as a lightweight headless audio player

---

# Requirements

The application requires **mpv** to be installed and available on your system.

You also need the Wreckers Radio desktop player itself.

---

# Linux

## 1. Install mpv

### Debian / Ubuntu

```bash
sudo apt update
sudo apt install mpv
```

### Fedora

```bash
sudo dnf install mpv
```

### Arch Linux

```bash
sudo pacman -S mpv
```

You can check that mpv is installed with:

```bash
mpv --version
```

## 2. Download Wreckers Radio

Download the latest Linux release from the project's **Releases** page.

Make the downloaded file executable if necessary:

```bash
chmod +x wreckers-radio
```

Then run it:

```bash
./wreckers-radio
```

---

# macOS

## 1. Install mpv

The easiest option is Homebrew.

If you already have Homebrew:

```bash
brew install mpv
```

Then verify:

```bash
mpv --version
```

If you don't have Homebrew, install it from the official website:

[https://brew.sh/](https://brew.sh/)

## 2. Download Wreckers Radio

Download the latest macOS release from the project's **Releases** page.

Depending on your Mac, use the appropriate build:

* Apple Silicon — M1 / M2 / M3 / M4
* Intel — older Intel Macs

Open the application to start Wreckers Radio.

> **macOS security notice:** If macOS says the application cannot be opened because it is from an unidentified developer, you may need to allow it under **System Settings → Privacy & Security**.

---

# Windows

## 1. Install mpv

Download mpv for Windows from the official mpv website:

[https://mpv.io/installation/](https://mpv.io/installation/)

Extract the downloaded archive.

The application needs to be able to find:

```text
mpv.exe
```

The easiest setup is to add the directory containing `mpv.exe` to your Windows **PATH**.

You can check this from PowerShell:

```powershell
mpv --version
```

If that prints the mpv version, you're ready.

## 2. Download Wreckers Radio

Download the latest Windows release from the project's **Releases** page.

Run:

```text
wreckers-radio.exe
```

The player will launch and the **Play** button will start the live stream.

---

# Running from Source

If you want to build the application yourself, you'll need:

* Rust
* Cargo
* mpv

## 1. Install Rust

Install Rust using `rustup`:

[https://rustup.rs/](https://rustup.rs/)

Check your installation:

```bash
rustc --version
cargo --version
```

## 2. Install mpv

Install mpv using the instructions for your operating system above.

Make sure this works:

```bash
mpv --version
```

## 3. Clone the repository

```bash
git clone https://github.com/RGGH/wreckers_app
cd wreckers-radio
```

## 4. Run

```bash
cargo run --release
```

Or build the application:

```bash
cargo build --release
```

The compiled executable will be located in:

```text
target/release/
```

On Windows it will be:

```text
target/release/wreckers-radio.exe
```

---

# How It Works

Wreckers Radio provides an AzuraCast **now-playing API**.

The application periodically requests the current track information:

```text
Wreckers Radio
      │
      ▼
AzuraCast now-playing API
      │
      ▼
Artist / Title / Listeners
      │
      ▼
     Iced
      │
      ▼
Desktop UI
```

The live audio is handled separately by `mpv`:

```text
Wreckers Radio MP3 Stream
          │
          ▼
         mpv
          │
          ▼
       Speakers
```

The application launches `mpv` when **Play** is pressed and terminates it when **Stop** is pressed or when the application closes.

---

# Troubleshooting

## "Couldn't start mpv"

If you see:

```text
Couldn't start mpv
```

make sure mpv is installed:

```bash
mpv --version
```

If the command isn't found, mpv either isn't installed or isn't available on your system `PATH`.

---

## mpv works from the terminal but not from the app

Make sure the `mpv` executable is available through the environment used to launch the application.

For example:

```bash
which mpv
```

on Linux/macOS, or:

```powershell
where.exe mpv
```

on Windows.

---

## No audio

Try running mpv directly with the stream URL:

```bash
mpv https://az.wreckersradio.uk/listen/wreckersradio/radio.mp3
```

If that doesn't produce audio, the issue is likely with mpv, your audio device, or the network rather than the desktop application.

---

# Development

The application uses:

* [Rust](https://www.rust-lang.org/)
* [Iced](https://iced.rs/)
* [Reqwest](https://crates.io/crates/reqwest)
* [Serde](https://serde.rs/)
* [mpv](https://mpv.io/)

The application does not decode the radio stream itself. `mpv` handles streaming, buffering, reconnects and audio playback.

---

# License

```text
MIT License
```



---

# Wreckers Radio

🎵 **Listen to Wreckers Radio and support independent radio.**


