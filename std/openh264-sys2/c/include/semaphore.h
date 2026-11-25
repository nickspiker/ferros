#ifndef FERROS_SEMAPHORE_H
#define FERROS_SEMAPHORE_H
#include <time.h>
/* A counting semaphore on the kernel's futex (ferros_rt posix.rs): the count, and how many may be asleep, so a post with nobody waiting makes no syscall. */
typedef struct { unsigned long __count; unsigned long __sleepers; } sem_t;
#ifdef __cplusplus
extern "C" {
#endif
int sem_init(sem_t *s, int shared, unsigned int value);
int sem_destroy(sem_t *s);
int sem_post(sem_t *s);
int sem_wait(sem_t *s);
int sem_trywait(sem_t *s);
int sem_timedwait(sem_t *s, const struct timespec *at);
#ifdef __cplusplus
}
#endif
#endif
