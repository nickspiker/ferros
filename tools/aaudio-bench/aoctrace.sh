#!/system/bin/sh
# aoctrace.sh on|off|dump <out>: every AP-to-AoC message (aoc.h:200 aoc_service_write: x0 the service device, whose first word is its kobject's name pointer; x1 the buffer, x2 the length), 1 KiB of each, thru tracefs kprobes (root). Mirrors SENSORS.md "Capturing USF".
T=/sys/kernel/tracing
[ -d $T/events ] || T=/sys/kernel/debug/tracing
case $1 in
on)
  echo 0 > $T/tracing_on; echo > $T/trace; echo 65536 > $T/buffer_size_kb
  for fn in aoc_service_write aoc_service_write_timeout; do
    grep -qw "$fn" /proc/kallsyms && echo "p:aoctx_$fn $fn len=%x2 svc=+0(+0(%x0)):string a=+0(%x1):x32[64] b=+256(%x1):x32[64] c=+512(%x1):x32[64] d=+768(%x1):x32[64]" >> $T/kprobe_events
  done
  for e in $T/events/kprobes/aoctx_*; do echo 1 > $e/enable; done
  echo 1 > $T/tracing_on; cat $T/kprobe_events ;;
mark) echo "$2" > $T/trace_marker ;;
off) echo 0 > $T/tracing_on; for e in $T/events/kprobes/aoctx_*; do echo 0 > $e/enable; done; echo > $T/kprobe_events ;;
dump) cat $T/trace > "$2" ;;
esac
