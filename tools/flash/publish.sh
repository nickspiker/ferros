#!/usr/bin/env bash
# Stage a ferros-flash release (docs/FLASHER.md "Distribution"): the executables (the CLI and the GUI, tools/flash-gui, for Windows x86_64, Linux x86_64 and macOS arm64), the board manifests, the mark (logo.vsf, role `logo`: the GUI draws it from the release) and the boot images, each under its BLAKE3, a release document signed with the ferros key naming them all, and the two one-line installers with the executables' SHA-256 written in.
# macOS is cross-built with osxcross (/mnt/Harbor/Code/osxcross, as Photon's deploy.sh does); the binaries are unsigned and unnotarized, so Gatekeeper warns, and install.sh clears the quarantine flag it can.
# The host (owner, 2026-11-24: "R2"): the R2 bucket `ferros`, served at https://get.ferros.org (its custom domain), as Photon's bucket is at brobdingnagian.holdmyoscilloscope.com. FERROS_FLASH_BASE is the URL the installers and the tool fetch from (default that one); `--upload` puts the staged folder in the bucket (FERROS_FLASH_BUCKET, default `ferros`) with wrangler, under the R2 token in keys/cloudflare-r2-token.
# Files named by their BLAKE3 never change, so they are cached for good; the installers and the fixed-name executables change with every publish, so they go up with `no-cache` (an edge holding an old executable beside a new installer would fail every install's hash check).
# Boot images: FERROS_FLASH_BOOT_<BOARD>=<whole signed boot partition image> adds one; the owner's rule (2026-11-24) is that only production builds are hosted, with every Android path compiled out, so the channel is `production` only when every image given is one, else `development`.
# FERROS_FLASH_COMMIT names the commit when building outside a checkout (a scratch export).
# Usage: [FERROS_FLASH_CHANNEL=development] [FERROS_FLASH_BOOT_AKITA=img] tools/flash/publish.sh [--upload]
set -euo pipefail
cd "$(dirname "$0")/../.."
BASE=${FERROS_FLASH_BASE:-https://get.ferros.org}
BUCKET=${FERROS_FLASH_BUCKET:-ferros}
UPLOAD=0
for a in "$@"; do case $a in --upload) UPLOAD=1 ;; *) echo "unknown argument $a"; exit 2 ;; esac; done
CHANNEL=${FERROS_FLASH_CHANNEL:-development}
KEY=${FERROS_KEY:-/mnt/Harbor/Code/keys/ferros.key.vsf}
OUT=${OUT:-target/flash-publish}
T=${CARGO_TARGET_DIR:-target}
rm -rf "$OUT" && mkdir -p "$OUT"

