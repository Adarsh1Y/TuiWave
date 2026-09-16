# lastwave

A **LastWave-inspired YouTube Music player for the Linux terminal**. Search YouTube
Music directly through its InnerTube API, stream audio through `mpv`, and enjoy
full-screen terminal album art — all without the YouTube web player.

![TUI](https://img.shields.io/badge/TUI-ratatui-teal)
![Rust](https://img.shields.io/badge/Rust-2024-orange)

## Features

- **Direct InnerTube streaming** — no browser, no cookies, no YouTube web client.
- **Codec-aware streaming** — picks the best Opus (itag 251) or AAC (itag 140)
  audio-only stream automatically, or honours a preferred codec from the config.
- **Reliable stream resolution** — tries the pre-signed `VISIONOS` client first,
  falls back through other clients, an in-house `nsig` decipher, and finally the
  `yt-dlp` binary you may already have installed.
- **`mpv`-powered playback engine** — controls mpv over a JSON-RPC Unix socket with
  real-time playback state (`time-pos`, duration, volume, pause, title).
- **Local file library** — browse and play FLAC / OPUS / OGG / M4A / MP3 / AAC
  files from your local directories through the same mpv engine.
- **Playlists** — one-key *Like* with a persistent Liked list, named playlists
  saved from the queue, and full YouTube Music playlists via URL.
- **Deep-bass EQ** — a built-in EQ preset (bass/mid/treble) applied through mpv's
  `af` chain, toggle with one key, persisted in the config.
- **Resilient playback** — a dead/stalled stream is re-resolved once and, if it
  fails again, playback moves on to the next track.
- **Song-only search** — video and episode results are filtered out of search.
- **Terminal album art** — half-block art rendered from the track thumbnail,
  with full-resolution kitty graphics when running under kitty; cached under
  `~/.cache/lastwave/art`.
- **MPRIS integration** — exposes an MPRIS2 media player service so external
  remotes (`playerctl`, media keys, desktop shells) can control playback.
- **`--play`** — start a local file or a search autoplay straight from the
  command line.
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

### One-liner (recommended)

```sh
curl -fsSL https://raw.githubusercontent.com/Adarsh1Y/TuiWave/master/install.sh | sh
```

Installs the latest prebuilt binary to `~/.local/bin/lastwave`.
Works on x86_64 and arm64 Linux, no Rust toolchain needed.

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

### Arch Linux (PKGBUILD)

```sh
git clone https://github.com/Adarsh1Y/TuiWave
cd TuiWave
makepkg -si            # from packaging/PKGBUILD
```

The package installs the binary, the `lastwave(1)` man page, an MIT license,
and shell completions for bash, zsh, and fish.

### Man page and completions

- Man page: `man/lastwave.1` — also shown via `man lastwave` after installation.
- Completions: `completions/lastwave.bash`, `completions/lastwave.zsh`, and
  `completions/_lastwave` (fish). Source them directly or copy them into your
  shell's completion directory.

### Verify

```sh
lastwave --version
```

## Usage

```text
Usage: lastwave [OPTIONS]

Options:
  -v, --volume <VOLUME>      Start volume (0-150), overrides config
      --mpv-path <MPV_PATH>  Path to the mpv binary
      --resume               Restore the previous session on start
      --no-resume            Start fresh, ignoring any saved session
      --play <ARG>           Play a local file or search query immediately
  -h, --help                 Print help
  -V, --version              Print version
```

`--play` plays an existing local file path directly, or treats `ARG` as a
search query and autoplays the first playable result.

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
codec = "best"          # "best" | "opus" | "aac"
local_dirs = ["/home/you/Music"]
eq = { preset = "Deep Bass", presets = [
    { name = "Deep Bass", bass = 12, mid = 0, treble = -2 }
] }
```

- `codec` — preferred stream codec (`best`, `opus`, `aac`).
- `local_dirs` — folders scanned for the local file library (`u`).
- `eq` — `preset` names an active preset from `presets`. Each preset maps its
  **bass/mid/treble** gains to an mpv filter chain.

## Controls

> **One rule everywhere: bare letters type; `Alt+letter` acts.**
> A persistent key-bind bar at the bottom of every screen shows the current
> mode's bindings; `?` shows the full list.

### Now Playing

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `←` / `→` | Seek −10s / +10s |
| `Shift+←` / `Shift+→` | Seek −60s / +60s |
| `,` / `.` | Volume down / up (5) |
| `Alt+n` / `Alt+p` | Next / previous track |
| `Alt+r` | Cycle repeat mode |
| `Alt+z` | Toggle shuffle |
| `Alt+l` | Like / unlike current track |
| `Alt+L` | Open the Liked playlist |
| `Alt+P` | Browse playlists |
| `Alt+y` | Load a YouTube Music playlist (URL or id) |
| `Alt+S` | Save current queue as a playlist |
| `Alt+u` | Local file library |
| `Alt+e` | Toggle the EQ preset |
| `Alt+x` | Force clean (no EQ) |
| `Alt+o` / `Alt+t` | Open the queue |
| `s` or `/` | Search |
| `?` | Help overlay |
| `q` or `Ctrl+C` | Quit |

### Search

| Key | Action |
|---|---|
| Any character | Add to the query (live search) |
| `Backspace` | Remove last character |
| `↑` / `↓` | Select result |
| `Enter` | Play selected result |
| `Alt+l` | Like / unlike selected result |
| `Esc` | Back to Now Playing |

### Queue

| Key | Action |
|---|---|
| `↑` / `↓` or `k` / `j` | Move selection |
| `Enter` | Play selected item |
| `Alt+d` / `Delete` | Remove item |
| `Alt+z` | Toggle shuffle |
| `Alt+r` | Cycle repeat mode |
| `Alt+o` / `Alt+t` or `Esc` | Back to Now Playing |

### Playlists & Local

| Key | Action |
|---|---|
| `↑` / `↓` or `k` / `j` | Move selection |
| `Enter` | Play selected playlist / track |
| `Alt+l` | Like / unlike a track |
| `Alt+d` / `Delete` | Remove from playlist (detail view) |
| `Alt+x` | Delete the playlist entirely (detail view) |
| `Esc` (`Alt+P`/`Alt+u` to re-enter) | Back to Now Playing |

## How streaming works

Every video id goes through `resolve_stream`:

1. **`VISIONOS` client** — the same pre-signed client yt-dlp favours upstream.
   Sends a player request with a Safari device context to
   `https://www.youtube.com/youtubei/v1/player` and picks the best audio-only
   format for the configured codec — Opus (itag 251) or AAC (itag 140), falling
   back through lower-bitrate formats as needed.
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
├── config.rs            # ~/.config/lastwave/config.toml handling (codec, EQ, local dirs)
├── model.rs             # Track model (YT Music + local files)
├── library.rs           # Local FLAC/Opus library scanner
├── playlists.rs         # Liked + named playlist JSON store
├── mpv.rs               # mpv JSON-RPC engine + playback state + EQ chain
├── art.rs               # Terminal album art rendering
├── tui/                 # ratatui interface (app + view)
└── innertube/           # Direct InnerTube API client
    ├── config.rs        # Innertube config scraping (key, client version…)
    ├── search.rs        # YouTube Music search (+ song-only filtering)
    ├── player.rs        # multi-client stream resolution + codec pick + verification
    ├── playlist.rs      # YouTube Music playlist fetching (continuations)
    ├── nsig.rs          # QuickJS-powered nsig decipher
    └── ytdl.rs          # yt-dlp fallback resolver
```

## License

This project is not affiliated with, endorsed by, or connected to YouTube,
Google, or mpv. Content legality follows the provisions of your jurisdiction.