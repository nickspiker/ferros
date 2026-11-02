#!/usr/bin/env bash
# Android on husky after a factory reset: everything POTS, SMS and MMS switched off, data left working, and no way to dial, 911 included (owner, 2026-11-02: "SMS/MMS/POTS -> GONE"; MVNO is data-only, no 911). Also the OTA updater (BOOT.md: it writes the inactive slot, which is ferros's; the UFS firewall refuses it now, this is belt and braces).
# Packages are disabled for the user, not uninstalled, so `pm enable` brings any back. Needs root (Magisk, superuser granted to ADB) and Android up over USB.
# Kept on purpose: com.android.phone (hosts the data stack), the telephony provider, telecom (system), carrier config and setup (APNs), the Samsung RIL hooks, cell broadcasts (emergency alerts are not POTS).
# Limit: with the dialer, Emergency SOS and IMS gone there is no way for a person to dial, and the lock screen's Emergency button opens nothing. The modem itself can still attach for an emergency call if something privileged asks telecom; the hard guarantee is the radio's own configuration under ferros, later.
# Usage: tools/husky-debloat.sh [--restore]
set -uo pipefail
GONE=(
    com.google.android.dialer            # Phone, the emergency dialer, visual voicemail
    com.google.android.apps.messaging    # Messages (SMS, MMS, RCS)
    com.android.mms.service              # MMS
    com.shannon.imsservice               # IMS: VoLTE, VoWiFi, SMS over IMS
    com.shannon.rcsservice               # RCS
    com.android.telephony.imsmedia       # IMS voice media
    com.android.imsserviceentitlement    # IMS entitlement
    com.android.stk                      # SIM toolkit
    com.android.emergency                # Emergency SOS and information
    com.google.android.apps.carrier.log  # carrier logging
)
sudo_sh() { adb -d shell "su -c \"$1\"" 2>&1 | grep -v SuCallbackHandler; }
if [ "${1:-}" = "--restore" ]; then
    for p in "${GONE[@]}"; do sudo_sh "pm enable --user 0 $p"; done
    sudo_sh "pm enable com.google.android.gms/com.google.android.gms.update.SystemUpdateService; settings put global ota_disable_automatic_update 0"
    exit 0
fi
for p in "${GONE[@]}"; do
    sudo_sh "pm disable-user --user 0 $p" | sed 's/^/   /'
done
sudo_sh "pm disable com.google.android.gms/com.google.android.gms.update.SystemUpdateService; settings put global ota_disable_automatic_update 1" | sed 's/^/   /'
echo "== disabled now:"
adb -d shell 'pm list packages -d' | tr -d '\r' | sed 's/package:/   /' | sort
echo "== data:"
adb -d shell 'dumpsys telephony.registry' | tr -d '\r' | grep -m2 -E "mDataConnectionState|mDataActivity" | sed 's/^ */   /'
