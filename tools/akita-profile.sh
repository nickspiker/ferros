#!/usr/bin/env bash
# The AK3B (Pixel 8a) profiled with an i1 Pro through ArgyllCMS, unattended (owner, 2026-11-08: "you want the i1 on it so you can run a full cal set too?"), as tools/display-profile.sh profiles husky's HK3.
# The patches are the display server's own (op 52 mode 8 through the capture payload: a disc of any 10-bit colour on any surround, raw past the present table), centred on the panel, the instrument lined up on the 1 cm disc once. The panel's brightness through op 6 (DBV) and op 9 (WRCTRLD: G#20 normal, G#E0 high brightness mode).
# Every reading is spectral (`spotread -s`) and kept raw in spotread.log; readings.csv holds section, label, dbv, radius px, the 10-bit rgb, the surround, seconds since the run began, Y cd/m², x, y.
# The link carries 8 bits a channel (the DQE truncates the two low bits of the 10-bit word), so the codes below are 8-bit codes times four.
#
# Usage: tools/akita-profile.sh check | run [sections...]   (sections: drift dbv ramps nearblack colours rolloff apl sustain drift; default all, in that order)
# Needs: the i1 Pro calibrated on its tile (spotread asks once a session), readable (/dev/bus/usb), lined up on the disc; a development display on the 8a; a dark room.
set -uo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
CAP=$REPO/target/el0/aarch64-unknown-none/release/capture.bin
# A centimetre on akita's glass (430 ppi): the disc's radius 84 px.
R1=84
ROOT=$REPO/oracle/display/akita-ak3b
n=0; while [ -e "$ROOT/profile-$n" ]; do n=$((n + 1)); done
DIR=${PROFILE_DIR:-$ROOT/profile-$n}
CSV=$DIR/readings.csv
LOG=$DIR/spotread.log

le() { printf '%02X%02X' $(($1 & 255)) $(($1 >> 8)); }
send() { timeout 60 "$B" spawn "$CAP" "$1" --use display >/dev/null 2>&1; }
# patch fg_r fg_g fg_b bg_r bg_g bg_b radius (10-bit codes)
patch() { send "3408$(le $1)$(le $2)$(le $3)$(le $4)$(le $5)$(le $6)$(le $7)"; }
dbv() { send "06$(le $1)"; }
normal() { send "095320"; dbv "$1"; }
hbm() { send "0953E0"; dbv "${1:-4095}"; }

read_one() {
    local label=$1 out
    out=$(timeout 120 spotread -e -N -O -x -s 2>&1)
    printf '=== %s t=%s\n%s\n' "$label" "$SECONDS" "$out" >> "$LOG"
    echo "$out" | grep -oE "Yxy: *[-0-9.]+ +[-0-9.]+ +[-0-9.]+" | head -1 | sed -E 's/Yxy: *//'
}
# m section label dbv r g b [bg] [radius] [settle s]: draw, settle, read, record (codes 8-bit, sent times four).
m() {
    local sec=$1 label=$2 d=$3 r=$4 g=$5 b=$6 bg=${7:-0} rad=${8:-$R1} settle=${9:-1.5}
    patch $((r * 4)) $((g * 4)) $((b * 4)) $((bg * 4)) $((bg * 4)) $((bg * 4)) "$rad"; sleep "$settle"
    local y; y=$(read_one "$sec $label dbv=$d rgb8=$r,$g,$b bg=$bg radius=$rad")
    [ -z "$y" ] && y="? ? ?"
    echo "$sec,$label,$d,$rad,$r-$g-$b,$bg,$SECONDS,${y// /,}" | tee -a "$CSV"
}

DBV_NORMAL="209 300 400 600 853 1000 1100 1200 1240 1260 1280 1300 1353 1500 1700 2000 2300 2600 2900 3175"
DBV_HBM="3176 3400 3600 3800 4000 4095"
CODES="0 8 16 24 32 40 48 56 64 72 80 88 96 104 112 120 128 136 144 152 160 168 176 184 192 200 208 216 224 232 240 248 255"

sec_drift() { hbm; m drift white hbm 255 255 255; }
# Luminance per DBV, full white in the disc: normal mode's curve (Google: a 2.2 power law, 2 to 800 cd/m²) and high brightness mode's (linear to 1400).
sec_dbv() {
    for d in $DBV_NORMAL; do normal "$d"; m dbv white "$d" 255 255 255; done
    for d in $DBV_HBM; do hbm "$d"; m dbv white "hbm$d" 255 255 255; done
}
# Tone curves, grey and each channel alone, at normal mode's top.
sec_ramps() {
    normal 3175
    for c in $CODES; do m ramps "grey$c" 3175 $c $c $c; m ramps "red$c" 3175 $c 0 0; m ramps "green$c" 3175 0 $c 0; m ramps "blue$c" 3175 0 0 $c; done
}
# The shadows, at the panel's brightest so there is light to read.
sec_nearblack() { hbm; for c in 1 2 3 4 6 8 12 16; do m nearblack "grey$c" hbm $c $c $c 0 "$R1" 3; m nearblack "grey$c" hbm $c $c $c 0 "$R1" 3; done; }
# Primaries, secondaries, white and a mid grey, spectra kept, at normal mode's top and in high brightness mode.
sec_colours() {
    for d in 3175 hbm; do
        if [ $d = hbm ]; then hbm; else normal $d; fi
        for p in red:255:0:0 green:0:255:0 blue:0:0:255 cyan:0:255:255 magenta:255:0:255 yellow:255:255:0 white:255:255:255 grey128:128:128:128; do
            IFS=: read -r name r g b <<< "$p"; m colours "$name" $d $r $g $b
        done
    done
}
# The disc against larger areas and the whole panel.
sec_rolloff() {
    for d in 3175 hbm; do
        if [ $d = hbm ]; then hbm; else normal $d; fi
        for rad in $R1 $((2 * R1)) $((4 * R1)) 0; do m rolloff white $d 255 255 255 0 $rad; done
    done
}
# The disc white, the surround stepped from black to white.
sec_apl() {
    for d in 3175 hbm; do
        if [ $d = hbm ]; then hbm; else normal $d; fi
        for s in 0 64 128 192 255; do m apl "white-on-$s" $d 255 255 255 $s; done
    done
}
# Full white held half a minute, read back to back, then a rest on black.
sec_sustain() {
    for d in 3175 hbm; do
        if [ $d = hbm ]; then hbm; else normal $d; fi
        patch 1020 1020 1020 0 0 0 0
        for i in $(seq 12); do m sustain "white+$i" $d 255 255 255 0 0 0; done
        patch 0 0 0 0 0 0 0; sleep 20
    done
}

case "${1:-}" in
    check) hbm; patch 1020 1020 1020 0 0 0 $R1; sleep 2; echo "Yxy $(timeout 120 spotread -e -N -O -x 2>&1 | grep -oE 'Yxy: *[-0-9. ]+')" ;;
    run)
        shift; mkdir -p "$DIR"
        [ -f "$CSV" ] || echo "section,label,dbv,radius_px,rgb8,surround8,t_s,Y_cdm2,x,y" > "$CSV"
        secs=${*:-drift dbv ramps nearblack colours rolloff apl sustain drift}
        for s in $secs; do "sec_$s"; done
        # The panel left as the display server set it: high brightness mode at its top, the patch released.
        hbm; send "3400"
        echo "done: $CSV" ;;
    *) sed -n 2,8p "$0" ;;
esac
