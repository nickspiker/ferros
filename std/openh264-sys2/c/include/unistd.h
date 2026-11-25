#ifndef FERROS_UNISTD_H
#define FERROS_UNISTD_H
#include <sys/types.h>
typedef unsigned int useconds_t;
#ifdef __cplusplus
extern "C" {
#endif
/* The thread sleeps in the kernel until the counter passes the time asked (SYS_SLEEP), never a spin. */
int usleep(useconds_t usec);
/* The encoder names its thread events with it (named semaphores, Apple only): one process, one number. */
pid_t getpid(void);
#ifdef __cplusplus
}
#endif
#endif
