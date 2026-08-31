#!/bin/sh
# Installs the prebuilt `prismpdf` CLI from GitHub Releases (macOS and Linux).
#
#   curl -fsSL https://raw.githubusercontent.com/theunloop/prism-pdf/main/scripts/install.sh | sh
#
# Environment:
#   PRISMPDF_VERSION      release to install, e.g. "0.4.1" (default: the latest release)
#   PRISMPDF_INSTALL_DIR  directory the binary is copied into (default: ~/.local/bin)
#
# The script picks the archive matching this machine, verifies it against the release's
# SHA256SUMS file, and copies the single `prismpdf` binary into place. The default prefix needs
# no root, and no shell profile is edited — if the directory is not on PATH, it says so and
# stops there. Windows users run scripts/install.ps1 instead.
#
# Platform selection mirrors the CLI matrix in .github/workflows/release.yml: on Linux, musl
# systems get the static linux-musl-* build, and so do glibc systems older than the 2.17 floor
# the glibc builds target (armv7 has no musl build, so it always gets the glibc one).

set -eu

REPO="theunloop/prism-pdf"
INSTALL_DIR="${PRISMPDF_INSTALL_DIR:-$HOME/.local/bin}"

say()  { printf '%s\n' "$*"; }
fail() { printf 'install.sh: error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

fetch() { # fetch <url> <outfile>
  if have curl; then
    curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"
  elif have wget; then
    wget -q -O "$2" "$1"
  else
    fail "neither curl nor wget is available"
  fi
}

# /releases/latest redirects to /releases/tag/v<x.y.z>; reading the redirect target avoids the
# GitHub API and its per-IP rate limit.
latest_version() {
  if have curl; then
    url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
      --proto '=https' --tlsv1.2 "https://github.com/$REPO/releases/latest")
  else
    url=$(wget -q -S --max-redirect=10 -O /dev/null "https://github.com/$REPO/releases/latest" 2>&1 \
      | sed -n 's/^ *[Ll]ocation: *//p' | tail -1)
  fi
  case "$url" in
    */releases/tag/v*) printf '%s\n' "${url##*/tag/v}" ;;
    *) fail "could not resolve the latest release (got: ${url:-nothing})" ;;
  esac
}

sha256_of() {
  if have sha256sum; then sha256sum "$1" | awk '{print $1}'
  elif have shasum; then shasum -a 256 "$1" | awk '{print $1}'
  elif have openssl; then openssl dgst -sha256 "$1" | awk '{print $NF}'
  else fail "no SHA-256 tool available (need sha256sum, shasum, or openssl)"
  fi
}

# --- Pick the release asset for this machine -------------------------------------------------

os=$(uname -s)
arch=$(uname -m)

case "$os" in
  Darwin)
    case "$arch" in
      x86_64) rid="osx-x64" ;;
      arm64)  rid="osx-arm64" ;;
      *) fail "unsupported macOS architecture: $arch" ;;
    esac
    ;;
  Linux)
    case "$arch" in
      x86_64 | amd64)   cpu="x64" ;;
      aarch64 | arm64)  cpu="arm64" ;;
      armv7l | armv7)   cpu="arm" ;;
      *) fail "unsupported Linux architecture: $arch" ;;
    esac
    # `ldd --version` names musl on musl systems and ends its first line with the glibc version
    # on glibc ones. Anything unparseable falls back to the static musl build, which runs on any
    # distribution — except on armv7, which only ships a glibc build.
    ldd_out=$(ldd --version 2>&1 || true)
    flavor="musl"
    case "$ldd_out" in
      *musl*) ;;
      *)
        glibc=$(printf '%s\n' "$ldd_out" | sed -n '1s/.* \([0-9][0-9]*\.[0-9][0-9]*\)$/\1/p')
        if [ -n "$glibc" ] && [ "$(printf '%s\n' "2.17" "$glibc" | sort -t. -k1,1n -k2,2n | head -1)" = "2.17" ]; then
          flavor="gnu"
        fi
        ;;
    esac
    if [ "$flavor" = "gnu" ] || [ "$cpu" = "arm" ]; then
      rid="linux-$cpu"
    else
      case "$cpu" in
        x64 | arm64) rid="linux-musl-$cpu" ;;
        *) fail "no static build for $arch and glibc '$ldd_out' is below the 2.17 floor" ;;
      esac
    fi
    ;;
  MINGW* | MSYS* | CYGWIN* | Windows_NT)
    fail "this is a Windows shell — run scripts/install.ps1 instead:
  powershell -c \"irm https://raw.githubusercontent.com/$REPO/main/scripts/install.ps1 | iex\""
    ;;
  *)
    fail "unsupported operating system: $os"
    ;;
esac

version="${PRISMPDF_VERSION:-$(latest_version)}"
version="${version#v}"
archive="prismpdf-v${version}-${rid}.tar.gz"
sums="SHA256SUMS-v${version}.txt"
base="https://github.com/$REPO/releases/download/v${version}"

# --- Download, verify, install ---------------------------------------------------------------

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading $archive (v$version, $rid)..."
fetch "$base/$archive" "$tmp/$archive"
fetch "$base/$sums" "$tmp/$sums"

expected=$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$tmp/$sums")
[ -n "$expected" ] || fail "$sums has no entry for $archive"
actual=$(sha256_of "$tmp/$archive")
[ "$actual" = "$expected" ] || fail "checksum mismatch for $archive
  expected: $expected
  actual:   $actual"
say "Checksum verified."

tar xzf "$tmp/$archive" -C "$tmp"
bin="$tmp/prismpdf-v${version}-${rid}/prismpdf"
[ -f "$bin" ] || fail "archive did not contain the expected binary"

mkdir -p "$INSTALL_DIR"
cp "$bin" "$INSTALL_DIR/prismpdf"
chmod 755 "$INSTALL_DIR/prismpdf"
# curl downloads carry no quarantine attribute, but clear it if something added one.
if [ "$os" = "Darwin" ] && have xattr; then
  xattr -d com.apple.quarantine "$INSTALL_DIR/prismpdf" 2>/dev/null || true
fi

say "Installed prismpdf v$version to $INSTALL_DIR/prismpdf"
if reported=$("$INSTALL_DIR/prismpdf" --version 2>/dev/null); then
  say "  $reported"
fi

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    say ""
    say "Note: $INSTALL_DIR is not on your PATH. Add it in your shell profile, e.g.:"
    say "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
