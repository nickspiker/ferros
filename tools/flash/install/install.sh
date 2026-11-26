#!/bin/sh
# ferros-flash installer for Linux and macOS, the same shape as Photon's install-release.sh.
# It downloads the two executables (the installer window, ferros-flash-gui, and the command line tool, ferros-flash) into a private directory, checks each SHA-256 against the one written into this script at publish time, installs them to ~/.local/bin and opens the window.
# On macOS the executables are not signed or notarized, so Gatekeeper warns; the quarantine flag is cleared here, and the rest is the person's to allow (docs/FLASHER.md "The GUI").
# The tool checks everything else it fetches by BLAKE3 against a release signed with the ferros key; SHA-256 here only because every system has a SHA-256 tool and few have b3sum.
# Run: curl -sSfL __BASE__/install.sh | sh
set -e

BASE="__BASE__"
SHA256_linux_x86_64="__SHA256_LINUX_X86_64__"
SHA256_macos_arm64="__SHA256_MACOS_ARM64__"
SHA256_GUI_linux_x86_64="__SHA256_GUI_LINUX_X86_64__"
SHA256_GUI_macos_arm64="__SHA256_GUI_MACOS_ARM64__"

OS=$(uname -s)
ARCH=$(uname -m)
case "$OS-$ARCH" in
    Linux-x86_64) PLATFORM=linux_x86_64 ;;
    Darwin-arm64 | Darwin-aarch64) PLATFORM=macos_arm64 ;;
    *) echo "ferros-flash is not built for $OS on $ARCH yet."; exit 1 ;;
esac
# A published hash is 64 hex digits; anything else means this copy was never published for that platform.
hash_for() {
    h=$(eval "printf '%s' \"\$$1_$PLATFORM\"")
    case "$h" in
        [0-9a-f]*) [ ${#h} -eq 64 ] && printf '%s' "$h" ;;
    esac
}
EXPECTED=$(hash_for SHA256)
EXPECTED_GUI=$(hash_for SHA256_GUI)
[ -n "$EXPECTED" ] && [ -n "$EXPECTED_GUI" ] || { echo "This copy of the installer carries no hash for $PLATFORM: nothing was installed."; exit 1; }

TMP=$(mktemp -d "${TMPDIR:-/tmp}/ferros-flash.XXXXXXXX")
chmod 700 "$TMP"
trap 'rm -rf "$TMP"' EXIT
NAME=$(printf '%s' "$PLATFORM" | tr '_' '-')
# One executable: fetched, checked against its published hash, made runnable.
fetch() {
    echo "Downloading $1..."
    if command -v curl >/dev/null 2>&1; then curl -sSfL "$BASE/$1-$NAME" -o "$TMP/$1"
    else wget -q "$BASE/$1-$NAME" -O "$TMP/$1"; fi
    if command -v sha256sum >/dev/null 2>&1; then ACTUAL=$(sha256sum "$TMP/$1" | cut -d' ' -f1)
    else ACTUAL=$(shasum -a 256 "$TMP/$1" | cut -d' ' -f1); fi
    if [ "$ACTUAL" != "$2" ]; then
        echo "The download is not the published $1."
        echo "  expected $2"
        echo "  got      $ACTUAL"
        exit 1
    fi
    echo "The download is the published $1."
    chmod +x "$TMP/$1"
    [ "$OS" = Darwin ] && xattr -c "$TMP/$1" 2>/dev/null || true
}
fetch ferros-flash-gui "$EXPECTED_GUI"
fetch ferros-flash "$EXPECTED"

mkdir -p "$HOME/.local/bin"
mv "$TMP/ferros-flash-gui" "$HOME/.local/bin/ferros-flash-gui"
mv "$TMP/ferros-flash" "$HOME/.local/bin/ferros-flash"
echo "Installed to $HOME/.local/bin: ferros-flash-gui (the installer window) and ferros-flash (the same on the command line)."
echo "Every run keeps a log (Linux: ~/.local/share/ferros-flash/runs, macOS: ~/Library/Application Support/ferros-flash/runs); please send it to us whatever happens."
if [ "$OS" = Linux ]; then
    echo "On Linux, reaching the device's bootloader without root needs a udev rule for USB vendor 18d1 (most distributions' android-udev package has it)."
fi
if [ "$OS" = Darwin ]; then
    echo "On macOS these executables are not signed or notarized, so Gatekeeper may warn; allow them in System Settings, Privacy & Security, if it does."
fi
echo "Opening the installer window (later: ferros-flash-gui, or ferros-flash guide in a terminal)."
"$HOME/.local/bin/ferros-flash-gui" &
