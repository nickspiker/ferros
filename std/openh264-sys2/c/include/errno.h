#ifndef FERROS_ERRNO_H
#define FERROS_ERRNO_H
/* Per thread (ferros_rt posix.rs). The numbers are Linux's, which is what openh264 compares against. */
#ifdef __cplusplus
extern "C" {
#endif
int *__errno_location(void);
#ifdef __cplusplus
}
#endif
#define errno (*__errno_location())
#define EINTR 4
#define EAGAIN 11
#define ENOMEM 12
#define EBUSY 16
#define EINVAL 22
#define ETIMEDOUT 110
#endif
