#!/usr/bin/env bash
# Install the USB card's keep-awake rule and the dead-controller reviver on the dev box (root).
#
#   sudo tools/usb-card/install.sh
set -eu
here=$(dirname "$(readlink -f "$0")")
install -m 0644 "$here/99-asm1042-awake.rules" /etc/udev/rules.d/99-asm1042-awake.rules
install -m 0755 "$here/xhci-revive.sh" /usr/local/sbin/xhci-revive.sh
install -m 0644 "$here/xhci-revive.service" /etc/systemd/system/xhci-revive.service
udevadm control --reload
# Apply the rule to the card and its hubs now, not only at the next plug or boot.
udevadm trigger --action=bind --subsystem-match=pci --attr-match=vendor=0x1b21 --attr-match=device=0x1042
udevadm trigger --action=bind --subsystem-match=usb
systemctl daemon-reload
systemctl enable --now xhci-revive.service
echo "installed: udev rule, /usr/local/sbin/xhci-revive.sh, xhci-revive.service (running)"
