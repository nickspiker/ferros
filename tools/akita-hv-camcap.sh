#!/usr/bin/env bash
# The 8a's camera captures (docs/CAMERA.md "akita: the capture session"): Android on the 8a (akita) runs each camera in each of its modes under an oracle hv on slot b, the hv logs the registers it touches into the console ring, a deliberate crash keeps the ring, and the next boot's pstore is pulled into keys/akita/.
# Lumis on Android first wherever it opens the mode (the owner, 2026-11-18): its binned entry, its maximum-resolution entry and its shutter to 16 s; the Pixel Camera app for what Lumis cannot open (the 2x crop if Lumis does not list it, the video modes).
# The owner's hands: his wife's Android is unlocked by him alone (no PIN, password or handle is ever asked for, typed or kept by this script), and he opens each Lumis entry when the script says.
#
# Steps, each its own command, run from the main checkout (a worktree's cargo trips on the sibling crates), the 8a on a motherboard USB port (FERROS_PORT its ferros socket):
#   tools/akita-hv-camcap.sh build              host only: the five capture images (i2c-main, i2c-uw, i2c-front, isp, isp-uw) into $W
#   tools/akita-hv-camcap.sh save               from ferros on the 8a: boot_b (the gate) and boot_a dumped twice each into $C/back, the gate's Android kernel checked against $LINUX; boot_b's dump is what `restore` puts back
#   tools/akita-hv-camcap.sh rehearse           from ferros: Android under the gate as it is (`diag "android here"`), nothing flashed; the owner unlocks; root, Lumis, the camera list and the clocks read
#   tools/akita-hv-camcap.sh flash <image>      to fastboot, slot b checked current, boot_b written whole (no set-active), Android boots under the capture image
#   tools/akita-hv-camcap.sh capture <session>  the marked session (the owner opens what it says), the crash, the next boot's ring pulled twice and compared
#   tools/akita-hv-camcap.sh again              a fresh capture boot on the image already flashed (the read boot cleared the hv's one-shot marker)
#   tools/akita-hv-camcap.sh decode <session>   host only: the transfers and their integrity, the inits per mark, the holds; the marks and PDMA lists of an ISP session
#   tools/akita-hv-camcap.sh restore            boot_b back from $C/back/boot_b.img; ferros boots through the gate (the identity unlocks at boot)
# Sessions and their images: i2c-main, i2c-uw, i2c-front (each its own image); isp-main, isp-front, isp-main-max (the image `isp`); isp-uw (the image `isp-uw`).
# Nothing here writes any partition but boot_b, and only in `flash` and `restore`; never a set-active, never a wipe, never a vbmeta.
set -eu
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh
cd "$(dirname "$0")/.."
REPO=$PWD
# No FERROS_HV_* from the caller's shell reaches a build: each image sets exactly its own.
for v in $(env | grep -o '^FERROS_HV_[A-Z0-9_]*' || true); do unset "$v"; done
W=${W:-$REPO/target/akita-camcap}
C=${C:-/mnt/Harbor/Code/keys/akita/captures-$(date +%F)}
B=${B:-$REPO/target/debug/ferros-bridge}
# Android's kernel as slot b's gate carries it (Google's 6.1.157 ab14791245, zero-padded to 17 x 2 MiB): `save` checks it against the dump.
LINUX=${LINUX:-/mnt/Harbor/Code/keys/akita/images/linux_Image_akita_2026-11-13}
AVB_KEY=${AVB_KEY:-/mnt/Harbor/Code/keys/avb.vsf}
# The footer's template: the gate dumped by `save`, else the 2026-11-13 one in keys/akita.
AVB_TEMPLATE=${AVB_TEMPLATE:-}
# Armed 60 s after the reset (the hv's counter runs about 20 s ahead of Android's uptime): past the camera service's boot probe, before the owner can have unlocked and opened anything.
ARM_MS=60000
CAP=$((1 << 15))
MKIMG=$W/cargo/debug/ferros-mkimg
OI=$W/oracle-i2c/release/oracle-i2c
CO=$W/cam-oracle/release/cam-oracle

