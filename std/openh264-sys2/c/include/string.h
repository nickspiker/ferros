#ifndef FERROS_STRING_H
#define FERROS_STRING_H
#include <stddef.h>
/* memcpy, memmove and memset are ferros_rt's (mem.rs), memcmp and strlen compiler_builtins' and ferros_rt's; the rest ferros_rt posix.rs. */
#ifdef __cplusplus
extern "C" {
#endif
void *memcpy(void *d, const void *s, size_t n);
void *memmove(void *d, const void *s, size_t n);
void *memset(void *d, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
size_t strlen(const char *s);
char *strncpy(char *d, const char *s, size_t n);
#ifdef __cplusplus
}
#endif
#endif
