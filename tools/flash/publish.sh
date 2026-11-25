#!/usr/bin/env bash
# Stage a ferros-flash release (docs/FLASHER.md "Distribution"): the executables, the board manifests and the boot images, each under its BLAKE3, a release document signed with the ferros key naming them all, and the two one-line installers with the executables' SHA-256 written in.
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

cargo build -q --release -p ferros-flash --target x86_64-pc-windows-gnu
cargo build -q --release -p ferros-flash --target x86_64-unknown-linux-gnu
cargo build -q -p ferros-flash --features publish
FF=$T/debug/ferros-flash
cp "$T/x86_64-pc-windows-gnu/release/ferros-flash.exe" "$OUT/ferros-flash-windows-x86_64.exe"
cp "$T/x86_64-unknown-linux-gnu/release/ferros-flash" "$OUT/ferros-flash-linux-x86_64"
$FF board akita "$OUT/akita.board.vsf"
$FF board husky "$OUT/husky.board.vsf"

FILES=(--file tool - windows-x86_64 ferros-flash.exe "$OUT/ferros-flash-windows-x86_64.exe"
       --file tool - linux-x86_64 ferros-flash "$OUT/ferros-flash-linux-x86_64"
       --file board akita - akita "$OUT/akita.board.vsf"
       --file board husky - husky "$OUT/husky.board.vsf")
for b in akita husky; do
    v=FERROS_FLASH_BOOT_${b^^}
    [ -n "${!v:-}" ] && FILES+=(--file boot "$b" - boot "${!v}")
done
$FF release make --channel "$CHANNEL" --commit "${FERROS_FLASH_COMMIT:-$(git rev-parse HEAD)}" --key "$KEY" --out "$OUT/release.vsf" "${FILES[@]}"
$FF release verify "$OUT/release.vsf"

W=$(sha256sum "$OUT/ferros-flash-windows-x86_64.exe" | cut -c1-64 | tr a-f A-F)
L=$(sha256sum "$OUT/ferros-flash-linux-x86_64" | cut -c1-64)
sed -e "s|__BASE__|$BASE|g" -e "s|__SHA256_WINDOWS_X86_64__|$W|" tools/flash/install/install.ps1 > "$OUT/install.ps1"
sed -e "s|__BASE__|$BASE|g" -e "s|__SHA256_LINUX_X86_64__|$L|" -e "s|__SHA256_MACOS_ARM64__||" tools/flash/install/install.sh > "$OUT/install.sh"

echo "== staged in $OUT ($CHANNEL)"
echo "   Windows, in PowerShell (run as a file, never thru iex: docs/FLASHER.md \"The first Windows test\"):"
echo "     irm $BASE/install.ps1 -OutFile \$env:TEMP\\ferros-install.ps1"
echo "     powershell -ExecutionPolicy Bypass -File \$env:TEMP\\ferros-install.ps1"
echo "   Linux:   curl -sSfL $BASE/install.sh | sh"
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
put ferros-flash-windows-x86_64.exe "$OUT/ferros-flash-windows-x86_64.exe" application/octet-stream no-cache
put ferros-flash-linux-x86_64 "$OUT/ferros-flash-linux-x86_64" application/octet-stream no-cache
put release.vsf "$OUT/release.vsf" application/octet-stream no-cache
# The installers as Photon's are served (text/plain, no charset). The Windows one is run as a file, never thru Invoke-Expression (docs/FLASHER.md "The first Windows test": two failures that way on 2026-11-24).
put install.ps1 "$OUT/install.ps1" text/plain no-cache
put install.sh "$OUT/install.sh" text/plain no-cache
echo "== uploaded to $BUCKET ($BASE)"