# The 42 front-end pages a camera open touches (the Pro's census, 2026-10-19) and the PHY's first page (the light build of 2026-11-15 trapped G#1C10_0000-G#1C10_4000).
ISP_PAGES="G#1C00_0000,G#1C00_1000,G#1C00_3000,G#1C00_7000,G#1C02_0000,G#1C04_0000,G#1C05_0000,G#1C06_0000,G#1C07_0000,G#1C08_0000,G#1C09_0000,G#1C0A_0000,G#1C0B_0000,G#1C0C_0000,G#1C0D_0000,G#1C0E_0000,G#1C0F_0000,G#1C10_0000,G#1C10_1000,G#1C10_2000,G#1C10_3000,G#1C32_0000,G#1C32_2000,G#1C32_8000,G#1C32_9000,G#1C33_0000,G#1C33_1000,G#1C33_3000,G#1C33_5000,G#1C33_7000,G#1C33_8000,G#1C33_9000,G#1C33_B000,G#1C33_C000,G#1C33_D000,G#1C33_E000,G#1C35_0000,G#1C39_0000,G#1C39_1000,G#1C39_2000,G#1C39_3000,G#1C39_4000,G#1C39_5000"
# The ultrawide's frames did not go thru the PDMA program on 2026-11-15: every page of the front end's span, so whatever carries them is in the log.
ISP_SPAN="G#1C00_0000-G#1C3F_F000"

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
    *) die "no image $1 (i2c-main, i2c-uw, i2c-front, isp, isp-uw)";;
    esac
    # One controller, its sensor alone (every akita sensor answers at G#1A; the main's lens, OIS and EEPROM at G#24 and the others' EEPROMs at G#50 stay out), the controller's nine setup and status words quiet.
    local q="" o
    for o in 0000 0004 0008 0020 0024 0030 0040 0048 0050; do q="$q,G#${ctrl}_$o"; done
    echo "FERROS_HV_ORACLE_PAGES=G#${ctrl}_0000 FERROS_HV_ORACLE_I2C_TARGET=G#1A FERROS_HV_ORACLE_QUIET=${q#,}"
}

session_image() {
    case $1 in
    i2c-main | i2c-uw | i2c-front) echo "$1";;
    isp-main | isp-front | isp-main-max) echo isp;;
    isp-uw) echo isp-uw;;
    *) die "no session $1 (i2c-main, i2c-uw, i2c-front, isp-main, isp-front, isp-main-max, isp-uw)";;
    esac
}

session_ctrl() {
    case $1 in
    i2c-main) echo G#1088_0000;;
    i2c-front) echo G#1089_0000;;
    i2c-uw) echo G#108A_0000;;
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
        say "root refused: the owner opens the Magisk app and grants Shell (Superuser), or unlocks the phone if it is locked; Enter when done"
        read -r _ </dev/tty
    done
}
mark() { A shell "su -c 'echo \"<3>ferros-mark: $1\" > /dev/kmsg'" >/dev/null 2>&1; }
# One step: the owner's instruction, Enter, the mark, then the owner acts; `s` skips it (marked as skipped).
step() {
    local m=$1; shift
    echo
    say "$*"
    local r
    read -r -p "   Enter, then do it (s + Enter: skip) " r </dev/tty
    if [ "$r" = s ]; then mark "$m: skipped"; return 1; fi
    mark "$m"
}
done_step() {
    local r
    read -r -p "   Enter when it is done " r </dev/tty
    mark "$1: end"
}
hold() { say "keep it running: $1 s"; sleep "$1"; }

to_fastboot() {
    need_port
    if ferros_here; then timeout 60 "$B" reboot fastboot >/dev/null 2>&1 || true
    elif device_pin akita 2>/dev/null && A get-state >/dev/null 2>&1; then A reboot bootloader || true; fi
    local i
    for i in $(seq 90); do device_pin akita 2>/dev/null && fastboot devices | grep -q . && break; sleep 2; done
    device_pin akita && fastboot devices | grep -q . || die "no fastboot: if the 8a shows fastboot but answers nothing, power + volume down by hand"
}
# fastboot's own words, and a stop on FAILED (a whole night once ran on a stale build behind a hidden failed flash).
flash_boot_b() {
    local out
    out=$(fastboot flash boot_b "$1" 2>&1)
    echo "$out"
    if echo "$out" | grep -q FAILED || ! echo "$out" | grep -q OKAY; then die "boot_b NOT written"; fi
}

