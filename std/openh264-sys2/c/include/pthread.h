#ifndef FERROS_PTHREAD_H
#define FERROS_PTHREAD_H
#include <sched.h>
#include <time.h>
/* POSIX threads over ferros threads and the kernel's futex (ferros_rt posix.rs, which owns these layouts). */
typedef unsigned long pthread_t;
typedef struct { unsigned long __stack; int __scope; int __policy; } pthread_attr_t;
typedef struct { unsigned long __word; } pthread_mutex_t;
typedef struct { unsigned long __seq; } pthread_cond_t;
typedef struct { int __unused; } pthread_mutexattr_t;
typedef struct { int __unused; } pthread_condattr_t;
#define PTHREAD_MUTEX_INITIALIZER { 0 }
#define PTHREAD_COND_INITIALIZER { 0 }
#define PTHREAD_SCOPE_SYSTEM 0
#define PTHREAD_SCOPE_PROCESS 1
#ifdef __cplusplus
extern "C" {
#endif
int pthread_create(pthread_t *t, const pthread_attr_t *attr, void *(*start)(void *), void *arg);
int pthread_join(pthread_t t, void **ret);
pthread_t pthread_self(void);
int pthread_attr_init(pthread_attr_t *a);
int pthread_attr_destroy(pthread_attr_t *a);
int pthread_attr_setscope(pthread_attr_t *a, int scope);
int pthread_attr_setschedpolicy(pthread_attr_t *a, int policy);
int pthread_attr_setstacksize(pthread_attr_t *a, size_t size);
int pthread_mutex_init(pthread_mutex_t *m, const pthread_mutexattr_t *attr);
int pthread_mutex_destroy(pthread_mutex_t *m);
int pthread_mutex_lock(pthread_mutex_t *m);
int pthread_mutex_trylock(pthread_mutex_t *m);
int pthread_mutex_unlock(pthread_mutex_t *m);
int pthread_cond_init(pthread_cond_t *c, const pthread_condattr_t *attr);
int pthread_cond_destroy(pthread_cond_t *c);
int pthread_cond_wait(pthread_cond_t *c, pthread_mutex_t *m);
int pthread_cond_timedwait(pthread_cond_t *c, pthread_mutex_t *m, const struct timespec *at);
int pthread_cond_signal(pthread_cond_t *c);
int pthread_cond_broadcast(pthread_cond_t *c);
#ifdef __cplusplus
}
#endif
#endif
