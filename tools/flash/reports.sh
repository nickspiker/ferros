#!/usr/bin/env bash
# The ferros-flash reports people sent to fgtw.org/report (docs/FLASHER.md "Submitting"): listed from the R2 bucket fgtw-data under ferros-reports/, each fetched once into target/flash-reports and rendered.
# fgtw.org deletes a report 2^5 days after it came; a report holds no handle, key or location, so the local copies are ordinary files, kept out of the repo by target/.
# Usage: tools/flash/reports.sh [<hash or its first characters>]
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT=target/flash-reports
mkdir -p "$OUT"
TOKEN=${CLOUDFLARE_API_TOKEN:-$(tr -d ' \n' < /mnt/Harbor/Code/keys/cloudflare-r2-token)}
ACCOUNT=${CLOUDFLARE_ACCOUNT_ID:-6be7ec6dc3bb3f60fabb62ff944cc2c2}
cargo build -q -p ferros-flash
FF=${CARGO_TARGET_DIR:-target}/debug/ferros-flash
keys=$(curl -sf -H "Authorization: Bearer $TOKEN" "https://api.cloudflare.com/client/v4/accounts/$ACCOUNT/r2/buckets/fgtw-data/objects?prefix=ferros-reports/" | grep -o '"key":"ferros-reports/[^"]*"' | cut -d'"' -f4)
for k in $keys; do
    name=${k#ferros-reports/}
    hash=${name#*-}
    hash=${hash%.vsf}
    case $hash in "${1:-}"*) ;; *) continue ;; esac
    [ -f "$OUT/$name" ] || CLOUDFLARE_API_TOKEN=$TOKEN CLOUDFLARE_ACCOUNT_ID=$ACCOUNT WRANGLER_SEND_METRICS=false wrangler r2 object get "fgtw-data/$k" --file "$OUT/$name" --remote >/dev/null
    echo "== $hash"
    $FF log unpack "$OUT/$name" "$OUT/${name%.vsf}.log.vsf" >/dev/null
    $FF log show "$OUT/${name%.vsf}.log.vsf"
done
