#!/usr/bin/env bash
# The 8a's camera captures (docs/CAMERA.md "akita: the capture session"): Android on the 8a (akita) runs each camera in each of its modes under an oracle hv on Android's slot, the hv logs the registers it touches into the console ring, a deliberate crash keeps the ring, and the next boot's pstore is pulled into keys/akita/.
# Since 2026-11-18 Android lives on slot a (Android 17, CP3A.261005.005, kernel 6.1.162, Magisk on init_boot_a) and slot b holds ferros's gate, so SLOT defaults to a: the capture images go to boot_a, around slot a's own kernel (unpacked from the stock boot_a saved in keys/akita/images), and ferros's boot_b is never written.
# Lumis on Android first wherever it opens the mode (the owner, 2026-11-18); the Pixel Camera app for what Lumis cannot open (the video modes).
# What Lumis 0.2.8 lists on the 8a (2026-11-18): the ultrawide (4208 x 3120, to 16 s), "multiple Back" = the main at 80 degrees (4624 x 3472, to 16 s) and the main's in-sensor 2x crop at 46 degrees (4624 x 3472 of 0.8 um pixels, to 16 s), the front (4208 x 3072, to 1 s); Camera2 has no maximum-resolution mode on any 8a camera (dumpsys: 4624 x 3472 is the main's whole array).
# AUTO=1 drives Android by adb taps (positions measured at 1080 x 2400, Lumis 0.2.8, Pixel Camera 10.4): no hands, when the 8a has no lock screen (`locksettings get-disabled` true). Without AUTO the owner acts at each step, as before; his wife's Android is unlocked by him alone (no PIN, password or handle is ever asked for, typed or kept by this script).
#
# Steps, each its own command (a worktree's cargo trips on the sibling crates: run from the main checkout or a copy of it), FERROS_PORT the 8a's ferros socket:
#   tools/akita-hv-camcap.sh build              host only: the capture images (i2c-main, i2c-uw, i2c-front, isp, isp-uw) into $W, around $STOCK's kernel
#   tools/akita-hv-camcap.sh save               boot_$SLOT dumped twice into $C/back (from ferros, or from Android with root); that is what `restore` puts back
#   tools/akita-hv-camcap.sh rehearse           from Android: root, Lumis, the camera list and the clocks read
#   tools/akita-hv-camcap.sh flash <image>      to fastboot, slot $SLOT checked current, boot_$SLOT written whole (no set-active), Android boots under the capture image
#   tools/akita-hv-camcap.sh capture <session>  the marked session, the crash, the next boot's ring pulled twice and compared
#   tools/akita-hv-camcap.sh again              a fresh capture boot on the image already flashed (the read boot cleared the hv's one-shot marker)
#   tools/akita-hv-camcap.sh decode <session>   host only: the transfers and their integrity, the inits per mark, the holds; the marks and PDMA lists of an ISP session
#   tools/akita-hv-camcap.sh restore            boot_$SLOT back from $C/back/boot_$SLOT.img
# Sessions and their images: i2c-main (binned, crop), i2c-main-16s, i2c-main-gcam on the image `i2c-main`, and the same three for i2c-uw and i2c-front (the front's long one i2c-front-1s), one boot each (`again` between); isp-main, isp-front, isp-main-crop on `isp`; isp-uw on `isp-uw`.
# Nothing here writes any partition but boot_$SLOT, and only in `flash` and `restore`; never a set-active, never a wipe, never a vbmeta.
set -eu
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh
# AKITA_SERIAL pins the 8a by its serial alone: device_pin otherwise asks every phone in fastboot for its product, and on 2026-11-18 a `getvar` to the Pro, wedged in another session's fastboot, hung a capture for nine minutes.
if [ -n "${AKITA_SERIAL:-}" ]; then device_pin() { device_pinned akita "$AKITA_SERIAL"; }; fi
cd "$(dirname "$0")/.."
REPO=$PWD
# No FERROS_HV_* from the caller's shell reaches a build: each image sets exactly its own.
for v in $(env | grep -o '^FERROS_HV_[A-Z0-9_]*' || true); do unset "$v"; done
SLOT=${SLOT:-a}
W=${W:-$REPO/target/akita-camcap}
C=${C:-/mnt/Harbor/Code/keys/akita/captures-$(date +%F)}
B=${B:-$REPO/target/debug/ferros-bridge}
# Android's own boot image on that slot, as Google shipped it (dumped twice from ferros, 2026-11-18): its kernel goes into every capture image, its AVB footer is the template.
STOCK=${STOCK:-/mnt/Harbor/Code/keys/akita/images/slot-a-CP3A.261005.005/boot_$SLOT.img}
LINUX=${LINUX:-$W/Image}
AVB_KEY=${AVB_KEY:-/mnt/Harbor/Code/keys/avb.vsf}
AVB_TEMPLATE=${AVB_TEMPLATE:-$STOCK}
AUTO=${AUTO:-0}
# Armed 60 s after the reset (the hv's counter runs about 20 s ahead of Android's uptime): past the camera service's boot probe, before anything is opened.
ARM_MS=60000
CAP=$((1 << 15))
MKIMG=$W/cargo/debug/ferros-mkimg
OI=$W/oracle-i2c/release/oracle-i2c
CO=$W/cam-oracle/release/cam-oracle