cmd_build() {
    [ -f "$LINUX" ] || die "no Android kernel at $LINUX"
    local tmpl=$AVB_TEMPLATE
    [ -n "$tmpl" ] || { [ -f "$C/back/boot_b.img" ] && tmpl=$C/back/boot_b.img || tmpl=/mnt/Harbor/Code/keys/akita/images/avb_template_boot_b_2026-11-13.img; }
    [ -f "$tmpl" ] || die "no AVB template at $tmpl"
    mkdir -p "$W"
    CARGO_TARGET_DIR=$W/cargo cargo build -q -p ferros-mkimg
    local img k
    for img in i2c-main i2c-uw i2c-front isp isp-uw; do
        k=$(image_knobs $img)
        # As the gate's hv (akita's board, the hv core) plus stage 2, the oracle, no PMU reads, the oracle's knobs, armed at ARM_MS, CAP lines at most; built in a target of its own so no other session's hv build is taken for it.
        env $k FERROS_HV_BOARD=akita FERROS_HV_CORE=1 FERROS_HV_STAGE2=1 FERROS_HV_ORACLE=1 FERROS_HV_NO_PMU_READ=1 \
            FERROS_HV_ORACLE_CAP=$CAP FERROS_HV_ORACLE_ARM_AFTER_MS=$ARM_MS \
            CARGO_TARGET_DIR=$W/cargo cargo build -q -p ferros_hv --target aarch64-unknown-none-softfloat --release
        aarch64-linux-gnu-objcopy -O binary "$W/cargo/aarch64-unknown-none-softfloat/release/ferros_hv" "$W/hv-$img.bin"
        [ "$(stat -c %s "$W/hv-$img.bin")" -le $((0x180000)) ] || die "hv-$img is over its G#18_0000 pad"
        # The hv alone in front of Android (no ferros packed, no gate); the console quiet from boot (pstore integrity) and the image's name on the command line, which `capture` checks.
        "$MKIMG" gate --hv "$W/hv-$img.bin" --image "$LINUX" --cmdline "loglevel=4 ferros_cap.image=$img" -o "$W/$img.bare.img"
        # Whole partition with its own footer (a bare 20 MB image buries Google's vbmeta and ABL refuses the slot, 2026-11-06).
        "$MKIMG" avb-footer --key "$AVB_KEY" --google-boot "$tmpl" --image "$W/$img.bare.img" -o "$W/$img.img"
        say "$img: $W/$img.img ($(stat -c %s "$W/$img.img") bytes, blake3 $(b3sum --no-names "$W/$img.img" | cut -c1-16)); hv knobs: $k"
    done
    CARGO_TARGET_DIR=$W/oracle-i2c cargo build -q --release --manifest-path tools/oracle-i2c/Cargo.toml
    CARGO_TARGET_DIR=$W/cam-oracle cargo build -q --release --manifest-path tools/cam-oracle/Cargo.toml
    say "built; the template was $tmpl"
}

cmd_save() {
    need_port
    ferros_here || die "the 8a is not on ferros (FERROS_PORT=${FERROS_PORT:-unset})"
    [ -x "$B" ] || cargo build -q -p ferros-bridge
    mkdir -p "$C/back"
    local p
    for p in boot_b boot_a; do
        "$B" part dump $p "$C/back/$p.img"
        "$B" part dump $p "$C/back/$p.again"
        cmp "$C/back/$p.img" "$C/back/$p.again" || die "$p read twice differs"
        rm "$C/back/$p.again"
    done
    "$B" part list > "$C/back/partitions.txt"
    ( cd "$C/back" && b3sum boot_b.img boot_a.img > B3SUMS )
    # The gate's Android kernel: the hv is padded to G#18_0000 in the boot image's kernel, the Linux Image right after it.
    CARGO_TARGET_DIR=$W/cargo cargo build -q -p ferros-mkimg
    "$MKIMG" unpack-kernel --boot "$C/back/boot_b.img" -o "$W/boot_b.blob"
    tail -c +$((0x180000 + 1)) "$W/boot_b.blob" | cmp -n "$(stat -c %s "$LINUX")" - "$LINUX" >/dev/null \
        || die "slot b's gate carries another Android kernel than $LINUX: extract it from $W/boot_b.blob before building"
    say "slot b's gate carries $LINUX; its hv: $(head -c $((0x180000)) "$W/boot_b.blob" | strings -n 3 | grep -A1 -m1 'Linux at EL1; build' | tr '\n' ' ')"
    say "saved: $C/back (boot_b.img is what restore flashes)"
}

