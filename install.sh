#!/bin/sh
set -eu

REPO="jss826/den"
INSTALL_DIR="${DEN_INSTALL_DIR:-$HOME/.local/bin}"

# Detect platform
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
  Linux)  target_os="unknown-linux-gnu" ;;
  Darwin) target_os="apple-darwin" ;;
  *)      printf "Unsupported OS: %s\n" "$OS" >&2; exit 1 ;;
esac

case "$ARCH" in
  x86_64|amd64)  target_arch="x86_64" ;;
  aarch64|arm64) target_arch="aarch64" ;;
  *)             printf "Unsupported architecture: %s\n" "$ARCH" >&2; exit 1 ;;
esac

TARGET="${target_arch}-${target_os}"

# Fetch latest release tag
printf "Fetching latest release...\n"
TAG=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep '"tag_name"' | head -1 | cut -d'"' -f4)

if [ -z "$TAG" ]; then
  printf "Failed to fetch latest release.\n" >&2
  exit 1
fi

ASSET="den-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"

printf "Installing den %s (%s)...\n" "$TAG" "$TARGET"

# Download and extract
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "$URL" -o "${TMPDIR}/${ASSET}"
tar xzf "${TMPDIR}/${ASSET}" -C "$TMPDIR"

# Install
mkdir -p "$INSTALL_DIR"
mv "${TMPDIR}/den" "${INSTALL_DIR}/den"
chmod +x "${INSTALL_DIR}/den"

printf "Installed den to %s/den\n" "$INSTALL_DIR"

# PATH check
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    printf "\nNote: %s is not in your PATH.\n" "$INSTALL_DIR"
    printf "Add it with:\n"
    printf "  export PATH=\"%s:\$PATH\"\n" "$INSTALL_DIR"
    ;;
esac