# The 42 front-end pages a camera open touches (the Pro's census, 2026-10-19) and the PHY's first page (the light build of 2026-11-15 trapped G#1C10_0000-G#1C10_4000).
ISP_PAGES="G#1C00_0000,G#1C00_1000,G#1C00_3000,G#1C00_7000,G#1C02_0000,G#1C04_0000,G#1C05_0000,G#1C06_0000,G#1C07_0000,G#1C08_0000,G#1C09_0000,G#1C0A_0000,G#1C0B_0000,G#1C0C_0000,G#1C0D_0000,G#1C0E_0000,G#1C0F_0000,G#1C10_0000,G#1C10_1000,G#1C10_2000,G#1C10_3000,G#1C32_0000,G#1C32_2000,G#1C32_8000,G#1C32_9000,G#1C33_0000,G#1C33_1000,G#1C33_3000,G#1C33_5000,G#1C33_7000,G#1C33_8000,G#1C33_9000,G#1C33_B000,G#1C33_C000,G#1C33_D000,G#1C33_E000,G#1C35_0000,G#1C39_0000,G#1C39_1000,G#1C39_2000,G#1C39_3000,G#1C39_4000,G#1C39_5000"
# The ultrawide's frames did not go thru the PDMA program on 2026-11-15: every page of the front end's span, so whatever carries them is in the log.
ISP_SPAN="G#1C00_0000-G#1C3F_F000"
# The GNSS captures keep spi_21's data (TX_DATA +18, RX_DATA +1C), its packet count (+20, an RX's length) and the GPIO data word holding gpp4-3 (G#1084_0084, the AP-to-GNSS handshake); quiet are the controller's set-up and status words (CH_CFG, MODE_CFG, SLAVE_SEL, STATUS, SWAP_CFG, PENDING_CLR: the first session's 2^15 lines were two thirds of them, 2026-11-18) and an EINT another driver works in the same GPIO page (+900, +A00, +B08).
GNSS_QUIET="G#111E_0000,G#111E_0008,G#111E_000C,G#111E_0014,G#111E_0028,G#111E_002C,G#1084_0900,G#1084_0A00,G#1084_0B08"

die() { echo "== $*" >&2; exit 1; }
say() { echo "== $*"; }

# Each image's hv knobs.
image_knobs() {
    local ctrl
    case $1 in
    i2c-main) ctrl=1088;;
    i2c-front) ctrl=1089;;
    i2c-uw) ctrl=108A;;
    isp) echo "FERROS_HV_ORACLE_PAGES=$ISP_PAGES FERROS_HV_ORACLE_LATE=1 FERROS_HV_PDMA_DUMP=1"; return;;
    isp-uw) echo "FERROS_HV_ORACLE_PAGES=$ISP_SPAN FERROS_HV_ORACLE_LATE=1 FERROS_HV_PDMA_DUMP=1"; return;;
    # The GNSS (Samsung KEPLER) on spi_21: its controller page (PIO only, every payload byte a TX_DATA write or an RX_DATA read) and the PERIC0 GPIO page that holds the AP-to-GNSS handshake gpp4-3, GNSS_QUIET quiet. gnss-boot logs from the hv's start (the firmware load), gnss from ARM_MS (a session to a fix).
    gnss) echo "FERROS_HV_ORACLE_PAGES=G#111E_0000,G#1084_0000 FERROS_HV_ORACLE_QUIET=$GNSS_QUIET"; return;;
    gnss-boot) echo "FERROS_HV_ORACLE_PAGES=G#111E_0000,G#1084_0000 FERROS_HV_ORACLE_QUIET=$GNSS_QUIET FERROS_HV_ORACLE_ARM_AFTER_MS=0"; return;;
    *) die "no image $1 (i2c-main, i2c-uw, i2c-front, isp, isp-uw, gnss, gnss-boot)";;
    esac
    # One controller, its sensor alone (every akita sensor answers at G#1A; the main's lens, OIS and EEPROM at G#24 and the others' EEPROMs at G#50 stay out), the controller's nine setup and status words quiet.
    local q="" o
    for o in 0000 0004 0008 0020 0024 0030 0040 0048 0050; do q="$q,G#${ctrl}_$o"; done
    echo "FERROS_HV_ORACLE_PAGES=G#${ctrl}_0000 FERROS_HV_ORACLE_I2C_TARGET=G#1A FERROS_HV_ORACLE_QUIET=${q#,}"
}

