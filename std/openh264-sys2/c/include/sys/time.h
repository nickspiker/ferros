#ifndef FERROS_SYS_TIME_H
#define FERROS_SYS_TIME_H
#include <time.h>
/* The counter's time as time.h says, never the wall clock. */
struct timeval { time_t tv_sec; suseconds_t tv_usec; };
#ifdef __cplusplus
extern "C" {
#endif
int gettimeofday(struct timeval *tv, void *tz);
#ifdef __cplusplus
}
#endif
#endif
