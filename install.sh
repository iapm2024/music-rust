#!/bin/bash

# install.sh - Installs music-rust (Native Rust) on the system
# Author: iapizarro
# Licensed under the GNU General Public License v3

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PREFIX=""
UNINSTALL=0

for arg in "$@"; do
    case "$arg" in
        --prefix=*) PREFIX="${arg#*=}" ;;
        --uninstall) UNINSTALL=1 ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

if [ -z "$PREFIX" ]; then
    if [ "$EUID" -ne 0 ]; then
        PREFIX="$HOME/.local"
    else
        PREFIX="/usr/local"
    fi
fi

DEST_DIR="$PREFIX/share/music-rust"
BIN_DIR="$PREFIX/bin"

do_uninstall() {
    echo "🎵 Uninstalling music-rust by iapizarro from $PREFIX..."
    rm -f "$BIN_DIR/music-rust"
    rm -f "$HOME/.cargo/bin/music-rust" 2>/dev/null || true
    rm -rf "$DEST_DIR"
    echo "======================================================="
    echo " Uninstallation successful!"
    echo "======================================================="
    exit 0
}

if [ "$UNINSTALL" -eq 1 ]; then
    do_uninstall
fi

check_deps() {
    if ! command -v cargo &> /dev/null; then
        echo "Error: Cargo is required but not installed."
        echo "Please install Rust & Cargo (e.g. curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh)."
        exit 1
    fi

    if ! command -v pkg-config &> /dev/null; then
        echo "Note: pkg-config not found. If the build fails, install system dependencies:"
        echo "  Debian/Ubuntu: sudo apt install libasound2-dev libdbus-1-dev pkg-config"
        echo "  Fedora:        sudo dnf install alsa-lib-devel dbus-devel pkgconf-pkg-config"
        echo "  Arch Linux:    sudo pacman -S alsa-lib dbus pkgconf"
        echo "  openSUSE:      sudo zypper install alsa-devel dbus-1-devel pkg-config"
    fi
}

check_deps

echo "🎵 Building release binary with Cargo..."
(cd "$SCRIPT_DIR" && cargo build --release)

echo "Installing music-rust binary to $BIN_DIR..."
mkdir -p "$BIN_DIR"
cp "$SCRIPT_DIR/target/release/music-rust" "$BIN_DIR/music-rust.new"
chmod 755 "$BIN_DIR/music-rust.new"
mv -f "$BIN_DIR/music-rust.new" "$BIN_DIR/music-rust"

if [ -d "$HOME/.cargo/bin" ] && [ "$BIN_DIR" != "$HOME/.cargo/bin" ]; then
    install -m 755 "$SCRIPT_DIR/target/release/music-rust" "$HOME/.cargo/bin/music-rust" 2>/dev/null || true
fi

# Ensure any legacy desktop launcher shortcut is cleaned up
rm -f "$PREFIX/share/applications/music-rust.desktop"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
fi

echo "Cleaning build target cache to conserve disk space..."
(cd "$SCRIPT_DIR" && cargo clean)

echo "======================================================="
echo " Installation successful! music-rust v0.3.0 is ready."
if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    echo " Note: $BIN_DIR is not in your PATH. You may add it via:"
    echo "   export PATH=\"\$PATH:$BIN_DIR\""
fi
echo " Run 'music-rust' in your terminal."
echo "======================================================="
