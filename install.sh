#!/bin/sh
# One-line installer for cc-statusline (macOS / Linux):
#
#   curl -fsSL https://raw.githubusercontent.com/Demogorgon314/cc-statusline/main/install.sh | sh
#
# Downloads the prebuilt binary from GitHub releases into ~/.local/bin (or
# $CC_STATUSLINE_BIN_DIR) and sets it as Claude Code's status line command.
# Pin a version with CC_STATUSLINE_VERSION=v0.1.0.
set -eu

repo="Demogorgon314/cc-statusline"
bin_dir="${CC_STATUSLINE_BIN_DIR:-$HOME/.local/bin}"
version="${CC_STATUSLINE_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

case "$(uname -m)" in
  arm64 | aarch64) arch=arm64 ;;
  x86_64 | amd64) arch=x64 ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac
case "$(uname -s)" in
  Darwin) os=darwin ;;
  Linux) os=linux ;;
  *) die "unsupported OS: $(uname -s) (on Windows use install.ps1)" ;;
esac

asset="cc-statusline-$os-$arch.tar.gz"
if [ "$version" = latest ]; then
  url="https://github.com/$repo/releases/latest/download/$asset"
else
  url="https://github.com/$repo/releases/download/$version/$asset"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
say "Downloading $url"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL -o "$tmp/$asset" "$url" || die "download failed"
elif command -v wget >/dev/null 2>&1; then
  wget -q -O "$tmp/$asset" "$url" || die "download failed"
else
  die "need curl or wget"
fi
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$bin_dir"
mv -f "$tmp/cc-statusline" "$bin_dir/cc-statusline"
chmod +x "$bin_dir/cc-statusline"
say "Installed $bin_dir/cc-statusline ($("$bin_dir/cc-statusline" --version))"

if "$bin_dir/cc-statusline" install; then
  :
else
  say "Claude Code already has another status line command; to replace it run:"
  say "  $bin_dir/cc-statusline install --force"
fi

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) say "Note: $bin_dir is not on PATH; add it to run 'cc-statusline config' directly." ;;
esac
say "Restart Claude Code to see it. Configure with: cc-statusline config"
