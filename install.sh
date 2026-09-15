#!/usr/bin/env sh
set -eu

REPO="Adarsh1Y/TuiWave"
BIN="lastwave"

case "$(uname -m)" in
    x86_64 | amd64) TARGET="x86_64-unknown-linux-gnu" ;;
    aarch64 | arm64) TARGET="aarch64-unknown-linux-gnu" ;;
    *)
        echo "error: unsupported architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

if command -v mpv >/dev/null 2>&1; then
    :
else
    echo "note: mpv was not found. lastwave needs it to play audio." >&2
    echo "      install it first, e.g.: sudo pacman -S mpv | sudo apt install mpv" >&2
fi

if command -v curl >/dev/null 2>&1; then
    FETCH="curl -fsSL"
else
    FETCH="wget -qO-"
fi

INSTALL_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
mkdir -p "$INSTALL_DIR"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

URL="https://github.com/$REPO/releases/latest/download/${BIN}-${TARGET}.tar.gz"
echo "Downloading ${BIN}-${TARGET} …"
$FETCH "$URL" | tar -xz -C "$TMP_DIR"

install -m755 "$TMP_DIR/$BIN" "$INSTALL_DIR/$BIN"

echo
echo "Installed $BIN to $INSTALL_DIR/$BIN"
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "note: $INSTALL_DIR is not in your PATH." >&2
       echo "      add it with:  export PATH=\"\$HOME/.local/bin:\$PATH\"" >&2
       ;;
esac
echo "Run: $BIN"