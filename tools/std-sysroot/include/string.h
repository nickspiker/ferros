/* The freestanding C headers a ferros std build hands the cc crate (tools/std-build.sh, -isystem): the cross gcc ships no libc headers, and the C in the graph (PQClean's KEM suites, blake3's kernels) wants only these declarations. The functions are compiler_builtins' mem routines, linked into every std image. */
#ifndef FERROS_STRING_H
#define FERROS_STRING_H
#include <stddef.h>
void *memcpy(void *dst, const void *src, size_t n);
void *memmove(void *dst, const void *src, size_t n);
void *memset(void *dst, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
size_t strlen(const char *s);
#endif
