# aaudio-bench

An Android-side measurement tool, for comparing Android's audio paths with ferros's on the same device (docs/AUDIO.md "The sweep"): run under Android with root, never part of ferros.

- `aaudio_bench.c`: AAudio streams with MMAP forced on or off (`out`, `in`, `rt`). `rt` plays tools/sweep-ir's sweep at -24 dBFS while the microphone records, and writes sweep-ir's own dump, so `sweep-ir ir` measures Android's loopback the way it measures ferros's.
  Build: `$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android29-clang -O2 -o aaudio_bench aaudio_bench.c -laaudio -ldl -lm` (NDK 25).
- `aoctrace.sh on|mark <text>|off|dump <file>`: tracefs kprobes on `aoc_service_write`, every message the AP sends the AoC with its service's name, to read what Android's HAL sends to open a stream (the method of docs/SENSORS.md "Capturing USF").

Push both to /data/local/tmp, run, pull the dumps, and remove both afterwards.
