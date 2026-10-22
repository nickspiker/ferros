#!/usr/bin/env bash
# A full profile of the husky panel with an i1 Pro through ArgyllCMS, patches driven by the kernel (`bridge display patch`), in phases so the person holding the instrument and the one driving can take turns (DISPLAY-CALIB.md has the first, partial profile of 2026-10-17).
#
# Every reading is spectral (`spotread -s`) and kept raw; the CSV holds the Yxy. The measurement window is a disc 1 cm across (PX_PER_CM, 489 ppi) at the centre of the panel: the instrument is lined up on it once, and every patch is drawn there, so window size (APL) and full field can be compared for the roll-off the panel and its driver apply (voltage sag, deliberate limiting).
#
# Phases:
#   off       no shell next boot (vault `home.off`), reboot: the panel is the bridge's alone; an instrument on the glass must not make the map redraw
#   target    the alignment target: the 1 cm disc, white at 133 nits, on black
#   cal       the i1 Pro's own white-tile calibration (instrument in its cradle on the tile)
#   check     one reading of the target, to confirm the instrument sees the disc
#   run [sections...]   the readings (default: all, in order): drift rolloff sustain apl ramps nearblack colours drift
#   ten <control|dither|link10|restore>   is the link really 10 bits: grey 10-bit codes 512-520 at ~1000 nits, read twice each, in the 1 cm disc. control = 10 bpc compose on the 8 bpc link with the dither off (expect plateaus four codes wide), dither = the same dithered (the instrument averages it: an even ramp), link10 = the 10 bpc DSC stream, no dither (even steps only if all ten bits arrive), restore = the default (10 bpc compose, dithered)
#   on        the shell back (remove `home.off`), reboot
# Output: oracle/display/husky-hk3/profile-<date>/readings.csv (section, label, dbv, window px, rgb, surround, seconds since start, Y cd/m², x, y) and spotread.log (every reading's full output, spectra included).
# Needs: the instrument readable (sudo chmod a+rw /dev/bus/usb/<bus>/<dev>, or the colord group), cold ferros on USB, a dark room.
set -uo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
PX_PER_CM=${PX_PER_CM:-193}
DIR=${PROFILE_DIR:-$REPO/oracle/display/husky-hk3/profile-$(date +%Y%m%d)}
CSV=$DIR/readings.csv
LOG=$DIR/spotread.log
T0=$(date +%s)
run() { timeout 60 "$B" "$@" >/dev/null 2>&1; }
wait_ferros() { for _ in $(seq 60); do lsusb -d 1209:4665 >/dev/null 2>&1 && return 0; sleep 2; "$REPO/tools/usb-kick.sh" --if-wedged >/dev/null 2>&1; done; echo "ferros is not on USB"; return 1; }
dbv() { local v=$1; run display dcs "f0 5a 5a / 51 $(printf '%02x %02x' $((v >> 8)) $((v & 255))) / f0 a5 a5"; }
# High brightness mode: the panel's HBM bit (53 = G#E0) with DBV G#0FFF; back out to normal mode at the given DBV.
hbm_on() { run display dcs "f0 5a 5a / 53 e0 / 51 0f ff / f0 a5 a5"; }
hbm_off() { run display dcs "f0 5a 5a / 53 20 / f0 a5 a5"; dbv "${1:-1353}"; }
patch() { run display patch "$1" "$2" "${3:-000000}"; }
# One reading: the full output to the log under a header, the Yxy back ("Result is Yxy: 139.52 0.3127 0.3290").
read_one() {
    local label=$1 out
    out=$(timeout 90 spotread -e -N -O -x -s 2>&1)
    printf '=== %s t=%s\n%s\n' "$label" "$(( $(date +%s) - T0 ))" "$out" >> "$LOG"
    echo "$out" | grep -oE "Yxy: *[-0-9.]+ +[-0-9.]+ +[-0-9.]+" | head -1 | sed -E 's/Yxy: *//'
}
# section label dbv window rgb surround [settle s]: draw, settle, read, record.
m() {
    local sec=$1 label=$2 d=$3 win=$4 rgb=$5 sur=${6:-000000} settle=${7:-1.5}
    patch "$win" "$rgb" "$sur"; sleep "$settle"
    local r; r=$(read_one "$sec $label dbv=$d win=$win rgb=$rgb sur=$sur")
    [ -z "$r" ] && r="? ? ?"
    echo "$sec,$label,$d,$win,$rgb,$sur,$(( $(date +%s) - T0 )),${r// /,}" | tee -a "$CSV"
}
# The 10-bit disc: 10-bit grey code, read twice.
m10() {
    local sec=$1 c=$2
    run display patch10 "$PX_PER_CM" "$c" "$c" "$c" 0; sleep 1.5
    for k in 1 2; do
        local r; r=$(read_one "$sec g10=$c #$k")
        [ -z "$r" ] && r="? ? ?"
        echo "$sec,g10-$c-$k,3307,$PX_PER_CM,g10=$c,0,$(( $(date +%s) - T0 )),${r// /,}" | tee -a "$CSV"
    done
}
ten() {
    case "$1" in
        control) run display modeset wqhd 10; run display dither 0 ;;
        dither) run display modeset wqhd 10; run display dither 1 ;;
        link10) run display modeset wqhd link10 ;;
        restore) run display modeset wqhd 10; dbv 1353; patch $W1 ffffff; return ;;
        *) echo "ten control|dither|link10|restore"; return 1 ;;
    esac
    dbv 3307
    for c in $(seq 512 520); do m10 "ten-$1" "$c"; done
}
W1=$PX_PER_CM; W2=$((2 * PX_PER_CM)); W4=$((4 * PX_PER_CM))
LEVELS="196 400 853 1353 2000 2600 3307"
CODES="0 16 32 48 64 80 96 112 128 144 160 176 192 208 224 240 255"

