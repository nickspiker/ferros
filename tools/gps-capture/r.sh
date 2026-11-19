#!/system/bin/sh
# Runs gcap.sh as root (Magisk's su), so the host's adb line names only this script.
exec su -c "sh /data/local/tmp/gcap.sh $*"
