#!/usr/bin/env bash
# Re-enumerate the phone's USB port without a replug.
#
# The dev box has one usable port (bus 3 port 3, USB 2.0 root port). At the phone's reboots into the bootloader (and the crash reboots of oracle captures) the host often reads the descriptors with -71 protocol errors a few times and then gives the port up ("usb3-port3: unable to enumerate USB device"); it stays dead until a physical unplug. Toggling the port's sysfs `disable` switch is the same as an unplug: the hub driver enumerates it again from scratch. Needs write access to the switch (udev rule /etc/udev/rules.d/99-usb3-port3-kick.rules, group wheel).
#
#   tools/usb-kick.sh              kick now
#   tools/usb-kick.sh --if-wedged  kick only if nothing is enumerated on the port and the kernel gave the port up since the last check (cheap: call it from wait loops)
# FERROS_USB_PORT (default 3-0:1.0/usb3-port3) and FERROS_USB_DEV (default 3-3) select the port.
set -u
PORT=${FERROS_USB_PORT:-3-0:1.0/usb3-port3}
DEV=${FERROS_USB_DEV:-3-3}
SW=/sys/bus/usb/devices/$PORT/disable
STATE=${XDG_RUNTIME_DIR:-/tmp}/ferros-usb-kick.last

kick() {
    [ -w "$SW" ] || { echo "usb-kick: $SW not writable (udev rule missing?)" >&2; return 1; }
    echo 1 > "$SW"; sleep 2; echo 0 > "$SW"
    date +%s > "$STATE"
    # The adb server can keep a stale view of the device across the re-enumeration (2026-10-20: lsusb showed the phone, adb listed nothing until a server restart); the next adb command starts a fresh one.
    adb kill-server >/dev/null 2>&1 || true
    echo "== USB port ${PORT##*/} kicked" >&2
}

if [ "${1:-}" = --if-wedged ]; then
    now=$(date +%s)
    last=$(cat "$STATE" 2>/dev/null || echo $((now - 60)))
    echo "$now" > "$STATE"
    [ -e /sys/bus/usb/devices/$DEV ] && exit 0
    if journalctl -k --since "@$last" --no-pager 2>/dev/null | grep -q "${PORT##*/}: unable to enumerate"; then
        sleep 3   # the phone may still be mid-transition: give it a moment, then kick
        [ -e /sys/bus/usb/devices/$DEV ] || kick
    fi
    exit 0
fi
kick
