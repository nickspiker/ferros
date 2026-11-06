# ferros's device guard (owner, 2026-11-06: "make sure each tool checks the device type so we don't go flash husky on whatever this one's called"). Source it, then `device_pin <product>` (husky, akita) before any fastboot or adb call.
# device_pin finds the phone whose product is <product> (adb's ro.product.device, else fastboot's getvar product, else the serial it was last seen with, for a phone running ferros that neither sees) and pins it in ANDROID_SERIAL, which fastboot and adb both honour: with two phones on the host, every call goes to that one.
# The `fastboot` function below then asks the phone for its product again before anything that writes or reboots, and refuses unless it is the pinned one. `fastboot devices` lists the pinned phone only, so a wait loop cannot take the other phone in fastboot for this one.

FERROS_DEVICE=""
FERROS_DEVICES_DIR=/mnt/Harbor/Code/ferros/target/devices

device_pin() {
    local want=$1 s p
    mkdir -p "$FERROS_DEVICES_DIR"
    for s in $(command adb devices 2>/dev/null | awk 'NR > 1 && $2 == "device" {print $1}'); do
        p=$(command adb -s "$s" shell getprop ro.product.device 2>/dev/null | tr -d '\r')
        if [ "$p" = "$want" ]; then
            device_pinned "$want" "$s"
            return 0
        fi
    done
    for s in $(command fastboot devices 2>/dev/null | awk '{print $1}'); do
        p=$(command fastboot -s "$s" getvar product 2>&1 | awk '/^product:/ {print $2}')
        if [ "$p" = "$want" ]; then
            device_pinned "$want" "$s"
            return 0
        fi
    done
    if [ -s "$FERROS_DEVICES_DIR/$want.serial" ]; then
        device_pinned "$want" "$(cat "$FERROS_DEVICES_DIR/$want.serial")"
        echo "== $want: not in adb or fastboot; pinned by the serial last seen (it may be running ferros)" >&2
        return 0
    fi
    echo "no $want on adb or fastboot, and none seen before: refusing to guess" >&2
    return 1
}

device_pinned() {
    FERROS_DEVICE=$1
    export ANDROID_SERIAL=$2
    echo "$2" > "$FERROS_DEVICES_DIR/$1.serial"
}

# fastboot, held to the pinned phone. `devices` and `getvar` pass through (listing only the pinned serial); everything else first checks the phone's product, then runs with its serial.
fastboot() {
    if [ -z "$FERROS_DEVICE" ]; then
        echo "fastboot: no device pinned (device_pin <product>): refused" >&2
        return 1
    fi
    # A caller's own -s must name the pinned phone.
    if [ "${1:-}" = -s ]; then
        if [ "$2" != "$ANDROID_SERIAL" ]; then
            echo "fastboot: -s $2 is not the pinned $FERROS_DEVICE ($ANDROID_SERIAL): refused" >&2
            return 1
        fi
        shift 2
    fi
    case "${1:-}" in
        devices)
            command fastboot devices | grep "^$ANDROID_SERIAL"
            return
            ;;
        getvar | --version | help)
            command fastboot -s "$ANDROID_SERIAL" "$@"
            return
            ;;
    esac
    local p
    # A bounded ask: fastboot waits for an absent serial forever, and then this would hang rather than refuse.
    p=$(timeout 15 fastboot -s "$ANDROID_SERIAL" getvar product 2>&1 | awk '/^product:/ {print $2}')
    if [ "$p" != "$FERROS_DEVICE" ]; then
        echo "fastboot $*: the phone in fastboot is '${p:-none}', not $FERROS_DEVICE: refused" >&2
        return 1
    fi
    command fastboot -s "$ANDROID_SERIAL" "$@"
}
