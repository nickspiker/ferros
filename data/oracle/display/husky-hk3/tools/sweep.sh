#!/system/bin/sh
# Display oracle sweep v2 (husky, Android, root). Modes pinned through SurfaceFlinger (1035 = setActiveMode: 0 WQHD60, 1 WQHD120, 2 FHD120, 3 FHD60); every DSI packet of each transition traced (dpu+panel dsi_tx, with bytes and delays); panel attrs, DPU register dump, TE/frame rates and battery current captured per state. Restores the user's state at the end.
O=/data/local/tmp/oracle2; rm -rf $O; mkdir -p $O
T=/sys/kernel/tracing; P=/sys/class/drm/card0-DSI-1/panel; D=/sys/kernel/debug/dri/0; B=/sys/class/backlight/panel0-backlight
mode0=$(dumpsys display | grep -o "mActiveModeId=[0-9]*" | head -1 | cut -d= -f2); idle0=$(cat $P/panel_idle); minv0=$(cat $P/min_vrefresh); ophz0=$(cat $P/op_hz)
echo "mode=$mode0 panel_idle=$idle0 min_vrefresh=$minv0 op_hz=$ophz0 brightness=$(settings get system screen_brightness) doze=$(settings get secure doze_always_on)" > $O/restore.txt
echo 0 > $T/tracing_on; echo 32768 > $T/buffer_size_kb; echo nop > $T/current_tracer; echo 1 > $T/events/dpu/enable; echo 1 > $T/events/panel/enable
tb() { echo > $T/trace; echo 1 > $T/tracing_on; }
te() { echo 0 > $T/tracing_on; cat $T/trace > $O/$1/dsi-trace.txt; }
cap() {
  mkdir -p $O/$1
  for a in refresh_rate op_hz power_state panel_idle min_vrefresh te2_timing idle_delay_ms time_in_state; do echo "$a = $(cat $P/$a 2>/dev/null | tr '\n' '|')"; done > $O/$1/panel.txt
  echo "backlight = $(cat $B/brightness) actual = $(cat $B/actual_brightness) hbm_mode = $(cat $B/hbm_mode 2>/dev/null) dimming_on = $(cat $B/dimming_on 2>/dev/null) state = $(cat $B/state 2>/dev/null)" >> $O/$1/panel.txt
  dumpsys display 2>/dev/null | grep -o "mActiveSfDisplayMode=DisplayMode{id=[0-9]*, width=[0-9]*, height=[0-9]*, [^}]*peakRefreshRate=[0-9.]*" | head -1 > $O/$1/mode.txt
  cat $D/crtc-0/reg_dump > $O/$1/dpu-regdump.txt 2>&1
  t0=$(cat $D/crtc-0/te_count); f0=$(cat $D/crtc-0/frame_done_count); i=0; : > $O/$1/current_ua.txt
  while [ $i -lt 10 ]; do cat /sys/class/power_supply/battery/current_now >> $O/$1/current_ua.txt; sleep 0.2; i=$((i+1)); done
  t1=$(cat $D/crtc-0/te_count); f1=$(cat $D/crtc-0/frame_done_count)
  echo "te_per_2s = $((t1-t0)) frames_per_2s = $((f1-f0))" > $O/$1/rates.txt
}
st() { n=$1; shift; mkdir -p $O/$n; tb; "$@" > $O/$n/cmd.txt 2>&1; sleep 3; te $n; cap $n; echo "state $n: rr=$(cat $P/refresh_rate) $(cat $O/$n/rates.txt)"; }
sf() { service call SurfaceFlinger 1035 i32 $1 > /dev/null; }
input keyevent KEYCODE_WAKEUP; sleep 2; cmd display set-brightness 140 --unit nits; sleep 1
# Modes with idle off: the register state and TE rate of the mode itself.
echo 0 > $P/panel_idle; sleep 1
st W120_noidle sf 1
st W60_noidle  sf 0
st F120_noidle sf 2
st F60_noidle  sf 3
# Idle on: the self-refresh / auto-rate states Android lives in; captured after 6 s of stillness.
echo 1 > $P/panel_idle; sleep 1
for m in 1 0 2 3; do case $m in 1) n=W120_idle;; 0) n=W60_idle;; 2) n=F120_idle;; 3) n=F60_idle;; esac; mkdir -p $O/$n; sf $m; sleep 1; tb; sleep 7; te $n; cap $n; echo "state $n: rr=$(cat $P/refresh_rate) $(cat $O/$n/rates.txt)"; done
sf 1; sleep 2
st IDLE_MIN10 sh -c "echo 10 > $P/min_vrefresh; sleep 7"
st IDLE_MIN30 sh -c "echo 30 > $P/min_vrefresh; sleep 7"
echo $minv0 > $P/min_vrefresh; sleep 1
# NS (op_hz 60) mode, at 60 Hz
sf 0; sleep 2
st OPHZ60_NS sh -c "echo 60 > $P/op_hz"
st OPHZ120_HS sh -c "echo 120 > $P/op_hz"
sf 1; sleep 2
# Brightness
st BR_2nits    cmd display set-brightness 2 --unit nits
st BR_140nits  cmd display set-brightness 140 --unit nits
st BR_1000nits cmd display set-brightness 1000 --unit nits
st BR_HBM_on   sh -c "echo 1 > $B/hbm_mode; echo 4095 > $B/brightness"
st BR_HBM_3700 sh -c "echo 3700 > $B/brightness"
st BR_HBM_off  sh -c "echo 0 > $B/hbm_mode; cmd display set-brightness 140 --unit nits"
st DIM_on      sh -c "echo 1 > $B/dimming_on"
st DIM_off     sh -c "echo 0 > $B/dimming_on"
# AOD
settings put secure doze_always_on 1; sleep 1
st AOD_enter input keyevent KEYCODE_SLEEP
sleep 5; cap AOD_settled
st AOD_exit input keyevent KEYCODE_WAKEUP
settings put secure doze_always_on 0; sleep 1
# Full off and on
st OFF input keyevent KEYCODE_SLEEP
sleep 3; cap OFF_settled
st ON input keyevent KEYCODE_WAKEUP
# restore
echo 0 > $T/events/dpu/enable; echo 0 > $T/events/panel/enable
sf $mode0; echo $idle0 > $P/panel_idle; echo $minv0 > $P/min_vrefresh; echo $ophz0 > $P/op_hz; echo 0 > $B/hbm_mode 2>/dev/null; settings put system screen_brightness 98; settings put secure doze_always_on 0
echo "SWEEP2 DONE; restored to mode $mode0"