session_image() {
    case $1 in
    i2c-main | i2c-main-16s | i2c-main-gcam) echo i2c-main;;
    i2c-uw | i2c-uw-16s | i2c-uw-gcam) echo i2c-uw;;
    i2c-front | i2c-front-1s | i2c-front-gcam) echo i2c-front;;
    isp-main | isp-front | isp-main-crop) echo isp;;
    isp-uw) echo isp-uw;;
    gnss-fix) echo gnss;;
    gnss-boot) echo gnss-boot;;
    *) die "no session $1 (i2c-main, i2c-main-16s, i2c-main-gcam, i2c-uw, i2c-uw-16s, i2c-uw-gcam, i2c-front, i2c-front-1s, i2c-front-gcam, isp-main, isp-front, isp-main-crop, isp-uw)";;
    esac
}

session_ctrl() {
    case $1 in
    i2c-main*) echo G#1088_0000;;
    i2c-front*) echo G#1089_0000;;
    i2c-uw*) echo G#108A_0000;;
    esac
}

# Which ferros device is the 8a: FERROS_PORT names its socket (the Pro may be on the same host).
ferros_here() {
    if [ -n "${FERROS_PORT:-}" ]; then [ "$(cat /sys/bus/usb/devices/$FERROS_PORT/idProduct 2>/dev/null)" = 4665 ]; return; fi
    [ "$(lsusb -d 1209:4665 2>/dev/null | wc -l)" -eq 1 ]
}
need_port() {
    [ -n "${FERROS_PORT:-}" ] || [ "$(lsusb -d 1209:4665 2>/dev/null | wc -l)" -le 1 ] || die "two ferros devices on the host: set FERROS_PORT to the 8a's socket (lsusb -t)"
}

