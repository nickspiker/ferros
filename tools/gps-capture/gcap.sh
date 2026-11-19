#!/system/bin/sh
# Device side of the Pro's GPS captures (docs/GPS.md "Android's aided session"), run as root (Magisk su) on its Android: gpsd's own log at LOG_DEBUG, lhd under tools/gtrace, a session started, the files gathered.
# Usage: gcap.sh info | xml <host|onchip> | run <tag> <seconds> | onchip | restore | gather <tag>
D=/data/local/tmp/gcap
mkdir -p $D
X=/vendor/etc/gnss/gps.xml
# lhd's command line as init starts it (its service line in the vendor init files).
lhd_cmd() { grep -rh "^service .*lhd" /vendor/etc/init/ | head -1 | sed 's/^service [^ ]* //'; }
lhd_svc() { grep -rh "^service .*lhd" /vendor/etc/init/ | head -1 | awk '{print $2}'; }
gpsd_svc() { grep -rh "^service .*gpsd" /vendor/etc/init/ | head -1 | awk '{print $2}'; }
case "$1" in
info)
    echo "== services"; grep -rh -A6 -E "^service .*(lhd|gpsd|scd)" /vendor/etc/init/
    echo "== state"; getprop | grep -E "init.svc.*(lhd|gpsd|scd|gnss)"
    echo "== /data/vendor/gps"; ls -la /data/vendor/gps/
    echo "== ota_disable_automatic_update $(settings get global ota_disable_automatic_update)"
    echo "== location mode $(settings get secure location_mode)"
    echo "== gps.xml"; ls -laZ $X; grep -n -E "Log=|LogPriMask|IgnoreOnChip|SuplEnable|acSuplServer|HttpSyncLto|IgnoreInjected" $X
    ;;
xml)
    [ -f $D/gps.xml.orig ] || cp $X $D/gps.xml.orig
    sed -e 's/Log="JAVA"/Log="LOGCAT"/' -e 's/LogPriMask="LOG_INFO"/LogPriMask="LOG_DEBUG"/' $D/gps.xml.orig > $D/gps.xml
    [ "$2" = onchip ] && sed -i -e 's/^\( *\)<hal$/\1<hal IgnoreOnChipRequest="false"/' $D/gps.xml
    chmod 644 $D/gps.xml
    chcon u:object_r:vendor_configs_file:s0 $D/gps.xml
    umount $X 2>/dev/null
    mount -o bind $D/gps.xml $X
    grep -n -E "Log=|LogPriMask|IgnoreOnChip" $X
    ;;
run)
    # The HAL and lhd stopped, lhd started under gtrace for $3 seconds, then gpsd: a chip restarted from cold with gpsd's own stored aiding.
    setprop ctl.stop "$(gpsd_svc)"; setprop ctl.stop "$(lhd_svc)"
    # A traced lhd of an earlier run is not init's: ended by hand (gtrace flushes as its program ends).
    pkill -f /vendor/bin/hw/lhd; sleep 2; pkill gtrace; sleep 1
    echo "lhd: $(lhd_cmd)"
    chmod 755 /data/local/tmp/gtrace; setsid nohup /data/local/tmp/gtrace $D/$2.trace $3 $(lhd_cmd) > $D/$2.gtrace.out 2>&1 &
    sleep 1
    setprop ctl.start "$(gpsd_svc)"
    sleep 1
    getprop | grep -E "init.svc.*(lhd|gpsd)"
    ;;
onchip)
    echo '$pglirm,req_breadcrumb,pvtOnChip,periodSec,1,batchOption,2' > /data/vendor/gps/glgpsctrl
    ;;
restore)
    pkill -f gtrace
    umount $X 2>/dev/null
    setprop ctl.stop "$(gpsd_svc)"; setprop ctl.stop "$(lhd_svc)"; sleep 1
    setprop ctl.start "$(lhd_svc)"; setprop ctl.start "$(gpsd_svc)"
    getprop | grep -E "init.svc.*(lhd|gpsd)"
    ;;
seen)
    # The gps.xml gpsd itself sees (its mount namespace), and lhd's.
    for p in $(pidof gpsd) $(pidof lhd); do echo "pid $p:"; grep -n -E "Log=|LogPriMask|IgnoreOnChip" /proc/$p/root$X; done
    ;;
binaries)
    # gpsd, lhd and the chip's patch, for reading gpsd's builders (Google's and Broadcom's: keys/ only).
    cp /vendor/bin/hw/gpsd /vendor/bin/hw/lhd $D/
    cp /vendor/firmware/SensorHub.patch $D/ 2>/dev/null
    chmod 644 $D/gpsd $D/lhd $D/SensorHub.patch
    ;;
gather)
    cp /data/vendor/gps/lto2.dat $D/$2.lto2.dat 2>/dev/null
    cp /data/vendor/gps/gldata.sto $D/$2.gldata.sto 2>/dev/null
    ls -la /data/vendor/gps/ > $D/$2.gpsdir.txt
    chmod 644 $D/$2.* 2>/dev/null
    ls -la $D/
    ;;
esac