sec_drift() { dbv 1353; m drift white 1353 $W1 ffffff; }
# Window against full field at every brightness: how much the panel gives a small bright area against the whole screen.
sec_rolloff() {
    for d in $LEVELS; do dbv $d; for w in $W1 $W2 $W4 0; do m rolloff white $d $w ffffff; done; done
    hbm_on; for w in $W1 $W2 $W4 0; do m rolloff white hbm $w ffffff; done; hbm_off 1353
}
# Full white held: whatever the limiter does over half a minute (heat, current), read back to back.
sec_sustain() {
    for d in 3307 hbm; do
        if [ $d = hbm ]; then hbm_on; else dbv $d; fi
        patch 0 ffffff; local t=$(date +%s)
        for i in $(seq 12); do m sustain "white+$(( $(date +%s) - t ))s" $d 0 ffffff 000000 0; done
        patch 0 000000; sleep 20
    done
    hbm_off 1353
}
# The disc white, the surround stepped from black to white: the limiter's response to average picture level, the window fixed.
sec_apl() {
    for d in 3307 hbm; do
        if [ $d = hbm ]; then hbm_on; else dbv $d; fi
        for s in 000000 404040 808080 c0c0c0 ffffff; do m apl "white-on-$s" $d $W1 ffffff $s; done
    done
    hbm_off 1353
}
# Tone curves, grey and each channel alone, in the window, at two brightnesses (Samsung panels shift gamma with DBV).
sec_ramps() {
    for d in 1353 3307; do
        dbv $d
        for c in $CODES; do h=$(printf '%02x' $c)
            m ramps "grey$c" $d $W1 "$h$h$h"; m ramps "red$c" $d $W1 "${h}0000"; m ramps "green$c" $d $W1 "00${h}00"; m ramps "blue$c" $d $W1 "0000${h}"
        done
    done
}
# The shadows, where gamma 2.185 against VSF's 2.0 matters most.
sec_nearblack() { dbv 1353; for c in 1 2 3 4 6 8 12; do h=$(printf '%02x' $c); m nearblack "grey$c" 1353 $W1 "$h$h$h" 000000 3; done; }
# Primaries, secondaries, white and a mid grey, spectra kept: the gamut under any observer (VSF's Stockman-Sharpe 10° included), and additivity.
sec_colours() {
    for d in 1353 3307; do dbv $d
        for p in red:ff0000 green:00ff00 blue:0000ff cyan:00ffff magenta:ff00ff yellow:ffff00 white:ffffff grey128:808080; do m colours "${p%%:*}" $d $W1 "${p#*:}"; done
    done
}

case "${1:-}" in
    off) run store put home.off 1 && run reboot; sleep 8; wait_ferros && echo "shell off; the panel is the bridge's" ;;
    on) run store rm home.off; run reboot; sleep 8; wait_ferros && echo "shell back" ;;
    target) dbv 1353; patch $W1 ffffff; echo "target: white disc, A#$W1 px (1 cm at $PX_PER_CM px/cm), 133 nits" ;;
    cal) echo "instrument on its white tile:"; echo | timeout 120 spotread -e -O 2>&1 | tail -3 ;;
    check) mkdir -p "$DIR"; dbv 1353; patch $W1 ffffff; sleep 1.5; echo "Yxy $(read_one check)" ;;
    ten)
        mkdir -p "$DIR"; [ -f "$CSV" ] || echo "section,label,dbv,window_px,rgb,surround,t_s,Y_cdm2,x,y" > "$CSV"
        ten "${2:-}" ;;
    run)
        shift; mkdir -p "$DIR"
        [ -f "$CSV" ] || echo "section,label,dbv,window_px,rgb,surround,t_s,Y_cdm2,x,y" > "$CSV"
        secs=${*:-"drift rolloff sustain apl ramps nearblack colours drift"}
        for s in $secs; do echo "== $s"; "sec_$s"; done
        patch 0 000000; echo "done: $CSV" ;;
    *) sed -n '2,20p' "$0"; exit 1 ;;
esac
