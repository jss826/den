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

# Fetch releases (include pre-releases)
printf "Fetching releases...\n"
RELEASES_JSON=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases?per_page=10")

# Parse tag names and pre-release flags
TAGS=$(printf '%s' "$RELEASES_JSON" | grep '"tag_name"' | cut -d'"' -f4)
PRERELEASE=$(printf '%s' "$RELEASES_JSON" | grep '"prerelease"' | head -10 | sed 's/.*: //;s/,//')

if [ -z "$TAGS" ]; then
  printf "Failed to fetch releases.\n" >&2
  exit 1
fi

# Display version menu
printf "\n"
i=0
printf '%s\n' "$TAGS" | while IFS= read -r tag; do
  flag=$(printf '%s\n' "$PRERELEASE" | sed -n "$((i+1))p")
  label="$tag"
  if [ "$flag" = "true" ]; then label="$label (pre-release)"; fi
  if [ "$i" -eq 0 ]; then label="$label *"; fi
  printf "  [%d] %s\n" "$i" "$label"
  i=$((i+1))
done

printf "\n"
printf "Select version [0]: "
if [ -t 0 ]; then
  read -r CHOICE
else
  CHOICE=""
  printf "(auto: 0)\n"
fi
CHOICE=${CHOICE:-0}

TAG=$(printf '%s\n' "$TAGS" | sed -n "$((CHOICE + 1))p")

if [ -z "$TAG" ]; then
  printf "Invalid selection: %s\n" "$CHOICE" >&2
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

printf "Installed den %s to %s/den\n" "$TAG" "$INSTALL_DIR"

# PATH check
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    printf "\nNote: %s is not in your PATH.\n" "$INSTALL_DIR"
    printf "Add it with:\n"
    printf "  export PATH=\"%s:\$PATH\"\n" "$INSTALL_DIR"
    ;;
esac
