#!/bin/bash
set -e

# proj installer
# Usage: curl -fsSL https://raw.githubusercontent.com/your-username/proj/main/install.sh | bash

REPO="your-username/proj"  # Update this when publishing
INSTALL_DIR="${PROJ_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${PROJ_VERSION:-latest}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

info() {
    echo -e "${BLUE}▶${NC} $1"
}

success() {
    echo -e "${GREEN}✓${NC} $1"
}

error() {
    echo -e "${RED}✗${NC} $1" >&2
    exit 1
}

# Detect OS and architecture
detect_platform() {
    OS=$(uname -s | tr '[:upper:]' '[:lower:]')
    ARCH=$(uname -m)

    case "$OS" in
        darwin) OS="apple-darwin" ;;
        linux) OS="unknown-linux-gnu" ;;
        *) error "Unsupported OS: $OS" ;;
    esac

    case "$ARCH" in
        x86_64) ARCH="x86_64" ;;
        arm64|aarch64) ARCH="aarch64" ;;
        *) error "Unsupported architecture: $ARCH" ;;
    esac

    PLATFORM="${ARCH}-${OS}"
    echo "$PLATFORM"
}

# Check for required tools
check_requirements() {
    if ! command -v curl &> /dev/null && ! command -v wget &> /dev/null; then
        error "curl or wget is required but not installed"
    fi

    if ! command -v tar &> /dev/null; then
        error "tar is required but not installed"
    fi
}

# Download file
download() {
    local url="$1"
    local output="$2"

    if command -v curl &> /dev/null; then
        curl -fsSL "$url" -o "$output"
    else
        wget -q "$url" -O "$output"
    fi
}

# Get latest release version
get_latest_version() {
    if command -v curl &> /dev/null; then
        curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"v([^"]+)".*/\1/'
    else
        wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"v([^"]+)".*/\1/'
    fi
}

main() {
    echo ""
    echo "  proj installer"
    echo "  Project-scoped developer environment manager"
    echo ""

    check_requirements

    PLATFORM=$(detect_platform)
    info "Detected platform: $PLATFORM"

    # Create install directory
    mkdir -p "$INSTALL_DIR"

    if [ "$VERSION" = "latest" ]; then
        info "Fetching latest version..."
        VERSION=$(get_latest_version)
        if [ -z "$VERSION" ]; then
            error "Could not determine latest version"
        fi
    fi

    info "Installing proj v$VERSION"

    # Download URL
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/proj-${PLATFORM}.tar.gz"
    TEMP_DIR=$(mktemp -d)
    ARCHIVE="$TEMP_DIR/proj.tar.gz"

    info "Downloading from $DOWNLOAD_URL"
    download "$DOWNLOAD_URL" "$ARCHIVE" || error "Download failed"

    info "Extracting..."
    tar -xzf "$ARCHIVE" -C "$TEMP_DIR"

    info "Installing to $INSTALL_DIR"
    mv "$TEMP_DIR/proj" "$INSTALL_DIR/"
    mv "$TEMP_DIR/proj-daemon" "$INSTALL_DIR/"
    chmod +x "$INSTALL_DIR/proj" "$INSTALL_DIR/proj-daemon"

    # Cleanup
    rm -rf "$TEMP_DIR"

    success "Installed proj v$VERSION"
    echo ""

    # Check if install dir is in PATH
    if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
        echo "Add this to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
        echo ""
        echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
        echo ""
    fi

    echo "Get started:"
    echo ""
    echo "  proj new my-app            # Create a project"
    echo "  proj my-app run npm dev    # Run dev server"
    echo "  proj my-app open           # Open in isolated browser"
    echo ""
}

# Alternative: install from source
install_from_source() {
    echo ""
    echo "  proj - Install from source"
    echo ""

    if ! command -v cargo &> /dev/null; then
        error "cargo is required. Install Rust from https://rustup.rs"
    fi

    info "Building proj..."
    cargo build --release

    info "Installing binaries..."
    mkdir -p "$INSTALL_DIR"
    cp target/release/proj "$INSTALL_DIR/"
    cp target/release/proj-daemon "$INSTALL_DIR/"

    success "Installed proj to $INSTALL_DIR"
}

# Check for --source flag
if [ "$1" = "--source" ]; then
    install_from_source
else
    main
fi
