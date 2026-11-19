#!/usr/bin/env bash
# The 8a's GNSS bring-up as gnssd does it, traced whole (docs/GPS.md "akita's KEPLER: the core is the CP's"): from Android on slot a with Magisk root, gnssd stopped and run again under tools/gtrace, so every byte it writes and reads on /dev/gnss_boot (the CP's commands that power the core, load /vendor/firmware/kepler.bin into the CP's GNSS_FW memory, check it and start it) and on /dev/gnss_ipc (CHPP over spi_21, nothing lost, unlike the hv's register captures) lands in one log, with the GNSS image ioctls' structs.
# No partition is written, no slot changed, nothing flashed: ferros's boot is never touched. No hands: Android must be up on slot a with root (the 8a has no lock screen); a Maps session (granted for the session and taken back, as tools/akita-hv-camcap.sh does) gives the fix the trace runs to.
# The log carries the session's place (the chip's fixes): it stays in keys/ ($C), never in a repo; `decode` prints no place.
#
# Steps, each its own command (FERROS_PORT/AKITA_SERIAL as in akita-hv-camcap.sh):
#   tools/akita-gnss-trace.sh build      host only: gtrace (static aarch64, no libc) and gnss-oracle
#   tools/akita-gnss-trace.sh run        the trace (and the images gnssd loads, with kepler.bin, into $C): gnssd stopped, the core held in reset (RESET=1, the default: gnssd's own `pcie_assert_reset`, G#0203 on gnss_boot, so the new gnssd must load the firmware again), gnssd under gtrace for SECS (default 180), a Maps session to a fix, gnssd given back to init
#   tools/akita-gnss-trace.sh decode     host only: the gnss_boot commands and the CHPP session's BETP records (gnss-oracle --gtrace --betp), no place
set -eu
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh
if [ -n "${AKITA_SERIAL:-}" ]; then device_pin() { device_pinned akita "$AKITA_SERIAL"; }; fi
cd "$(dirname "$0")/.."
REPO=$PWD
W=${W:-$REPO/target/akita-gnss-trace}
C=${C:-/mnt/Harbor/Code/keys/akita/captures-$(date +%F)}
SECS=${SECS:-180}
RESET=${RESET:-1}
GT=$W/gtrace/aarch64-unknown-linux-gnu/release/gtrace
GO=$W/oracle/release/gnss-oracle
D=/data/local/tmp

die() { echo "== $*" >&2; exit 1; }
say() { echo "== $*"; }
A() { adb "$@"; }
in_() { A shell "$*" >/dev/null 2>&1 || true; }
su_() { A shell "su -c '$*'" 2>/dev/null | tr -d '\r'; }

cmd_build() {
    (cd tools/gtrace && CARGO_TARGET_DIR=$W/gtrace cargo build --release -q)
    (cd tools/gnss-oracle && CARGO_TARGET_DIR=$W/oracle cargo build --release -q)
    say "built: $GT, $GO"
}

# The gps provider's last fix by its elapsed time alone (akita-hv-camcap.sh `gps_et`): no position read.
gps_et() { A shell "dumpsys location" 2>/dev/null | tr -d '\r' | grep -o 'Location\[gps[^]]*' | grep -o 'et=[^ ,]*' | head -1 || true; }
maps_grant() { in_ "pm grant com.google.android.apps.maps android.permission.ACCESS_FINE_LOCATION; pm grant com.google.android.apps.maps android.permission.ACCESS_COARSE_LOCATION"; }
maps_ungrant() {
    in_ "pm revoke com.google.android.apps.maps android.permission.ACCESS_FINE_LOCATION; pm revoke com.google.android.apps.maps android.permission.ACCESS_COARSE_LOCATION"
    in_ "pm clear-permission-flags com.google.android.apps.maps android.permission.ACCESS_FINE_LOCATION user-set user-fixed; pm clear-permission-flags com.google.android.apps.maps android.permission.ACCESS_COARSE_LOCATION user-set user-fixed"
}

