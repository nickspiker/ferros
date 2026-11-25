#ifndef FERROS_SCHED_H
#define FERROS_SCHED_H
#include <stddef.h>
/* Only what openh264's CPU count asks (WelsQueryLogicalProcessInfo, the __linux__ arm): the process's set, from ferros_rt posix.rs. */
#define SCHED_OTHER 0
#define SCHED_FIFO 1
#define CPU_SETSIZE 64
typedef struct { unsigned long __bits; } cpu_set_t;
#define CPU_ZERO(s) ((s)->__bits = 0)
#define CPU_SET(c, s) ((s)->__bits |= 1UL << (c))
#define CPU_ISSET(c, s) (((s)->__bits >> (c)) & 1)
#define CPU_COUNT(s) __builtin_popcountl((s)->__bits)
typedef int pid_t;
#ifdef __cplusplus
extern "C" {
#endif
int sched_getaffinity(pid_t pid, size_t size, cpu_set_t *set);
int sched_yield(void);
#ifdef __cplusplus
}
#endif
#endif