# Google's USB driver package (tools/flash/src/driver.rs) must still be the file the tool pins: the tool fetches it from Google, never from us, so a changed file at Google's URL stops the publish here rather than every Windows install later.
DRV_URL=$(sed -n 's/^pub const URL: &str = "\(.*\)";/\1/p' tools/flash/src/driver.rs)
DRV_SHA=$(sed -n 's/^pub const SHA256: &str = "\(.*\)";/\1/p' tools/flash/src/driver.rs)
[ "$(curl -sSfL "$DRV_URL" | sha256sum | cut -c1-64)" = "$DRV_SHA" ] || { echo "Google's USB driver at $DRV_URL is no longer the pinned file (driver.rs): check Google's repository manifest (addon2-*.xml) and pin the new one"; exit 1; }
# The macOS cross environment (Photon's scripts/release/deploy.sh recipe): osxcross's clang wrapper as the compiler and linker, the SDK named outright since there is no xcrun here.
OSX=/mnt/Harbor/Code/osxcross/target
MACOS_SDK="$(ls -d $OSX/SDK/MacOSX*.sdk | sort -V | tail -1)"
[ -d "$MACOS_SDK" ] || { echo "no macOS SDK under $OSX/SDK"; exit 1; }
mac() {
    SDKROOT="$MACOS_SDK" CC_aarch64_apple_darwin=$OSX/bin/aarch64-apple-darwin-clang-wrapper CXX_aarch64_apple_darwin=$OSX/bin/aarch64-apple-darwin-clang-wrapper \
        CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=$OSX/bin/aarch64-apple-darwin-clang-wrapper OSXCROSS_TRIPLE=aarch64-apple-darwin "$@"
}
cargo build -q --release -p ferros-flash --target x86_64-pc-windows-gnu
cargo build -q --release -p ferros-flash --target x86_64-unknown-linux-gnu
mac cargo build -q --release -p ferros-flash --target aarch64-apple-darwin
cargo build -q -p ferros-flash --features publish
# The GUI is its own crate (fluor's host build stays out of the workspace lock), built into the same target directory: named outright, since a crate with its own [workspace] otherwise builds into its own target/ (2026-11-28: the publish copied from the workspace's and found nothing).
GUI=tools/flash-gui/Cargo.toml
cargo build -q --release --manifest-path $GUI --target-dir "$T" --target x86_64-pc-windows-gnu
cargo build -q --release --manifest-path $GUI --target-dir "$T" --target x86_64-unknown-linux-gnu
mac cargo build -q --release --manifest-path $GUI --target-dir "$T" --target aarch64-apple-darwin
FF=$T/debug/ferros-flash
cp "$T/x86_64-pc-windows-gnu/release/ferros-flash.exe" "$OUT/ferros-flash-windows-x86_64.exe"
cp "$T/x86_64-unknown-linux-gnu/release/ferros-flash" "$OUT/ferros-flash-linux-x86_64"
cp "$T/aarch64-apple-darwin/release/ferros-flash" "$OUT/ferros-flash-macos-arm64"
cp "$T/x86_64-pc-windows-gnu/release/ferros-flash-gui.exe" "$OUT/ferros-flash-gui-windows-x86_64.exe"
cp "$T/x86_64-unknown-linux-gnu/release/ferros-flash-gui" "$OUT/ferros-flash-gui-linux-x86_64"
cp "$T/aarch64-apple-darwin/release/ferros-flash-gui" "$OUT/ferros-flash-gui-macos-arm64"
$FF board akita "$OUT/akita.board.vsf"
$FF board husky "$OUT/husky.board.vsf"

FILES=(--file tool - windows-x86_64 ferros-flash.exe "$OUT/ferros-flash-windows-x86_64.exe"
       --file tool - linux-x86_64 ferros-flash "$OUT/ferros-flash-linux-x86_64"
       --file tool - macos-arm64 ferros-flash "$OUT/ferros-flash-macos-arm64"
       --file tool - windows-x86_64 ferros-flash-gui.exe "$OUT/ferros-flash-gui-windows-x86_64.exe"
       --file tool - linux-x86_64 ferros-flash-gui "$OUT/ferros-flash-gui-linux-x86_64"
       --file tool - macos-arm64 ferros-flash-gui "$OUT/ferros-flash-gui-macos-arm64"
       --file board akita - akita "$OUT/akita.board.vsf"
       --file board husky - husky "$OUT/husky.board.vsf"
       --file logo - - logo logo.vsf)
for b in akita husky; do
    v=FERROS_FLASH_BOOT_${b^^}
    [ -n "${!v:-}" ] && FILES+=(--file boot "$b" - boot "${!v}")
done
$FF release make --channel "$CHANNEL" --commit "${FERROS_FLASH_COMMIT:-$(git rev-parse HEAD)}" --key "$KEY" --out "$OUT/release.vsf" "${FILES[@]}"
$FF release verify "$OUT/release.vsf"