cmd_run() {
    [ -x "$GT" ] || die "no gtrace: $0 build"
    device_pin akita || die "no 8a on adb (Android on slot a)"
    [ "$(A shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ] || die "Android has not finished booting"
    [ "$(su_ id -u)" = 0 ] || die "root refused: Magisk's Shell grant"
    mkdir -p "$C"
    A push "$GT" "$D/gtrace" >/dev/null
    su_ "chmod 755 $D/gtrace"
    say "gnssd stopped"
    su_ "setprop ctl.stop gnssd"
    sleep 2
    if [ "$RESET" = 1 ]; then
        # gnssd's own `pcie_assert_reset`: the u16 G#0203 written to the CP's gnss_boot channel (its answer queued for a reader and dropped as this shell closes the device).
        say "the core held in reset (G#0203 on /dev/gnss_boot)"
        su_ "printf \"\\003\\002\" > /dev/gnss_boot"
        sleep 1
    fi
    say "gnssd under gtrace for $SECS s"
    if [ "${ATTACH:-1}" = 1 ]; then
        # gnssd started by init as always (its own domain and user, so the location HAL connects to it), gtrace attached the moment it has a pid: what gnssd sends once the HAL is there, which a gnssd run by hand under gtrace never saw (2026-11-19).
        A shell "su -c 'setprop ctl.start gnssd; until pidof gnssd >/dev/null; do :; done; $D/gtrace $D/gnssd.trace $SECS -p \$(pidof gnssd)'" >/dev/null 2>&1 &
    else
        A shell "su -c '$D/gtrace $D/gnssd.trace $SECS /vendor/bin/hw/gnssd'" >/dev/null 2>&1 &
    fi
    local tr=$!
    sleep 16
    local e0 e k=0
    e0=$(gps_et)
    in_ "input keyevent KEYCODE_WAKEUP; wm dismiss-keyguard"
    maps_grant
    in_ "am start -n com.google.android.apps.maps/com.google.android.maps.MapsActivity"
    until e=$(gps_et); [ -n "$e" ] && [ "$e" != "$e0" ]; do
        sleep 2; k=$((k + 1))
        [ $k -lt 64 ] || break
    done
    if [ $k -lt 64 ]; then say "a GPS fix about $((k * 2)) s after Maps opened"; sleep 32; else say "no GPS fix in 128 s"; fi
    in_ "input keyevent KEYCODE_HOME"
    maps_ungrant
    wait "$tr" || true
    su_ "setprop ctl.start gnssd"
    A exec-out "su -c 'cat $D/gnssd.trace'" > "$C/gnss-trace.log"
    A exec-out "su -c 'cat $D/gnssd.trace'" > "$C/gnss-trace.again"
    cmp -s "$C/gnss-trace.log" "$C/gnss-trace.again" && rm "$C/gnss-trace.again" || say "the two pulls differ: keep both"
    # The images gnssd handed cpif (gtrace saves each IOCTL_LOAD_GNSS_IMAGE buffer: kepler.bin whole, then the chip configuration it built from kepler.bin and gps.cfg), and the firmware file itself: Samsung's, keys/ only.
    local i
    for i in $(su_ "ls $D" | grep '^gnssd.trace.img'); do
        A exec-out "su -c 'cat $D/$i'" > "$C/${i#gnssd.}"
        say "$i: $(wc -c < "$C/${i#gnssd.}") bytes"
    done
    A exec-out "su -c 'cat /vendor/firmware/kepler.bin'" > "$C/kepler.bin"
    su_ "rm $D/gnssd.trace $D/gnssd.trace.img*"
    say "gnssd given back to init; the trace in $C/gnss-trace.log ($(wc -l < "$C/gnss-trace.log") lines)"
}

cmd_decode() {
    [ -x "$GO" ] || die "no gnss-oracle: $0 build"
    local t=${1:-$C/gnss-trace.log}
    # The CP's commands for the core (power, reset, firmware) as the oracle's marks, then every BETP record (the fix's place masked).
    "$GO" "$t" --gtrace --betp --hex 32 | grep -v 'BETP G#0002_0143' || true
    "$GO" "$t" --gtrace --betp | grep 'BETP G#0002_0143' | head -4 || true
}

case ${1:-} in
build) cmd_build;;
run) cmd_run;;
decode) shift; cmd_decode "$@";;
*) die "usage: $0 build | run | decode [trace]";;
esac
