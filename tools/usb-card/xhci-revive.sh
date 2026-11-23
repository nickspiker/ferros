#!/usr/bin/env bash
# Revive a USB host controller the kernel gave up on, the moment it says so.
#
# Follows the kernel log (journalctl -k -f: woken by each new line, no polling) and, on "xhci_hcd <pci>: HC died", unbinds and rebinds that controller, as a person would by hand.
# Rebinding enumerates its hubs and devices again from scratch; adb and the bridge find the devices by serial, so nothing else needs telling.
# Run as root by xhci-revive.service (tools/usb-card/install.sh).
set -u
DRV=/sys/bus/pci/drivers/xhci_hcd

journalctl -k -f -n 0 -o cat | while IFS= read -r line; do
    case "$line" in
        *"xhci_hcd "*": HC died"*)
            pci=${line#*xhci_hcd }
            pci=${pci%%: HC died*}
            logger -t xhci-revive "controller $pci died: unbinding and binding it again"
            echo "$pci" > "$DRV/unbind"
            echo "$pci" > "$DRV/bind"
            logger -t xhci-revive "controller $pci bound again"
            ;;
    esac
done
