#!/usr/bin/env bash
# Fold two single-speaker audio tracks into ONE stereo file with a speaker per side, at matched
# loudness — for listening back to a call with each voice on its own ear, and as the archive form
# of a recording whose speakers were captured on separate tracks.
#
#   usage: downmix-tracks.sh <left.wav> <right.wav> <out.flac> [target_LUFS]
#
# Loudness is matched by measurement, not by peak: each input's integrated loudness (EBU R128,
# ffmpeg ebur128) is measured, and each is gained to the same target (default -18 LUFS). A stereo
# input is folded to mono before placement. A limiter after the gain keeps a quiet track that needed
# a big lift from clipping on its loudest moments. Fully local.
set -euo pipefail
L="${1:?usage: downmix-tracks.sh <left> <right> <out.flac> [target_LUFS]}"
R="${2:?}"
OUT="${3:?}"
TARGET="${4:--18}"

lufs() {  # integrated loudness of a file, folded to mono first so stereo/mono compare alike
    ffmpeg -nostats -hide_banner -i "$1" -af "pan=mono|c0=0.5*c0+0.5*c1,ebur128=peak=none" -f null - 2>&1 \
      | awk '/Integrated loudness/{f=1} f && /I:/{print $2; exit}'
}
IL=$(lufs "$L"); IR=$(lufs "$R")
[ -n "$IL" ] && [ -n "$IR" ] || { echo "loudness measurement failed" >&2; exit 1; }
GL=$(python3 -c "print(round($TARGET - ($IL), 2))")
GR=$(python3 -c "print(round($TARGET - ($IR), 2))")
echo "left:  $IL LUFS -> gain ${GL} dB" >&2
echo "right: $IR LUFS -> gain ${GR} dB" >&2

ffmpeg -nostats -hide_banner -loglevel warning -y -i "$L" -i "$R" -filter_complex \
  "[0:a]pan=mono|c0=0.5*c0+0.5*c1,volume=${GL}dB,alimiter=limit=0.95:level=false[l];
   [1:a]pan=mono|c0=0.5*c0+0.5*c1,volume=${GR}dB,alimiter=limit=0.95:level=false[r];
   [l][r]join=inputs=2:channel_layout=stereo:map=0.0-FL|1.0-FR[out]" \
  -map "[out]" -ar 48000 -sample_fmt s16 -c:a flac "$OUT"

# Verify: the two output channels should now sit at the same loudness.
for ch in FL FR; do
    v=$(ffmpeg -nostats -hide_banner -i "$OUT" -af "pan=mono|c0=$ch,ebur128=peak=none" -f null - 2>&1 \
        | awk '/Integrated loudness/{f=1} f && /I:/{print $2; exit}')
    echo "out $ch: $v LUFS" >&2
done
