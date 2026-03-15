#!/bin/bash
set -euo pipefail

REPO="allisonhere/anote"
BINARY="anote"
INSTALL_DIR="${ANOTE_INSTALL_DIR:-}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

info()    { echo -e "  ${CYAN}→${NC} $1"; }
success() { echo -e "  ${GREEN}✓${NC} $1"; }
warn()    { echo -e "  ${YELLOW}⚠${NC} $1"; }
die()     { echo -e "  ${RED}✗${NC} $1" >&2; exit 1; }

echo ""
echo -e "${BOLD}  anote installer${NC}"
echo -e "  ${DIM}https://github.com/${REPO}${NC}"
echo ""

# ── Detect OS and arch ────────────────────────────────────────────────────────

OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$OS" in
  linux)  ;;
  darwin) ;;
  *)      die "Unsupported OS: $OS (supported: linux, darwin)" ;;
esac

case "$ARCH" in
  x86_64)         ARCH_NAME="x86_64" ;;
  aarch64|arm64)  ARCH_NAME="aarch64" ;;
  *)              die "Unsupported architecture: $ARCH (supported: x86_64, aarch64/arm64)" ;;
esac

# Darwin aarch64 uses arm64 in the binary name
if [[ "$OS" == "darwin" && "$ARCH_NAME" == "aarch64" ]]; then
  ARCH_NAME="arm64"
fi

ASSET_NAME="${BINARY}-${OS}-${ARCH_NAME}.tar.gz"
info "Detected: $OS / $ARCH_NAME"

# ── Resolve install directory ─────────────────────────────────────────────────

if [ -z "$INSTALL_DIR" ]; then
  if [ -d "$HOME/.local/bin" ] && echo "$PATH" | grep -q "$HOME/.local/bin"; then
    INSTALL_DIR="$HOME/.local/bin"
  elif [ -w "/usr/local/bin" ]; then
    INSTALL_DIR="/usr/local/bin"
  else
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
    warn "\$HOME/.local/bin is not in your PATH. Add this to your shell rc:"
    echo ""
    echo -e "    ${DIM}export PATH=\"\$HOME/.local/bin:\$PATH\"${NC}"
    echo ""
  fi
fi

mkdir -p "$INSTALL_DIR"
info "Install directory: $INSTALL_DIR"

# ── Check dependencies ────────────────────────────────────────────────────────

for cmd in curl tar; do
  command -v "$cmd" &>/dev/null || die "Required tool not found: $cmd"
done

# ── Fetch latest release ──────────────────────────────────────────────────────

info "Fetching latest release..."

LATEST_URL="https://api.github.com/repos/${REPO}/releases/latest"

if command -v curl &>/dev/null; then
  RELEASE_JSON=$(curl -fsSL "$LATEST_URL")
else
  die "curl is required"
fi

# Extract version tag
VERSION=$(echo "$RELEASE_JSON" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
[ -n "$VERSION" ] || die "Could not determine latest version"
info "Latest version: $VERSION"

# Extract download URL for our asset
DOWNLOAD_URL=$(echo "$RELEASE_JSON" | grep "browser_download_url" | grep "$ASSET_NAME" | head -1 | sed 's/.*"browser_download_url": *"\([^"]*\)".*/\1/')
[ -n "$DOWNLOAD_URL" ] || die "No release asset found for $ASSET_NAME — check https://github.com/${REPO}/releases"

# ── Download and install ──────────────────────────────────────────────────────

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

ARCHIVE="$TMP_DIR/$ASSET_NAME"
info "Downloading $ASSET_NAME..."
curl -fsSL --progress-bar "$DOWNLOAD_URL" -o "$ARCHIVE"

info "Extracting..."
tar -xzf "$ARCHIVE" -C "$TMP_DIR"

EXTRACTED="$TMP_DIR/$BINARY"
[ -f "$EXTRACTED" ] || die "Binary not found in archive"
chmod +x "$EXTRACTED"

DEST="$INSTALL_DIR/$BINARY"

# Backup existing installation if present
if [ -f "$DEST" ]; then
  EXISTING_VER=$("$DEST" --version 2>/dev/null | head -1 || echo "unknown")
  warn "Replacing existing install ($EXISTING_VER)"
fi

mv "$EXTRACTED" "$DEST"
success "Installed $BINARY $VERSION → $DEST"

# ── Verify ────────────────────────────────────────────────────────────────────

if command -v "$BINARY" &>/dev/null; then
  echo ""
  echo -e "  ${BOLD}${GREEN}Ready!${NC} Run ${CYAN}anote${NC} to start."
else
  echo ""
  warn "Make sure $INSTALL_DIR is in your PATH, then run ${CYAN}anote${NC}."
fi

echo ""
