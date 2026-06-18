#!/usr/bin/env sh
set -eu
version="${1:-0.1.0}"
os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$os/$arch" in
  darwin/arm64) target="darwin-aarch64" ;;
  linux/x86_64) target="linux-x86_64" ;;
  *) echo "unsupported platform: $os/$arch" >&2; exit 1 ;;
esac
url="https://github.com/radjathaher/captions-cli/releases/download/v${version}/captions-cli-${version}-${target}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$url" | tar -xz -C "$tmp"
install -m 0755 "$tmp/captions" "${PREFIX:-$HOME/.local}/bin/captions"
