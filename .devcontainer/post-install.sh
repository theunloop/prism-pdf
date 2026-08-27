#!/usr/bin/env bash
set -euo pipefail

echo "==> Setting up Rust toolchain components"
rustup component add clippy rustfmt
rustc --version
cargo --version

echo "==> Installing Claude Code"
curl -fsSL https://claude.ai/install.sh | bash

curl -fsSL https://chatgpt.com/codex/install.sh | sh

# Make sure the install location is on PATH for future shells
if ! grep -q '.local/bin' "$HOME/.bashrc" 2>/dev/null; then
  echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.bashrc"
fi

export PATH="$HOME/.local/bin:$PATH"
claude --version || echo "Claude Code installed; restart your shell to pick it up."

echo "==> Installing veraPDF (PDF/A reference validator — oracle for the conformance harness)"
# veraPDF is the industry reference PDF/A/PDF/UA validator. It's the ground truth the
# `conformance_corpus` test (crates/pdf/tests/) grades against — and what CLAUDE.md earmarks for
# PDF/A conformance in CI. Best-effort: a network/offline failure here must NOT break the container
# (the Rust harness still runs without it), so the whole block is guarded.
install_verapdf() {
  if command -v verapdf >/dev/null 2>&1; then
    echo "    veraPDF already installed: $(verapdf --version 2>/dev/null | head -1)"
    return 0
  fi

  # Toolchain it needs: a JRE (the base image ships one) plus unzip. Install only what's missing.
  local pkgs=()
  command -v java  >/dev/null 2>&1 || pkgs+=(default-jre-headless)
  command -v unzip >/dev/null 2>&1 || pkgs+=(unzip)
  if [ "${#pkgs[@]}" -gt 0 ]; then
    sudo apt-get update -qq && sudo apt-get install -y --no-install-recommends "${pkgs[@]}"
  fi

  # Stable channel always resolves to the latest release (1.30.2 at time of writing). To pin a
  # version instead, swap this for https://software.verapdf.org/releases/<ver>/verapdf-installer.zip
  local url="https://software.verapdf.org/releases/verapdf-installer.zip"
  local dest="$HOME/.local/opt/verapdf"
  local tmp
  tmp="$(mktemp -d)"

  echo "    downloading $url"
  curl -fsSL -o "$tmp/installer.zip" "$url" || { echo "    download failed"; return 1; }
  unzip -q "$tmp/installer.zip" -d "$tmp" || return 1

  # The archive extracts to verapdf-greenfield-<version>/ holding an IzPack installer + launcher.
  local srcdir
  srcdir="$(find "$tmp" -maxdepth 1 -type d -name 'verapdf-greenfield-*' | head -1)"
  [ -n "$srcdir" ] || { echo "    unexpected archive layout"; return 1; }

  # Headless IzPack auto-install: CLI + GUI + validation model (docs/plugins skipped) into $dest.
  cat > "$tmp/auto-install.xml" <<XML
<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<AutomatedInstallation langpack="eng">
    <com.izforge.izpack.panels.htmlhello.HTMLHelloPanel id="welcome"/>
    <com.izforge.izpack.panels.target.TargetPanel id="install_dir">
        <installpath>$dest</installpath>
    </com.izforge.izpack.panels.target.TargetPanel>
    <com.izforge.izpack.panels.packs.PacksPanel id="sdk_pack_select">
        <pack index="0" name="veraPDF Mac and *nix Scripts" selected="true"/>
        <pack index="1" name="veraPDF GUI" selected="true"/>
        <pack index="2" name="veraPDF Validation model" selected="true"/>
        <pack index="3" name="veraPDF Documentation" selected="false"/>
        <pack index="4" name="veraPDF Sample Plugins" selected="false"/>
    </com.izforge.izpack.panels.packs.PacksPanel>
    <com.izforge.izpack.panels.install.InstallPanel id="install"/>
    <com.izforge.izpack.panels.finish.SimpleFinishPanel id="finish"/>
</AutomatedInstallation>
XML

  rm -rf "$dest"
  ( cd "$srcdir" && sh ./verapdf-install "$tmp/auto-install.xml" ) || return 1

  # The launcher resolves its own symlinks, so a thin link onto PATH is enough.
  mkdir -p "$HOME/.local/bin"
  ln -sf "$dest/verapdf" "$HOME/.local/bin/verapdf"
  rm -rf "$tmp"
  echo "    installed: $(verapdf --version 2>/dev/null | head -1)"
}

install_verapdf || echo "WARNING: veraPDF not installed (offline?); the Rust conformance harness still runs without it."
