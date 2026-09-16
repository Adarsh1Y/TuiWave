#!/usr/bin/env sh
# Build the release artifacts for lastwave and emit SHA256SUMS for them.
#
# Artifacts are named lastwave-<target>.tar.gz (a single stripped binary) to
# match what install.sh downloads from GitHub Releases.
#
# Usage:
#   scripts/release.sh                 build for the current host
#   scripts/release.sh x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
#   PKGBUILD_SOURCE_SHA256=<sha> scripts/release.sh   # also stamp PKGBUILD
set -eu

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

PKGVER="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[ -n "$PKGVER" ] || { echo "error: cannot read version from Cargo.toml" >&2; exit 1; }

if [ "$#" -gt 0 ]; then
    TARGETS="$*"
else
    TARGETS="$(rustc -vV | sed -n 's/^host: //p')"
fi

OUT="${OUT_DIR:-dist}"
mkdir -p "$OUT"

echo "==> cargo build --release (locked)"
cargo build --release --locked

for target in $TARGETS; do
    if [ "$target" = "$(rustc -vV | sed -n 's/^host: //p')" ]; then
        BIN_DIR="target/release"
    else
        echo "==> cross-building $target"
        cargo build --release --locked --target "$target"
        BIN_DIR="target/$target/release"
    fi
    BIN="$BIN_DIR/lastwave"
    [ -x "$BIN" ] || { echo "error: $BIN missing" >&2; exit 1; }
    strip "$BIN"
    tar -czf "$OUT/lastwave-$target.tar.gz" --transform="s,^.*$,lastwave," -C "$REPO_ROOT" "$BIN"
done

echo "==> written SHA256SUMS"
(cd "$OUT" && sha256sum lastwave-*.tar.gz | tee "$REPO_ROOT/SHA256SUMS")

if [ -n "${PKGBUILD_SOURCE_SHA256:-}" ]; then
    sed -i "s/^sha256sums=('SKIP')/sha256sums=('$PKGBUILD_SOURCE_SHA256')/" packaging/PKGBUILD
    echo "==> packaging/PKGBUILD sha256sums updated"
fi

echo "==> upload these files to a GitHub Release (tag v$PKGVER):"
ls -l "$OUT" && echo "artifacts kept in $OUT/"