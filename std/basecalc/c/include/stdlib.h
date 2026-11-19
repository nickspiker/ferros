/* basecalc's C: the allocator is ferros_rt's (cabi.rs, over the process heap).
 * There is no process to abort or exit through, so both trap, which ends the thread in the kernel's fault report. */
#ifndef FERROS_BC_STDLIB_H
#define FERROS_BC_STDLIB_H
#include <stddef.h>
#define abort() __builtin_trap()
#define exit(code) __builtin_trap()
#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1
void *malloc(size_t n);
void *calloc(size_t count, size_t n);
void *realloc(void *p, size_t n);
void free(void *p);
char *getenv(const char *name);
long strtol(const char *s, char **end, int base);
unsigned long strtoul(const char *s, char **end, int base);
#endif
