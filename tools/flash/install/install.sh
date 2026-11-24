#!/bin/sh
# ferros-flash installer for Linux and macOS, the same shape as Photon's install-release.sh.
# It downloads the tool into a private directory, checks its SHA-256 against the one written into this script at publish time, and installs it to ~/.local/bin.
# The tool checks everything else it fetches by BLAKE3 against a release signed with the ferros key; SHA-256 here only because every system has a SHA-256 tool and few have b3sum.
# Run: curl -sSfL __BASE__/install.sh | sh
set -e

BASE="__BASE__"
SHA256_linux_x86_64="__SHA256_LINUX_X86_64__"
SHA256_macos_arm64="__SHA256_MACOS_ARM64__"

OS=$(uname -s)
ARCH=$(uname -m)
case "$OS-$ARCH" in
    Linux-x86_64) PLATFORM=linux_x86_64 ;;
    Darwin-arm64 | Darwin-aarch64) PLATFORM=macos_arm64 ;;
    *) echo "ferros-flash is not built for $OS on $ARCH yet."; exit 1 ;;
esac
EXPECTED=$(eval "printf '%s' \"\$SHA256_$PLATFORM\"")
case "$EXPECTED" in
    [0-9a-f]*) [ ${#EXPECTED} -eq 64 ] || EXPECTED="" ;;
    *) EXPECTED="" ;;
esac
[ -n "$EXPECTED" ] || { echo "This copy of the installer carries no hash for $PLATFORM: nothing was installed."; exit 1; }

TMP=$(mktemp -d "${TMPDIR:-/tmp}/ferros-flash.XXXXXXXX")
chmod 700 "$TMP"
trap 'rm -rf "$TMP"' EXIT
NAME=$(printf '%s' "$PLATFORM" | tr '_' '-')
echo "Downloading ferros-flash..."
if command -v curl >/dev/null 2>&1; then curl -sSfL "$BASE/ferros-flash-$NAME" -o "$TMP/ferros-flash"
else wget -q "$BASE/ferros-flash-$NAME" -O "$TMP/ferros-flash"; fi

if command -v sha256sum >/dev/null 2>&1; then ACTUAL=$(sha256sum "$TMP/ferros-flash" | cut -d' ' -f1)
else ACTUAL=$(shasum -a 256 "$TMP/ferros-flash" | cut -d' ' -f1); fi
if [ "$ACTUAL" != "$EXPECTED" ]; then
    echo "The download is not the published ferros-flash."
    echo "  expected $EXPECTED"
    echo "  got      $ACTUAL"
    exit 1
fi
echo "The download is the published ferros-flash."

mkdir -p "$HOME/.local/bin"
chmod +x "$TMP/ferros-flash"
[ "$OS" = Darwin ] && xattr -c "$TMP/ferros-flash" 2>/dev/null || true
mv "$TMP/ferros-flash" "$HOME/.local/bin/ferros-flash"
echo "Installed to $HOME/.local/bin/ferros-flash"
echo "Every run keeps a log (Linux: ~/.local/share/ferros-flash/runs, macOS: ~/Library/Application Support/ferros-flash/runs); please send it to us whatever happens."
if [ "$OS" = Linux ]; then
    echo "On Linux, reaching the phone's bootloader without root needs a udev rule for USB vendor 18d1 (most distributions' android-udev package has it)."
fi
echo "Run: ferros-flash guide"