sha() { sha256sum "$1" | cut -c1-64; }
W=$(sha "$OUT/ferros-flash-windows-x86_64.exe" | tr a-f A-F)
WG=$(sha "$OUT/ferros-flash-gui-windows-x86_64.exe" | tr a-f A-F)
sed -e "s|__BASE__|$BASE|g" -e "s|__SHA256_WINDOWS_X86_64__|$W|" -e "s|__SHA256_GUI_WINDOWS_X86_64__|$WG|" tools/flash/install/install.ps1 > "$OUT/install.ps1"
sed -e "s|__BASE__|$BASE|g" -e "s|__SHA256_LINUX_X86_64__|$(sha "$OUT/ferros-flash-linux-x86_64")|" -e "s|__SHA256_MACOS_ARM64__|$(sha "$OUT/ferros-flash-macos-arm64")|" \
    -e "s|__SHA256_GUI_LINUX_X86_64__|$(sha "$OUT/ferros-flash-gui-linux-x86_64")|" -e "s|__SHA256_GUI_MACOS_ARM64__|$(sha "$OUT/ferros-flash-gui-macos-arm64")|" tools/flash/install/install.sh > "$OUT/install.sh"

echo "== staged in $OUT ($CHANNEL)"
echo "   Windows, in PowerShell (run as a file, never thru iex: docs/FLASHER.md \"The first Windows test\"):"
echo "     irm $BASE/install.ps1 -OutFile \$env:TEMP\\ferros-install.ps1"
echo "     powershell -ExecutionPolicy Bypass -File \$env:TEMP\\ferros-install.ps1"
echo "   Linux and macOS:   curl -sSfL $BASE/install.sh | sh"
if [ $UPLOAD = 0 ]; then
    echo "   not uploaded (--upload puts it in the R2 bucket $BUCKET)"
    exit 0
fi
export WRANGLER_SEND_METRICS=false
if [ -z "${CLOUDFLARE_API_TOKEN:-}" ] && [ -r /mnt/Harbor/Code/keys/cloudflare-r2-token ]; then
    CLOUDFLARE_API_TOKEN=$(tr -d ' \n' < /mnt/Harbor/Code/keys/cloudflare-r2-token)
    export CLOUDFLARE_API_TOKEN
    export CLOUDFLARE_ACCOUNT_ID=${CLOUDFLARE_ACCOUNT_ID:-6be7ec6dc3bb3f60fabb62ff944cc2c2}
fi
put() {
    local name=$1 file=$2 ctype=$3 cache=$4 attempt
    for attempt in 0 1 2; do
        wrangler r2 object put "$BUCKET/$name" --file "$file" --content-type "$ctype" --cache-control "$cache" --remote >/dev/null && return 0
        echo "   upload of $name failed (attempt $attempt), again"
    done
    echo "   upload of $name failed three times: stopped"
    exit 1
}
# The hash-named files first, the release next, the installers last: an installer never names an executable that is not yet there.
for f in "$OUT"/*; do
    n=$(basename "$f")
    case $n in
        [0-9a-f][0-9a-f][0-9a-f][0-9a-f]*) [ ${#n} = 64 ] && put "$n" "$f" application/octet-stream "public, max-age=31536000, immutable" ;;
    esac
done
for n in ferros-flash-windows-x86_64.exe ferros-flash-linux-x86_64 ferros-flash-macos-arm64 ferros-flash-gui-windows-x86_64.exe ferros-flash-gui-linux-x86_64 ferros-flash-gui-macos-arm64; do
    put "$n" "$OUT/$n" application/octet-stream no-cache
done
put release.vsf "$OUT/release.vsf" application/octet-stream no-cache
# The installers as Photon's are served (text/plain, no charset). The Windows one is run as a file, never thru Invoke-Expression (docs/FLASHER.md "The first Windows test": two failures that way on 2026-11-24).
put install.ps1 "$OUT/install.ps1" text/plain no-cache
put install.sh "$OUT/install.sh" text/plain no-cache
echo "== uploaded to $BUCKET ($BASE)"
