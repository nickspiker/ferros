#!/usr/bin/env bash
# Waits until the Pro's Android gps provider has a location newer than uptime $1 seconds, then prints when (uptime), its satellites and accuracy: never the place.
S=38261FDJG0023A
while true; do
    l=$(adb -s $S shell dumpsys location 2>/dev/null | tr -d '\r' | grep -o 'Location\[gps[^]]*' | head -1)
    et=$(echo "$l" | grep -o 'et=+[0-9hms]*' | sed 's/et=+//')
    h=$(echo "$et" | grep -o '^[0-9]*h' | tr -d h); et=${et#*h}
    m=$(echo "$et" | grep -o '^[0-9]*m' | tr -d m); et=${et#*m}
    s=$(echo "$et" | grep -o '^[0-9]*s' | tr -d s)
    t=$(( ${h:-0} * 3600 + ${m:-0} * 60 + ${s:-0} ))
    up=$(adb -s $S shell cat /proc/uptime | cut -d. -f1)
    if [ "$t" -gt "$1" ]; then
        echo "gps location at uptime $t s (now $up s), $(echo "$l" | grep -o 'satellites=[0-9]*'), $(echo "$l" | grep -o 'hAcc=[0-9]*')"
        exit 0
    fi
    if [ "$up" -gt $(( $1 + 300 )) ]; then echo "none by uptime $up"; exit 1; fi
    sleep 1
done
