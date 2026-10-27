#ifndef FERROS_STDLIB_H
#define FERROS_STDLIB_H
#include <stddef.h>
/* No process to abort through: a trap ends the thread in the kernel's fault report. */
#define abort() __builtin_trap()
#define exit(code) __builtin_trap()
void *malloc(size_t n);
void *calloc(size_t count, size_t n);
void *realloc(void *p, size_t n);
void free(void *p);
#endif
