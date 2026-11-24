// aaudio_bench: AAudio on the Pro's Android, for the comparison with ferros (ferros docs/AUDIO.md "The sweep").
// Usage: aaudio_bench <out|in|rt> <mmap 0|1> [sweep.raw dump.bin]
// out: a quiet 1 kHz tone for 4 s; in: 4 s of capture; rt: the sweep played once at -24 dBFS while the mic records, written as sweep-ir's dump (tools/sweep-ir).
// Every stream is told what it got: MMAP or not, sharing, burst, buffer, rate.
#include <aaudio/AAudio.h>
#include <dlfcn.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <math.h>

#define RATE 48000
#define MARK 0x535745455044554DULL
#define ANCHORS 8192
#define FRAMES_MAX (1 << 18)

static int64_t now_ns(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (int64_t)t.tv_sec * 1000000000LL + t.tv_nsec; }

typedef int32_t (*set_policy_t)(int32_t);
typedef bool (*is_mmap_t)(AAudioStream *);
static is_mmap_t is_mmap;

static AAudioStream *open_stream(int dir, int chans, int mmap) {
    AAudioStreamBuilder *b;
    AAudio_createStreamBuilder(&b);
    AAudioStreamBuilder_setDirection(b, dir);
    AAudioStreamBuilder_setSampleRate(b, RATE);
    AAudioStreamBuilder_setChannelCount(b, chans);
    AAudioStreamBuilder_setFormat(b, AAUDIO_FORMAT_PCM_I16);
    AAudioStreamBuilder_setPerformanceMode(b, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);
    AAudioStreamBuilder_setSharingMode(b, mmap ? AAUDIO_SHARING_MODE_EXCLUSIVE : AAUDIO_SHARING_MODE_SHARED);
    if (dir == AAUDIO_DIRECTION_INPUT) AAudioStreamBuilder_setInputPreset(b, AAUDIO_INPUT_PRESET_UNPROCESSED);
    AAudioStream *s = NULL;
    aaudio_result_t r = AAudioStreamBuilder_openStream(b, &s);
    AAudioStreamBuilder_delete(b);
    if (r != AAUDIO_OK) { printf("open %s: %s\n", dir == AAUDIO_DIRECTION_INPUT ? "input" : "output", AAudio_convertResultToText(r)); exit(1); }
    // The buffer at two bursts, the least AAudio keeps glitch-free in its own guidance.
    AAudioStream_setBufferSizeInFrames(s, 2 * AAudioStream_getFramesPerBurst(s));
    printf("%s: mmap %d, sharing %s, perf %d, rate %d, chans %d, burst %d, buffer %d of %d, device %d\n",
        dir == AAUDIO_DIRECTION_INPUT ? "input" : "output", is_mmap ? is_mmap(s) : -1,
        AAudioStream_getSharingMode(s) == AAUDIO_SHARING_MODE_EXCLUSIVE ? "exclusive" : "shared",
        AAudioStream_getPerformanceMode(s), AAudioStream_getSampleRate(s), AAudioStream_getChannelCount(s),
        AAudioStream_getFramesPerBurst(s), AAudioStream_getBufferSizeInFrames(s), AAudioStream_getBufferCapacityInFrames(s), AAudioStream_getDeviceId(s));
    return s;
}

// The round trip's shared state.
static int16_t *sweep; static int sweep_n;
static int16_t *cap; static int cap_n; static int ichans;
static uint64_t anchors[2 * ANCHORS]; static int nanch;
static volatile int running = 1;
static int64_t tw; static int64_t ahead_frames;

static void *reader(void *arg) {
    AAudioStream *in = arg;
    int16_t buf[4096];
    int64_t total = 0;
    int burst = AAudioStream_getFramesPerBurst(in);
    while (running && cap_n < FRAMES_MAX) {
        int32_t n = AAudioStream_read(in, buf, burst, 100000000LL);
        if (n <= 0) continue;
        int64_t t = now_ns();
        for (int k = 0; k < n && cap_n < FRAMES_MAX; k++) cap[cap_n++] = buf[k * ichans];
        total += n;
        if (nanch < ANCHORS) { anchors[2 * nanch] = total; anchors[2 * nanch + 1] = t; nanch++; }
    }
    return NULL;
}