A() { adb "$@"; }
# The host's adb server often misses the 8a after it comes back on a USB 3 port (2026-11-15): restarted when lsusb sees Google's adb id and adb lists nothing.
adb_kick() {
    if lsusb -d 18d1:4ee7 >/dev/null 2>&1 && ! adb devices | grep -q 'device$'; then adb kill-server >/dev/null 2>&1 || true; adb start-server >/dev/null 2>&1 || true; fi
}
wait_android() {
    local k=0
    until device_pin akita 2>/dev/null && [ "$(A shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; do
        sleep 3; k=$((k + 1))
        [ $((k % 10)) = 0 ] && adb_kick
        [ $k -lt 200 ] || die "no Android after 10 minutes: stop here (no retries); see docs/CAMERA.md 'If Android does not come back'"
    done
}
# The owner unlocks his wife's Android himself; the script only waits for the user's storage to open (nothing typed, nothing read but that flag).
owner_unlock() {
    until [ "$(A shell getprop sys.user.0.ce_available 2>/dev/null | tr -d '\r')" = true ]; do
        say "the owner: unlock the 8a by hand now (the script waits)"
        sleep 8
    done
}
su_ok() { [ "$(A shell "su -c 'id -u'" 2>/dev/null | tr -d '\r')" = 0 ]; }
need_su() {
    until su_ok; do
        [ "$AUTO" = 1 ] && die "root refused: the owner opens the Magisk app and grants Shell (Superuser)"
        say "root refused: the owner opens the Magisk app and grants Shell (Superuser), or unlocks the phone if it is locked; Enter when done"
        read -r _ </dev/tty
    done
}
mark() { A shell "su -c 'echo \"<3>ferros-mark: $1\" > /dev/kmsg'" >/dev/null 2>&1; }
# Hands-free: the screen on and past the (disabled) lock screen, then taps.
in_() { A shell "$*" >/dev/null 2>&1 || true; }
wake() { in_ "input keyevent KEYCODE_WAKEUP; wm dismiss-keyguard"; }
# One step: the owner's instruction, Enter, the mark, then the owner acts; `s` skips it (marked as skipped). With AUTO=1 the mark, then the step's own taps (its third argument, a function).
step() {
    local m=$1 what=$2 act=${3:-}
    echo
    if [ "$AUTO" = 1 ]; then
        say "$m (hands-free)"
        wake
        mark "$m"
        [ -z "$act" ] || "$act"
        return 0
    fi
    say "$what"
    local r
    read -r -p "   Enter, then do it (s + Enter: skip) " r </dev/tty
    if [ "$r" = s ]; then mark "$m: skipped"; return 1; fi
    mark "$m"
}
done_step() {
    local m=$1 act=${2:-}
    if [ "$AUTO" = 1 ]; then
        [ -z "$act" ] || "$act"
        mark "$m: end"
        return 0
    fi
    local r
    read -r -p "   Enter when it is done " r </dev/tty
    mark "$m: end"
}
hold() { say "keep it running: $1 s"; sleep "$1"; }

# Lumis 0.2.8 at 1080 x 2400 (screenshots 2026-11-18): the menu's entries at y 1480 (ultrawide), 1690 (the main's "multiple Back"), 1880 (front), each opening a menu of its own: the ultrawide's and the front's single entry at 1690 (BACK above it), the main's 1480 (80 degrees) and 1690 (the 46 degree crop); in a camera, EXIT at (831, 1254) leaves Lumis whole, and SHUTTER's knob rides y 1935 (dragged right to the end: 16 s on the rear cameras, 1 s on the front). Each open starts at 1/15 s.
lumis_menu() { in_ "input keyevent KEYCODE_HOME"; sleep 1; in_ "am start -n com.lumis.camera/.PermissionsRequest"; sleep 4; }
lumis_uw() { lumis_menu; in_ "input tap 540 1480"; sleep 2; in_ "input tap 540 1690"; sleep 6; }
lumis_main() { lumis_menu; in_ "input tap 540 1690"; sleep 2; in_ "input tap 540 1480"; sleep 6; }
lumis_crop() { lumis_menu; in_ "input tap 540 1690"; sleep 2; in_ "input tap 540 1690"; sleep 6; }
lumis_front() { lumis_menu; in_ "input tap 540 1880"; sleep 2; in_ "input tap 540 1690"; sleep 6; }
lumis_longest() { in_ "input swipe 567 1935 1040 1935 600"; }
lumis_exit() { in_ "input tap 831 1254"; sleep 2; in_ "input keyevent KEYCODE_HOME"; sleep 1; }
lumis_uw_16() { lumis_uw; lumis_longest; hold 36; }
lumis_main_16() { lumis_main; lumis_longest; hold 36; }
lumis_front_1() { lumis_front; lumis_longest; hold 6; }
# Pixel Camera 10.4 at 1080 x 2400 (2026-11-18): zoom .5 (411, 1521), 1x (534, 1521), 2 (657, 1521); flip (855, 1776); Photo (468, 2172), Video (588, 2172). The app reopens the camera and mode used last, so every session leaves it on the rear 1x Photo.
gcam() { in_ "input keyevent KEYCODE_HOME"; sleep 1; in_ "am start -a android.media.action.STILL_IMAGE_CAMERA"; sleep 4; }
gcam_main() { gcam; in_ "input tap 534 1521"; hold 5; mark "pixel camera main 2x"; in_ "input tap 657 1521"; hold 5; in_ "input tap 534 1521"; sleep 1; }
gcam_uw() { gcam; in_ "input tap 411 1521"; hold 5; mark "pixel camera uw video"; in_ "input tap 588 2172"; hold 5; in_ "input tap 468 2172"; sleep 2; in_ "input tap 534 1521"; sleep 1; }
gcam_front() { gcam; in_ "input tap 855 1776"; hold 5; mark "pixel camera front video"; in_ "input tap 588 2172"; hold 5; in_ "input tap 468 2172"; sleep 2; in_ "input tap 855 1776"; sleep 3; }
gcam_home() { in_ "input keyevent KEYCODE_HOME"; sleep 1; }

to_fastboot() {
    need_port
    if ferros_here; then timeout 60 "$B" reboot fastboot >/dev/null 2>&1 || true
    elif device_pin akita 2>/dev/null && A get-state >/dev/null 2>&1; then A reboot bootloader || true; fi
    local i
    for i in $(seq 90); do device_pin akita 2>/dev/null && fastboot devices | grep -q . && break; sleep 2; done
    device_pin akita && fastboot devices | grep -q . || die "no fastboot: if the 8a shows fastboot but answers nothing, power + volume down by hand"
}
# fastboot's own words, and a stop on FAILED (a whole night once ran on a stale build behind a hidden failed flash).
flash_boot() {
    local out
    out=$(fastboot flash boot_$SLOT "$1" 2>&1)
    echo "$out"
    if echo "$out" | grep -q FAILED || ! echo "$out" | grep -q OKAY; then die "boot_$SLOT NOT written"; fi
}
slot_is_ours() {
    local slot
    slot=$(fastboot getvar current-slot 2>&1 | awk '/^current-slot:/ {print $2}')
    [ "$slot" = "$SLOT" ] || die "the current slot is '${slot:-?}', not $SLOT: stop; the owner decides (this script never sets a slot active)"
}

cmd_build() {
    [ -f "$STOCK" ] || die "no stock boot image at $STOCK"
    [ -f "$AVB_TEMPLATE" ] || die "no AVB template at $AVB_TEMPLATE"
    mkdir -p "$W"
    CARGO_TARGET_DIR=$W/cargo cargo build -q -p ferros-mkimg
    [ -f "$LINUX" ] || "$MKIMG" unpack-kernel --boot "$STOCK" -o "$LINUX"
    local img k
    for img in ${IMAGES:-i2c-main i2c-uw i2c-front isp isp-uw gnss gnss-boot}; do
        k=$(image_knobs $img)
        # As the gate's hv (akita's board, the hv core) plus stage 2, the oracle, no PMU reads, the oracle's knobs, armed at ARM_MS, CAP lines at most; built in a target of its own so no other session's hv build is taken for it.
        env FERROS_HV_ORACLE_CAP=$CAP FERROS_HV_ORACLE_ARM_AFTER_MS=$ARM_MS $k FERROS_HV_BOARD=akita FERROS_HV_CORE=1 FERROS_HV_STAGE2=1 FERROS_HV_ORACLE=1 FERROS_HV_NO_PMU_READ=1 \
            CARGO_TARGET_DIR=$W/cargo cargo build -q -p ferros_hv --target aarch64-unknown-none-softfloat --release
        aarch64-linux-gnu-objcopy -O binary "$W/cargo/aarch64-unknown-none-softfloat/release/ferros_hv" "$W/hv-$img.bin"
        [ "$(stat -c %s "$W/hv-$img.bin")" -le $((0x180000)) ] || die "hv-$img is over its G#18_0000 pad"
        # The hv alone in front of Android (no ferros packed, no gate); the console quiet from boot (pstore integrity) and the image's name on the command line, which `capture` checks.
        "$MKIMG" gate --hv "$W/hv-$img.bin" --image "$LINUX" --cmdline "loglevel=4 ferros_cap.image=$img" -o "$W/$img.bare.img"
        # Whole partition with its own footer (a bare 20 MB image buries Google's vbmeta and ABL refuses the slot, 2026-11-06).
        "$MKIMG" avb-footer --key "$AVB_KEY" --google-boot "$AVB_TEMPLATE" --image "$W/$img.bare.img" -o "$W/$img.img"
        say "$img: $W/$img.img ($(stat -c %s "$W/$img.img") bytes, blake3 $(b3sum --no-names "$W/$img.img" | cut -c1-16)); hv knobs: $k"
    done
    CARGO_TARGET_DIR=$W/oracle-i2c cargo build -q --release --manifest-path tools/oracle-i2c/Cargo.toml
    CARGO_TARGET_DIR=$W/cam-oracle cargo build -q --release --manifest-path tools/cam-oracle/Cargo.toml
    say "built around $LINUX; the template was $AVB_TEMPLATE"
}

cmd_save() {
    mkdir -p "$C/back"
    local p=boot_$SLOT
    need_port
    if ferros_here; then
        [ -x "$B" ] || cargo build -q -p ferros-bridge
        "$B" part dump $p "$C/back/$p.img"
        "$B" part dump $p "$C/back/$p.again"
    else
        wait_android
        need_su
        A exec-out "su -c 'cat /dev/block/by-name/$p'" > "$C/back/$p.img"
        A exec-out "su -c 'cat /dev/block/by-name/$p'" > "$C/back/$p.again"
    fi
    cmp "$C/back/$p.img" "$C/back/$p.again" || die "$p read twice differs"
    rm "$C/back/$p.again"
    ( cd "$C/back" && b3sum $p.img >> B3SUMS )
    say "saved: $C/back/$p.img, blake3 $(b3sum --no-names "$C/back/$p.img" | cut -c1-16) (what restore flashes)"
}

cmd_rehearse() {
    wait_android
    owner_unlock
    need_su
    mkdir -p "$C"
    {
        echo "build $(A shell getprop ro.build.fingerprint | tr -d '\r')"
        echo "slot $(A shell getprop ro.boot.slot_suffix | tr -d '\r')"
        echo "kernel $(A shell uname -r | tr -d '\r')"
        echo "hv reserved: $(A shell 'ls /sys/firmware/devicetree/base/reserved-memory/' | tr -d '\r' | grep -c '^hv@')"
        echo "lumis: $(A shell pm list packages com.lumis.camera | tr -d '\r')"
        echo "pixel camera: $(A shell pm list packages com.google.android.GoogleCamera | tr -d '\r')"
    } | tee "$C/rehearsal.txt"
    # The cameras' characteristics only: the service's client and event lines (which apps used the camera, when) are left on the phone.
    A shell "su -c 'dumpsys media.camera'" | tr -d '\r' | grep -v -i -E 'package|client|event|pid' > "$C/dumpsys-media-camera.txt" || true
    say "dumpsys: $(grep -c -i 'MaximumResolution' "$C/dumpsys-media-camera.txt" || true) maximum-resolution lines -> $C/dumpsys-media-camera.txt"
    step "rehearsal lumis" "the owner: open Lumis (allow the camera if it asks), then the main's first entry, and leave it open" lumis_main || true
    if [ "${AKITA_CLOCKS:-1}" = 1 ]; then
        # The master clocks while a camera streams, one clock at a time (debugfs's clk_summary resets this phone, 2026-11-15).
        A shell "su -c 'mountpoint -q /sys/kernel/debug || mount -t debugfs debugfs /sys/kernel/debug'" >/dev/null 2>&1 || true
        local c
        for c in $(A shell "su -c 'ls /sys/kernel/debug/clk'" 2>/dev/null | tr -d '\r' | grep -i -E 'cis_clk|cis-clk|ciscl' | head -16); do
            echo "$c $(A shell "su -c 'cat /sys/kernel/debug/clk/$c/clk_rate'" 2>/dev/null | tr -d '\r')"
        done | tee "$C/cis-clocks.txt"
    fi
    done_step "rehearsal lumis" lumis_exit
    say "rehearsal done: $C/rehearsal.txt. Next: $0 flash i2c-main"
}

cmd_flash() {
    local img=${1:?usage: $0 flash <i2c-main|i2c-uw|i2c-front|isp|isp-uw>}
    image_knobs "$img" >/dev/null
    [ -f "$W/$img.img" ] || die "no $W/$img.img: run build first"
    [ -f "$C/back/boot_$SLOT.img" ] || die "no $C/back/boot_$SLOT.img: run save first (restore needs it)"
    to_fastboot
    slot_is_ours
    if [ ! -f "$C/pkvm.txt" ]; then fastboot oem pkvm status > "$C/pkvm.txt" 2>&1 || true; say "pKVM: $(grep -i pkvm "$C/pkvm.txt" | head -1) (recorded, not changed)"; fi
    flash_boot "$W/$img.img"
    fastboot reboot >/dev/null 2>&1
    echo "$img" > "$C/flashed"
    say "boot_$SLOT holds $img; Android boots under it (no gate). Next: $0 capture <session>"
}

# The sessions, one capture boot each: on 2026-11-18 the main's binned and crop opens alone (each about 16000 lines with its frames' holds) filled the 2^15-line cap, so the long exposure and the Pixel Camera app get boots of their own (`again` between them). Lumis first.
steps_i2c-main() {
    step "lumis main binned" "open Lumis, the main lens's first entry (80 degrees, the whole field)" lumis_main && { hold 6; done_step "lumis main binned" lumis_exit; }
    step "lumis main crop" "EXIT; open the main's 46 degree entry (the in-sensor 2x crop)" lumis_crop && { hold 6; done_step "lumis main crop" lumis_exit; }
}
steps_i2c-main-16s() {
    step "lumis main 16s" "open the main's first entry, drag SHUTTER to 16 s, wait until a whole 16 s frame has landed (about 35 s), then EXIT; never a volume key (Lumis saves; the hv kills on both)" lumis_main_16 && done_step "lumis main 16s" lumis_exit
}
steps_i2c-main-gcam() {
    if step "pixel camera main" "open the Pixel Camera app on 1x for 5 s; then tap 2x yourself and hold 5 s" gcam_main; then
        [ "$AUTO" = 1 ] || { A shell am start -a android.media.action.STILL_IMAGE_CAMERA >/dev/null 2>&1 || true; hold 5; mark "pixel camera main 2x"; }
        done_step "pixel camera main" gcam_home
    fi
}
steps_i2c-uw() { step "lumis uw binned" "open Lumis, the ultrawide's entry" lumis_uw && { hold 6; done_step "lumis uw binned" lumis_exit; }; }
steps_i2c-uw-16s() { step "lumis uw 16s" "the ultrawide's entry, SHUTTER to 16 s, wait for one whole frame (about 35 s), EXIT" lumis_uw_16 && done_step "lumis uw 16s" lumis_exit; }
steps_i2c-uw-gcam() {
    if step "pixel camera uw" "open the Pixel Camera app and tap .5; after 5 s switch to Video and hold 5 s" gcam_uw; then
        [ "$AUTO" = 1 ] || { A shell am start -a android.media.action.STILL_IMAGE_CAMERA >/dev/null 2>&1 || true; sleep 3; A shell input tap 411 1521 >/dev/null 2>&1 || true; hold 5; mark "pixel camera uw video"; }
        done_step "pixel camera uw" gcam_home
    fi
}
steps_i2c-front() { step "lumis front binned" "open Lumis, the front lens's entry" lumis_front && { hold 6; done_step "lumis front binned" lumis_exit; }; }
steps_i2c-front-1s() { step "lumis front 1s" "the front's entry, SHUTTER to its longest (1 s), wait 5 s, EXIT" lumis_front_1 && done_step "lumis front 1s" lumis_exit; }
steps_i2c-front-gcam() {
    if step "pixel camera front" "open the Pixel Camera app and flip to the front; after 5 s switch to Video and hold 5 s, then flip back to the rear (the app reopens the camera used last)" gcam_front; then
        [ "$AUTO" = 1 ] || { A shell am start -a android.media.action.STILL_IMAGE_CAMERA >/dev/null 2>&1 || true; sleep 3; A shell input tap 855 1776 >/dev/null 2>&1 || true; hold 5; mark "pixel camera front video"; }
        done_step "pixel camera front" gcam_home
    fi
}
# An ISP session holds one camera's open: the PDMA dump is taken once a boot (its 16th frame), and the front end's per-frame answers fill the cap in seconds.
steps_isp-main() { step "lumis main binned" "open Lumis, the main lens's first entry" lumis_main && { hold 8; done_step "lumis main binned" lumis_exit; }; }
steps_isp-front() { step "lumis front binned" "open Lumis, the front lens's entry" lumis_front && { hold 8; done_step "lumis front binned" lumis_exit; }; }
steps_isp-uw() { step "lumis uw binned" "open Lumis, the ultrawide's entry" lumis_uw && { hold 8; done_step "lumis uw binned" lumis_exit; }; }
# The GNSS sessions. Nothing on the 8a asks for GPS by itself (Pixel Camera and Lumis leave the gps provider OFF; Maps has no location permission there), so a hands-free session grants Maps fine and coarse location for the session and takes them back after, to the flags they had (none set by a user). The fix is told by the gps provider's last location's elapsed time alone: no position is ever read or printed.
gps_et() { A shell "dumpsys location" 2>/dev/null | tr -d '\r' | grep -o 'Location\[gps[^]]*' | grep -o 'et=[^ ,]*' | head -1 || true; }
maps_grant() { in_ "pm grant com.google.android.apps.maps android.permission.ACCESS_FINE_LOCATION; pm grant com.google.android.apps.maps android.permission.ACCESS_COARSE_LOCATION"; }
maps_ungrant() {
    in_ "pm revoke com.google.android.apps.maps android.permission.ACCESS_FINE_LOCATION; pm revoke com.google.android.apps.maps android.permission.ACCESS_COARSE_LOCATION"
    in_ "pm clear-permission-flags com.google.android.apps.maps android.permission.ACCESS_FINE_LOCATION user-set user-fixed; pm clear-permission-flags com.google.android.apps.maps android.permission.ACCESS_COARSE_LOCATION user-set user-fixed"
    say "Maps's location: $(A shell dumpsys package com.google.android.apps.maps | tr -d '\r' | grep -E 'ACCESS_(FINE|COARSE)_LOCATION: granted' | sed 's/^ *//' | tr '\n' ' ')"
}
maps_open() { [ "$AUTO" = 1 ] && maps_grant; in_ "am start -n com.google.android.apps.maps/com.google.android.maps.MapsActivity"; }
steps_gnss-fix() {
    local e0 e k=0
    e0=$(gps_et)
    step "maps open" "open Google Maps (its location request starts the GNSS session)" maps_open || return 0
    until e=$(gps_et); [ -n "$e" ] && [ "$e" != "$e0" ]; do
        sleep 2; k=$((k + 1))
        [ $k -lt 128 ] || break
    done
    if [ $k -lt 128 ]; then mark "first fix"; say "a GPS fix about $((k * 2)) s after Maps opened"; hold 32; else mark "no fix in 256 s"; say "no GPS fix in 256 s"; fi
    mark "maps close"
    in_ "input keyevent KEYCODE_HOME"
    hold 16
    [ "$AUTO" = 1 ] && maps_ungrant
    return 0
}
steps_gnss-boot() { step "boot load" "nothing to do: the firmware load at boot is in the ring already" && hold 2; }
steps_isp-main-crop() { step "lumis main crop" "open Lumis, the main's 46 degree entry" lumis_crop && { hold 8; done_step "lumis main crop" lumis_exit; }; }

cmd_capture() {
    local s=${1:?usage: $0 capture <session>}
    local img
    img=$(session_image "$s")
    mkdir -p "$C"
    wait_android
    A shell cat /proc/cmdline | grep -q "ferros_cap.image=$img" || die "this boot is not the $img image (flash it, or 'again' after a read boot)"
    until A shell "test \$(cut -d. -f1 /proc/uptime) -ge $((ARM_MS / 1000 + 2))" 2>/dev/null; do sleep 3; done
    owner_unlock
    need_su
    A shell "su -c 'echo 4 > /proc/sys/kernel/printk'" >/dev/null 2>&1
    [ "$AUTO" = 1 ] && in_ "settings put global stay_on_while_plugged_in 7"
    say "session $s on $img: the phone on its stand, the scene lit; hands only where the script asks"
    mark "session $s"
    "steps_$s" || true
    mark "closed"
    A shell input keyevent KEYCODE_HOME >/dev/null 2>&1 || true
    sleep 2
    say "flushing: Android crashes on purpose now and boots again (no hands)"
    A shell "su -c 'echo c > /proc/sysrq-trigger'" >/dev/null 2>&1 || true
    sleep 15
    wait_android
    need_su
    local raw=$C/$s.console-ramoops-0
    A exec-out "su -c 'cat /sys/fs/pstore/console-ramoops-0'" > "$raw"
    A exec-out "su -c 'cat /sys/fs/pstore/console-ramoops-0'" > "$raw.again"
    cmp "$raw" "$raw.again" || die "the ring read twice differs"
    rm "$raw.again"
    grep -a -E "ferros hv|ferros-mark" "$raw" > "$C/$s.log" || true
    ( cd "$C" && b3sum "$s.console-ramoops-0" "$s.log" >> B3SUMS )
    say "$(wc -l < "$C/$s.log") lines -> $C/$s.log; oracle lines $(grep -c 'ferros hv: S2' "$C/$s.log" || true), marks $(grep -c 'ferros-mark' "$C/$s.log" || true)"
    grep -a -m1 -E 'oracle armed|oracle pages trapped' "$C/$s.log" || say "no arming line in the ring: the capture is not usable"
    grep -a -q 'oracle skipped this boot' "$C/$s.log" && say "this ring is a skip boot's: the capture is not in it"
    [ "$img" = "${img#isp}" ] || say "PDMA dump lines: $(grep -c 'PDMA buffer' "$C/$s.log" || true)"
    echo "$s" >> "$C/sessions"
    say "next: $0 decode $s; another session on $img: $0 again; else $0 flash <image> or $0 restore"
}

cmd_again() {
    wait_android
    [ -f "$C/flashed" ] && A shell cat /proc/cmdline | grep -q "ferros_cap.image=$(cat "$C/flashed")" || die "Android is not on the flashed capture image"
    A reboot
    say "a fresh capture boot of $(cat "$C/flashed"); then $0 capture <session>"
}

cmd_decode() {
    local s=${1:?usage: $0 decode <session>}
    [ -f "$C/$s.log" ] || die "no $C/$s.log"
    [ -x "$OI" ] || CARGO_TARGET_DIR=$W/oracle-i2c cargo build -q --release --manifest-path tools/oracle-i2c/Cargo.toml
    [ -x "$CO" ] || CARGO_TARGET_DIR=$W/cam-oracle cargo build -q --release --manifest-path tools/cam-oracle/Cargo.toml
    case $s in
    i2c-*)
        local base m n
        base=$(session_ctrl "$s")
        # Integrity: every transfer's byte count against its programmed length; any mismatch inside an init means recapture (feedback_oracle_capture_integrity).
        "$CO" i2c "$C/$s.log" --base "$base" > "$C/$s.decoded" 2> "$C/$s.integrity"
        cat "$C/$s.integrity"
        "$OI" "$C/$s.log" --base "$base" --max-bytes 32 --holds G#1A > "$C/$s.holds" 2>/dev/null || true
        # One init per mark: the sensor's transfers from the mark to its next stream on (Sony's G#0100 = 1).
        grep '^# MARK' "$C/$s.decoded" | sed 's/^# MARK //' | grep -v -E ': (end|skipped)$|^session |^closed$' | while read -r m; do
            n=$(echo "$m" | tr ' ' '-')
            "$CO" init "$C/$s.decoded" G#1A "$C/$s.$n.init" --mark "$m" --stream-on "01 00 01" 2>&1 | sed "s/^/$n: /" || true
        done
        say "the holds (exposure, frame and any long-exposure word): $C/$s.holds"
        ;;
    isp-*)
        "$CO" marks "$C/$s.log" | tee "$C/$s.marks"
        local k
        for k in 1 2 3 4; do "$CO" pdma "$C/$s.log" --list $k > "$C/$s.list$k.txt" 2>/dev/null && say "PDMA list $k: $(wc -l < "$C/$s.list$k.txt") lines" || rm -f "$C/$s.list$k.txt"; done
        say "then by hand (docs/oracle/CAMERA.md): cam-oracle cpu $C/$s.log <a> <b> <c> <prefix> --link <the link> (a, b, c from $C/$s.marks), cam-oracle pdma-image $C/$s.log <out.cpr3> --list <n> --own-image"
        ;;
    gnss-*)
        # How the trapped accesses fall over spi_21's registers (CH_CFG +0, MODE_CFG +8, SLAVE_SEL +C, TX_DATA +18, RX_DATA +1C, PACKET_CNT +20, SWAP_CFG +28) and the GPIO page; the bytes themselves by hand for now (docs/GPS.md "akita's KEPLER").
        grep -a 'ferros hv: S2 [0-9A-F]' "$C/$s.log" | awk '{print $4, $5}' | sort | uniq -c | sort -rn | head -32 > "$C/$s.summary" || true
        cat "$C/$s.summary"
        ;;
    *) die "no session $s";;
    esac
    ( cd "$C" && b3sum "$s".* 2>/dev/null | grep -v -e '\.console-ramoops-0$' -e '\.log$' >> B3SUMS ) || true
}

cmd_restore() {
    [ -f "$C/back/boot_$SLOT.img" ] || die "no $C/back/boot_$SLOT.img"
    to_fastboot
    slot_is_ours
    flash_boot "$C/back/boot_$SLOT.img"
    fastboot reboot >/dev/null 2>&1
    rm -f "$C/flashed"
    local i
    for i in $(seq 90); do
        ferros_here && { say "ferros on USB after $((i * 2)) s"; return 0; }
        adb devices 2>/dev/null | grep -q 'device$' && { say "Android on adb after $((i * 2)) s"; return 0; }
        sleep 2
    done
    die "nothing on USB after 180 s: read ABL's log from fastboot (fastboot oem dmesg) before anything else"
}

case "${1:-}" in
build) cmd_build;;
save) cmd_save;;
rehearse) cmd_rehearse;;
flash) cmd_flash "${2:-}";;
capture) cmd_capture "${2:-}";;
again) cmd_again;;
decode) cmd_decode "${2:-}";;
restore) cmd_restore;;
*) sed -n '2,22p' "$0"; exit 2;;
esac
