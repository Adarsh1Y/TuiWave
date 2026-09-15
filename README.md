# lastwave

A **LastWave-inspired YouTube Music player for the Linux terminal**. Search YouTube
Music directly through its InnerTube API, stream audio through `mpv`, and enjoy
full-screen terminal album art — all without the YouTube web player.

![TUI](https://img.shields.io/badge/TUI-ratatui-teal)
![Rust](https://img.shields.io/badge/Rust-2024-orange)

## Features

- **Direct InnerTube streaming** — no browser, no cookies, no YouTube web client.
- **Reliable stream resolution** — tries the pre-signed `VISIONOS` client first,
  falls back through other clients, an in-house `nsig` decipher, and finally the
  `yt-dlp` binary you may already have installed.
- **`mpv`-powered playback engine** — controls mpv over a JSON-RPC Unix socket with
  real-time playback state (`time-pos`, duration, volume, pause, title).
- **Terminal album art** — rasterised, half-block art rendered from the track
  thumbnail, cached under `~/.cache/lastwave/art`.
- **Persistent search history** and an autoplaying queue with shuffle/repeat.

## Dependencies

| Dependency | Why |
|---|---|
| **`mpv`** | Audio playback engine |
| **`yt-dlp`** (optional) | Fallback stream resolver if the direct path fails |
| **A YouTube-compatible network** | Uses YouTube's public InnerTube endpoints |

Only `mpv` is strictly required; the app works without `yt-dlp` unless YouTube's
bot-gating outpaces the direct resolver.

## Installation

### Prerequisites

Install `mpv` with your package manager:

```sh
# Arch Linux
sudo pacman -S mpv

# Debian / Ubuntu
sudo apt install mpv

# Fedora
sudo dnf install mpv
```

Optionally install `yt-dlp` for the fallback resolver:

```sh
sudo pacman -S yt-dlp        # Arch
sudo apt install yt-dlp      # Debian / Ubuntu
sudo dnf install yt-dlp      # Fedora
```

### Build from source

You need a Rust toolchain (edition 2024):

```sh
# Install Rust (if you don't have it):
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build and install:
cargo build --release
sudo install -m755 target/release/lastwave /usr/local/bin/lastwave
```

### Verify

```sh
lastwave --version
```

## Usage

```text
Usage: lastwave [OPTIONS]

Options:
  -v, --volume <VOLUME>    Start volume (0-150), overrides config
      --mpv-path <MPV_PATH>  Path to the mpv binary
  -h, --help               Print help
  -V, --version            Print version
```

### First run

Launch lastwave:

```sh
lastwave
```

It creates `~/.config/lastwave/config.toml` on first run. Searching YouTube
Music requires no authentication.

### Configuration

`~/.config/lastwave/config.toml`:

```toml
volume = 100
mpv_path = "mpv"
cache_dir = "/home/you/.cache/lastwave"
autoplay = true
```

## Controls

### Now Playing

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `←` / `→` | Seek −10s / +10s |
| `Shift+←` / `Shift+→` | Seek −60s / +60s |
| `,` / `.` | Volume down / up (5) |
| `n` / `p` | Next / previous track |
| `r` | Cycle repeat mode |
| `z` | Toggle shuffle |
| `s` or `/` | Search |
| `o` or `t` | Open the queue |
| `?` | Help overlay |
| `q` or `Ctrl+C` | Quit |

### Search

| Key | Action |
|---|---|
| Any character | Add to the query (live search) |
| `Backspace` | Remove last character |
| `↑` / `↓` | Select result |
| `Enter` | Play selected result |
| `Esc` | Back to Now Playing |

### Queue

| Key | Action |
|---|---|
| `↑` / `↓` or `k` / `j` | Move selection |
| `Enter` | Play selected item |
| `d` / `Delete` | Remove item |
| `z` | Toggle shuffle |
| `r` | Cycle repeat mode |
| `Esc`, `o` or `t` | Back to Now Playing |

## How streaming works

Every video id goes through `resolve_stream`:

1. **`VISIONOS` client** — the same pre-signed client yt-dlp favours upstream.
   Sends a player request with a Safari device context to
   `https://www.youtube.com/youtubei/v1/player` and picks the best audio-only
   format (itag 251 > 140 > 141 > 139).
2. **Verification** — every candidate URL is checked with a small ranged GET
   before playback; dead/bot-gated URLs are skipped.
3. **`nsig` decipher** — if a valid URL carries an `n` challenge, we try to
   decrypt it with a behavioural solver that locates the n-function in the
   current player script and runs it in an embedded QuickJS runtime.
4. **Other clients** — `IOS` and `ANDROID_VR` cover older or restricted content.
5. **`yt-dlp` fallback** — if every direct path fails, the `yt-dlp` binary
   (if installed) resolves the stream as a last resort.

## Project layout

```
src/
├── main.rs              # CLI entry point (clap)
├── config.rs            # ~/.config/lastwave/config.toml handling
├── model.rs             # Track model
├── mpv.rs               # mpv JSON-RPC engine + playback state
├── art.rs               # Terminal album art rendering
├── tui/                 # ratatui interface (app + view)
└── innertube/           # Direct InnerTube API client
    ├── config.rs        # Innertube config scraping (key, client version…)
    ├── search.rs        # YouTube Music search
    ├── player.rs        # multi-client stream resolution + verification
    ├── nsig.rs          # QuickJS-powered nsig decipher
    └── ytdl.rs          # yt-dlp fallback resolver
```

## License

This project is not affiliated with, endorsed by, or connected to YouTube,
Google, or mpv. Content legality follows the provisions of your jurisdiction.