int main(int argc, char **argv) {
    if (argc < 3) { printf("usage: aaudio_bench <out|in|rt> <mmap 0|1> [sweep.raw dump.bin]\n"); return 1; }
    int mmap = atoi(argv[2]);
    void *lib = dlopen("libaaudio.so", RTLD_NOW);
    set_policy_t setp = lib ? (set_policy_t)dlsym(lib, "AAudio_setMMapPolicy") : NULL;
    is_mmap = lib ? (is_mmap_t)dlsym(lib, "AAudioStream_isMMapUsed") : NULL;
    // AAUDIO_POLICY_NEVER 1, AAUDIO_POLICY_ALWAYS 3 (aaudio/AAudioTesting.h).
    if (setp) printf("mmap policy %d: %d\n", mmap ? 3 : 1, setp(mmap ? 3 : 1)); else printf("no AAudio_setMMapPolicy\n");
    if (!strcmp(argv[1], "out")) {
        AAudioStream *o = open_stream(AAUDIO_DIRECTION_OUTPUT, 2, mmap);
        AAudioStream_requestStart(o);
        int16_t buf[2 * 4096]; int burst = AAudioStream_getFramesPerBurst(o); long ph = 0;
        for (long f = 0; f < 4L * RATE; f += burst) {
            for (int k = 0; k < burst; k++, ph++) buf[2 * k] = buf[2 * k + 1] = (int16_t)(2048 * sin(2 * M_PI * 1000 * ph / RATE));
            AAudioStream_write(o, buf, burst, 100000000LL);
        }
        printf("output: xruns %d\n", AAudioStream_getXRunCount(o));
        AAudioStream_requestStop(o); AAudioStream_close(o);
        return 0;
    }
    if (!strcmp(argv[1], "in")) {
        AAudioStream *in = open_stream(AAUDIO_DIRECTION_INPUT, 1, mmap);
        AAudioStream_requestStart(in);
        int16_t buf[4096]; int burst = AAudioStream_getFramesPerBurst(in); long got = 0; int peak = 0;
        while (got < 4L * RATE) { int n = AAudioStream_read(in, buf, burst, 100000000LL); for (int k = 0; k < n; k++) if (abs(buf[k]) > peak) peak = abs(buf[k]); got += n > 0 ? n : 0; }
        printf("input: %ld frames, peak %d, xruns %d\n", got, peak, AAudioStream_getXRunCount(in));
        AAudioStream_requestStop(in); AAudioStream_close(in);
        return 0;
    }
    if (strcmp(argv[1], "rt") || argc < 5) { printf("rt needs sweep.raw and dump.bin\n"); return 1; }
    FILE *f = fopen(argv[3], "rb"); fseek(f, 0, SEEK_END); long len = ftell(f); fseek(f, 0, SEEK_SET);
    sweep_n = len / 2; sweep = malloc(len); fread(sweep, 1, len, f); fclose(f);
    cap = calloc(FRAMES_MAX, 2);
    AAudioStream *in = open_stream(AAUDIO_DIRECTION_INPUT, 1, mmap);
    AAudioStream *o = open_stream(AAUDIO_DIRECTION_OUTPUT, 2, mmap);
    ichans = AAudioStream_getChannelCount(in);
    AAudioStream_requestStart(in); AAudioStream_requestStart(o);
    pthread_t th; pthread_create(&th, NULL, reader, in);
    int burst = AAudioStream_getFramesPerBurst(o);
    int16_t buf[2 * 4096];
    // A spacing of silence (2^14 frames), the sweep at -24 dBFS (a 16th), a spacing of silence.
    long pre = 1 << 14, total = pre + sweep_n + (1 << 14);
    for (long at = 0; at < total; at += burst) {
        for (int k = 0; k < burst; k++) { long i = at + k - pre; int16_t v = (i >= 0 && i < sweep_n) ? sweep[i] / 16 : 0; buf[2 * k] = buf[2 * k + 1] = v; }
        if (at <= pre && pre < at + burst) {
            // The sweep's first frame goes in with this write: its time and the frames ahead of it (written, less read by the device).
            tw = now_ns();
            ahead_frames = AAudioStream_getFramesWritten(o) - AAudioStream_getFramesRead(o) + (pre - at);
        }
        AAudioStream_write(o, buf, burst, 100000000LL);
    }
    running = 0; pthread_join(th, NULL);
    printf("rt: output xruns %d, input xruns %d, %d frames captured, %d anchors, %lld frames ahead at the sweep's write\n", AAudioStream_getXRunCount(o), AAudioStream_getXRunCount(in), cap_n, nanch, (long long)ahead_frames);
    AAudioStream_requestStop(o); AAudioStream_requestStop(in); AAudioStream_close(o); AAudioStream_close(in);
    // sweep-ir's dump: eight words, the anchors, the samples as three microphones (this mono capture in the first).
    FILE *d = fopen(argv[4], "wb");
    uint64_t head[8] = { MARK, 1000000000ULL, (uint64_t)tw, (uint64_t)(ahead_frames * 4), 0, (uint64_t)cap_n, (uint64_t)nanch, 0xFF | (uint64_t)(mmap ? 1 : 0) << 8 };
    fwrite(head, 8, 8, d); fwrite(anchors, 8, 2 * ANCHORS, d);
    for (int k = 0; k < cap_n; k++) { int16_t fr[3] = { cap[k], 0, 0 }; fwrite(fr, 2, 3, d); }
    fclose(d);
    return 0;
}