cmd_rehearse() {
    need_port
    if ferros_here; then
        [ -x "$B" ] || cargo build -q -p ferros-bridge
        "$B" diag "android here" || die "the gate did not take 'android here'"
    fi
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
    say "dumpsys: $(grep -c -i 'MaximumResolution' "$C/dumpsys-media-camera.txt") maximum-resolution lines, $(grep -c -i 'physicalIds\|physical camera' "$C/dumpsys-media-camera.txt") physical-camera lines -> $C/dumpsys-media-camera.txt"
    step "rehearsal lumis" "the owner: open Lumis (allow the camera if it asks); on its menu, count each lens's entries (green = maximum resolution, red = cropped) and tell the agent the numbers; then open the main's first entry and leave it open" || true
    if [ "${AKITA_CLOCKS:-1}" = 1 ]; then
        # The master clocks while a camera streams, one clock at a time (debugfs's clk_summary resets this phone, 2026-11-15).
        A shell "su -c 'mountpoint -q /sys/kernel/debug || mount -t debugfs debugfs /sys/kernel/debug'" >/dev/null 2>&1 || true
        local c
        for c in $(A shell "su -c 'ls /sys/kernel/debug/clk'" 2>/dev/null | tr -d '\r' | grep -i -E 'cis_clk|cis-clk|ciscl' | head -16); do
            echo "$c $(A shell "su -c 'cat /sys/kernel/debug/clk/$c/clk_rate'" 2>/dev/null | tr -d '\r')"
        done | tee "$C/cis-clocks.txt"
    fi
    done_step "rehearsal lumis"
    step "rehearsal pixel camera" "the owner: close Lumis (EXIT, then home); the script opens the Pixel Camera app; answer any first-run question yourself" || true
    A shell am start -a android.media.action.STILL_IMAGE_CAMERA >/dev/null 2>&1 || true
    done_step "rehearsal pixel camera"
    A shell input keyevent KEYCODE_HOME >/dev/null 2>&1 || true
    say "rehearsal done: $C/rehearsal.txt. Next: $0 flash i2c-main (from here: Android goes to fastboot)"
}

cmd_flash() {
    local img=${1:?usage: $0 flash <i2c-main|i2c-uw|i2c-front|isp|isp-uw>}
    image_knobs "$img" >/dev/null
    [ -f "$W/$img.img" ] || die "no $W/$img.img: run build first"
    [ -f "$C/back/boot_b.img" ] || die "no $C/back/boot_b.img: run save first (restore needs it)"
    to_fastboot
    local slot
    slot=$(fastboot getvar current-slot 2>&1 | awk '/^current-slot:/ {print $2}')
    [ "$slot" = b ] || die "the current slot is '${slot:-?}', not b: stop; the owner decides (this script never sets a slot active)"
    if [ ! -f "$C/pkvm.txt" ]; then fastboot oem pkvm status > "$C/pkvm.txt" 2>&1 || true; say "pKVM: $(grep -i pkvm "$C/pkvm.txt" | head -1) (recorded, not changed)"; fi
    flash_boot_b "$W/$img.img"
    fastboot reboot >/dev/null 2>&1
    echo "$img" > "$C/flashed"
    say "boot_b holds $img; Android boots under it (no gate). Next: $0 capture <session>"
}

