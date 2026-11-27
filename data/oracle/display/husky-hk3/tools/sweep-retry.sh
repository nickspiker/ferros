#!/system/bin/sh
# Retry: NS 60 Hz op mode (needs the panel at 60 first), AOD (with the framework owning brightness), then restore the user's state exactly.
O=/data/local/tmp/oracle3; rm -rf $O; mkdir -p $O
T=/sys/kernel/tracing; P=/sys/class/drm/card0-DSI-1/panel; D=/sys/kernel/debug/dri/0; B=/sys/class/backlight/panel0-backlight
echo 0 > $T/tracing_on; echo 32768 > $T/buffer_size_kb; echo 1 > $T/events/dpu/enable; echo 1 > $T/events/panel/enable
tb() { echo > $T/trace; echo 1 > $T/tracing_on; }
te() { echo 0 > $T/tracing_on; cat $T/trace > $O/$1/dsi-trace.txt; }
cap() { mkdir -p $O/$1; for a in refresh_rate op_hz power_state panel_idle min_vrefresh te2_timing; do echo "$a = $(cat $P/$a 2>/dev/null | tr '\n' '|')"; done > $O/$1/panel.txt; echo "backlight = $(cat $B/brightness) actual = $(cat $B/actual_brightness) hbm_mode = $(cat $B/hbm_mode) lp_state = $(cat $B/lp_state 2>/dev/null) state = $(cat $B/state 2>/dev/null)" >> $O/$1/panel.txt; dumpsys display | grep -o "mActiveSfDisplayMode=DisplayMode{id=[0-9]*, width=[0-9]*, height=[0-9]*, [^}]*peakRefreshRate=[0-9.]*" | head -1 > $O/$1/mode.txt; cat $D/crtc-0/reg_dump > $O/$1/dpu-regdump.txt 2>&1; }
st() { n=$1; shift; mkdir -p $O/$n; tb; "$@" > $O/$n/cmd.txt 2>&1; sleep 4; te $n; cap $n; echo "state $n: rr=$(cat $P/refresh_rate) power=$(cat $P/power_state) bl=$(cat $B/brightness) tx=$(grep -c dsi_tx $O/$n/dsi-trace.txt)"; }
sf() { service call SurfaceFlinger 1035 i32 $1 > /dev/null; }
input keyevent KEYCODE_WAKEUP; sleep 2
echo 0 > $B/hbm_mode; cmd display set-brightness 140 --unit nits; sleep 1; echo "bl after framework set: $(cat $B/brightness)"
echo 0 > $P/panel_idle; sf 0; sleep 4; echo "at W60: rr=$(cat $P/refresh_rate)"
st OPHZ60_NS sh -c "echo 60 > $P/op_hz; cat $P/op_hz"
st OPHZ120_HS sh -c "echo 120 > $P/op_hz; cat $P/op_hz"
sf 1; sleep 3; echo 1 > $P/panel_idle
settings put secure doze_always_on 1; sleep 2
st AOD_enter input keyevent KEYCODE_SLEEP
sleep 6; cap AOD_settled; echo "AOD settled: power=$(cat $P/power_state) bl=$(cat $B/brightness) lp=$(cat $B/lp_state 2>/dev/null)"
st AOD_exit input keyevent KEYCODE_WAKEUP
settings put secure doze_always_on 0
echo 0 > $T/events/dpu/enable; echo 0 > $T/events/panel/enable
# restore: the phone lived at 1008x2244 (SF mode 2 in 1035 numbering, 120 Hz with 60 as the policy's alternative), idle on, min_vrefresh 0, op_hz 120, brightness owned by the framework
sf 2; sleep 2; cmd display clear-user-preferred-display-mode; echo 1 > $P/panel_idle; echo 0 > $P/min_vrefresh; echo 120 > $P/op_hz; echo 0 > $B/hbm_mode; settings put system screen_brightness 98; settings put secure doze_always_on 0
sleep 2; echo "RESTORED: rr=$(cat $P/refresh_rate) idle=$(cat $P/panel_idle) minv=$(cat $P/min_vrefresh) ophz=$(cat $P/op_hz) mode=$(dumpsys display | grep -o "mActiveSfDisplayMode=DisplayMode{id=[0-9]*, width=[0-9]*, height=[0-9]*" | head -1)"
