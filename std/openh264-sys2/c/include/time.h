#ifndef FERROS_TIME_H
#define FERROS_TIME_H
#include <sys/types.h>
/* Every clock here is the generic counter, monotonic since the device started (ferros_rt posix.rs): openh264 takes differences, and makes absolute deadlines for timed waits that the same clock then reads, so no wall time is needed and none is given. */
struct timespec { time_t tv_sec; long tv_nsec; };
typedef int clockid_t;
#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1
#ifdef __cplusplus
extern "C" {
#endif
int clock_gettime(clockid_t clock, struct timespec *ts);
/* Declared for WelsStrftime, which nothing calls: no definition exists, so a call would fail at link, loud (there is no wall time here to give). */
struct tm { int tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year, tm_wday, tm_yday, tm_isdst; };
struct tm *localtime(const time_t *t);
size_t strftime(char *s, size_t n, const char *fmt, const struct tm *tm);
#ifdef __cplusplus
}
#endif
#endif
