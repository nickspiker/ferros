#!/system/bin/sh
# husky-hv-dio-hunt.sh — runs ON THE PHONE as root (adb push, then `su -c "sh /data/local/tmp/dio-hunt.sh 600"`): ftrace on the block layer, f2fs direct I/O, the scheduler, IPIs, the UFS interrupt, softirqs, workqueues and cpu_idle; Mahere launched and panned by synthetic gestures; stops at the first Mahere thread stuck in blk_io_schedule on two polls and saves the trace, the thread stacks, the block-queue snapshot and the stuck thread's scheduler bookkeeping to /data/local/tmp/dio/. HV.md "2026-10-19, the direct-I/O hang" is the finding it was written for.
T=/sys/kernel/tracing; OUT=/data/local/tmp/dio; mkdir -p $OUT; rm -f $OUT/*
LIMIT=${1:-600}
echo 0 > $T/tracing_on; echo > $T/trace; echo nop > $T/current_tracer; echo 12288 > $T/buffer_size_kb
for e in block_bio_queue block_bio_remap block_getrq block_rq_insert block_rq_issue block_rq_complete block_bio_complete block_rq_error block_split; do echo 1 > $T/events/block/$e/enable 2>/dev/null; done
for e in f2fs_direct_IO_enter f2fs_direct_IO_exit f2fs_submit_read_bio; do echo 1 > $T/events/f2fs/$e/enable 2>/dev/null; done
for e in sched_waking sched_wakeup sched_switch sched_migrate_task; do echo 0 > $T/events/sched/$e/filter; echo 1 > $T/events/sched/$e/enable; done
for e in ipi_raise ipi_entry ipi_exit; do echo 1 > $T/events/ipi/$e/enable 2>/dev/null; done
echo 'name ~ "*ufs*"' > $T/events/irq/irq_handler_entry/filter; echo 1 > $T/events/irq/irq_handler_entry/enable
echo 1 > $T/events/irq/softirq_entry/enable; echo 1 > $T/events/irq/softirq_exit/enable
for e in workqueue_queue_work workqueue_execute_start workqueue_execute_end; do echo 1 > $T/events/workqueue/$e/enable 2>/dev/null; done
echo 1 > $T/events/power/cpu_idle/enable
echo 1 > $T/tracing_on
echo "trace on $(cat $T/buffer_size_kb) KB/cpu; events: $(grep -c 1 $T/events/block/*/enable | grep -c ':1')"
# Second argument "noidle": every cpuidle state but WFI disabled first, so no core goes through CPU_SUSPEND (the hv's resume path) during the run.
if [ "${2:-}" = noidle ]; then
  for c in /sys/devices/system/cpu/cpu[0-8]; do for st in $c/cpuidle/state*; do [ "$(cat $st/name)" != WFI ] && echo 1 > $st/disable; done; done
  echo "idle states off: $(cat /sys/devices/system/cpu/cpu0/cpuidle/state*/disable | tr '\n' ' ')"
fi
monkey -p nz.mahere.app -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1
t0=$(date +%s); n=0; stuck=0; lastsw=
check() {
  p=$(pidof nz.mahere.app); [ -z "$p" ] && { echo "$(( $(date +%s)-t0 ))s: no mahere pid"; return 1; }
  for t in /proc/$p/task/*; do
    st=$(cut -d' ' -f3 /proc/$(basename $t)/stat 2>/dev/null); st=$(awk '{print $3}' $t/stat 2>/dev/null)
    if [ "$st" = D ]; then w=$(cat $t/wchan); sw=$(grep nr_switches $t/sched | awk '{print $3}'); echo "$(( $(date +%s)-t0 ))s: tid $(basename $t) $(cat $t/comm) D in $w switches=$sw"
      # Stuck means no progress: the same switch count as last poll. A healthy Mahere shows D in blk_io_schedule on most polls at thousands of switches a second (measured after the fix, 2026-10-19).
      if [ "$w" = blk_io_schedule ] && [ "$sw" = "$lastsw" ]; then return 2; fi; lastsw=$sw; fi
  done
  return 0
}
while [ $(( $(date +%s)-t0 )) -lt $LIMIT ]; do
  n=$((n+1))
  case $((n%6)) in
    0) input swipe 300 1500 700 900 250;;
    1) input swipe 700 900 300 1500 250;;
    2) input swipe 500 600 500 1700 300;;
    3) input tap 504 1100; input tap 504 1100;;
    4) input swipe 200 1200 800 1200 200;;
    5) input swipe 800 1300 200 1100 400;;
  esac
  sleep 1.5
  check; r=$?
  if [ $r = 2 ]; then stuck=$((stuck+1)); else stuck=0; fi
  if [ $stuck -ge 2 ]; then
    echo 0 > $T/tracing_on
    echo "HANG at $(( $(date +%s)-t0 ))s uptime $(cut -d' ' -f1 /proc/uptime)"
    p=$(pidof nz.mahere.app)
    for t in /proc/$p/task/*; do echo "== $(basename $t) $(cat $t/comm) $(awk '{print $3}' $t/stat) wchan=$(cat $t/wchan) syscall=$(cat $t/syscall)"; cat $t/stack; done > $OUT/stacks.txt 2>&1
    { echo "sda inflight $(cat /sys/block/sda/inflight) dm-53 inflight $(cat /sys/block/dm-53/inflight)"; cat /sys/kernel/debug/block/sda/hctx0/busy; cat /sys/kernel/debug/block/sda/hctx0/tags_bitmap; cat /sys/kernel/debug/block/sda/hctx0/sched_tags_bitmap; cat /sys/kernel/debug/block/sda/hctx0/state; } > $OUT/block.txt 2>&1
    for t in /proc/$p/task/*; do [ "$(awk '{print $3}' $t/stat)" = D ] && cat $t/sched > $OUT/sched.txt 2>&1; done
    cat /sys/kernel/debug/sched/debug > $OUT/sched_debug.txt 2>&1
    grep -iE "ufs|IPI|Rescheduling" /proc/interrupts > $OUT/interrupts.txt 2>&1
    cat $T/trace > $OUT/trace.txt
    echo "saved $(wc -l < $OUT/trace.txt) trace lines"
    exit 2
  fi
  [ $((n%20)) = 0 ] && echo "$(( $(date +%s)-t0 ))s: alive, $n gestures, pid $(pidof nz.mahere.app)"
done
echo 0 > $T/tracing_on
echo "NO HANG in $LIMIT s ($n gestures)"