# The sessions. Lumis first; the Pixel Camera app last in a session (its preview's per-frame exposure writes fill the ring fastest).
steps_i2c-main() {
    step "lumis main binned" "open Lumis, the main lens's first entry (binned, the whole field)" && { hold 6; done_step "lumis main binned"; }
    step "lumis main maxres" "EXIT to the menu; open the main's maximum-resolution entry (green), if Lumis lists one" && { hold 6; done_step "lumis main maxres"; }
    step "lumis main crop" "EXIT; open the main's cropped entry (red: the 2x), if Lumis lists one" && { hold 6; done_step "lumis main crop"; }
    step "lumis main 16s" "EXIT; open the main's first entry again, drag SHUTTER to 16 s, wait until a whole 16 s frame has landed (about 35 s), then EXIT; never a volume key (Lumis saves; the hv kills on both)" && done_step "lumis main 16s"
    if step "pixel camera main" "leave Lumis (home); the script opens the Pixel Camera app on 1x for 5 s; then tap 2x yourself and hold 5 s"; then
        A shell am start -a android.media.action.STILL_IMAGE_CAMERA >/dev/null 2>&1 || true
        hold 5; mark "pixel camera main 2x"; done_step "pixel camera main"
    fi
}
steps_i2c-uw() {
    step "lumis uw binned" "open Lumis, the ultrawide's first entry" && { hold 6; done_step "lumis uw binned"; }
    step "lumis uw maxres" "EXIT; the ultrawide's maximum-resolution entry, if listed" && { hold 6; done_step "lumis uw maxres"; }
    step "lumis uw 16s" "EXIT; the ultrawide's first entry, SHUTTER to 16 s, wait for one whole frame (about 35 s), EXIT" && done_step "lumis uw 16s"
    if step "pixel camera uw" "home; the script opens the Pixel Camera app and taps .5; after 5 s switch to Video yourself and hold 5 s"; then
        A shell am start -a android.media.action.STILL_IMAGE_CAMERA >/dev/null 2>&1 || true
        sleep 3; A shell input tap 414 1513 >/dev/null 2>&1 || true
        hold 5; mark "pixel camera uw video"; done_step "pixel camera uw"
    fi
}
steps_i2c-front() {
    step "lumis front binned" "open Lumis, the front lens's first entry" && { hold 6; done_step "lumis front binned"; }
    step "lumis front maxres" "EXIT; the front's maximum-resolution entry, if listed" && { hold 6; done_step "lumis front maxres"; }
    step "lumis front 1s" "EXIT; the front's first entry, SHUTTER to its longest (1 s), wait 5 s, EXIT" && done_step "lumis front 1s"
    if step "pixel camera front" "home; the script opens the Pixel Camera app and flips to the front; after 5 s switch to Video yourself and hold 5 s, then flip back to the rear (the app reopens the camera used last)"; then
        A shell am start -a android.media.action.STILL_IMAGE_CAMERA >/dev/null 2>&1 || true
        sleep 3; A shell input tap 859 1796 >/dev/null 2>&1 || true
        hold 5; mark "pixel camera front video"; done_step "pixel camera front"
    fi
}
# An ISP session holds one camera's open: the PDMA dump is taken once a boot (its 16th frame), and the front end's per-frame answers fill the cap in seconds.
steps_isp-main() { step "lumis main binned" "open Lumis, the main lens's first entry" && { hold 8; done_step "lumis main binned"; }; }
steps_isp-front() { step "lumis front binned" "open Lumis, the front lens's first entry" && { hold 8; done_step "lumis front binned"; }; }
steps_isp-uw() { step "lumis uw binned" "open Lumis, the ultrawide's first entry" && { hold 8; done_step "lumis uw binned"; }; }
steps_isp-main-max() { step "lumis main maxres" "open Lumis, the main's maximum-resolution entry" && { hold 8; done_step "lumis main maxres"; }; }

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
    *) die "no session $s";;
    esac
    ( cd "$C" && b3sum "$s".* 2>/dev/null | grep -v -e '\.console-ramoops-0$' -e '\.log$' >> B3SUMS ) || true
}

cmd_restore() {
    [ -f "$C/back/boot_b.img" ] || die "no $C/back/boot_b.img"
    to_fastboot
    local slot
    slot=$(fastboot getvar current-slot 2>&1 | awk '/^current-slot:/ {print $2}')
    [ "$slot" = b ] || die "the current slot is '${slot:-?}', not b: stop; the owner decides"
    flash_boot_b "$C/back/boot_b.img"
    fastboot reboot >/dev/null 2>&1
    rm -f "$C/flashed"
    local i
    for i in $(seq 60); do ferros_here && { say "ferros on USB after $((i * 2)) s (through the gate); the identity unlocks at boot"; return 0; }; sleep 2; done
    die "no ferros on USB after 120 s: the gate shows on the panel? volume down there would boot Android; else fastboot by hand"
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
*) sed -n '2,21p' "$0"; exit 2;;
esac
