#ifndef FERROS_STDLIB_H
#define FERROS_STDLIB_H
#include <stddef.h>
/* The allocator is std's copy of ferros_rt (cabi.rs), the process's one heap. */
#ifdef __cplusplus
extern "C" {
#endif
void *malloc(size_t n);
void *calloc(size_t count, size_t n);
void *realloc(void *p, size_t n);
void free(void *p);
#ifdef __cplusplus
}
#endif
static inline int abs(int x) { return __builtin_abs(x); }
#endif
