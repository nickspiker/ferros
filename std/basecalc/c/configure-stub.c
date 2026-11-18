/* What GMP's, MPFR's and MPC's configure scripts may link against when they build for ferros (tools/basecalc-build.sh): never run, only linked, so each check finds exactly the C library a ferros process has.
 * ferros_rt gives the allocator and strlen, compiler_builtins the mem routines; std/basecalc's cshim.rs gives strchr, the streams' names and fprintf.
 * open and close only let GMP's compiler sanity test link; raise only tells GMP to call it, and signal.h makes that call a trap.
 * Everything else configure asks for is missing here, so the libraries take their own fallbacks (no locale, no vsnprintf, no sysconf). */
typedef unsigned long size_t;
void *memcpy(void *d, const void *s, size_t n) { return d; }
void *memmove(void *d, const void *s, size_t n) { return d; }
void *memset(void *d, int c, size_t n) { return d; }
int memcmp(const void *a, const void *b, size_t n) { return 0; }
int bcmp(const void *a, const void *b, size_t n) { return 0; }
size_t strlen(const char *s) { return 0; }
char *strchr(const char *s, int c) { return 0; }
void *malloc(size_t n) { return 0; }
void *calloc(size_t c, size_t n) { return 0; }
void *realloc(void *p, size_t n) { return 0; }
void free(void *p) {}
int open(const char *p, int f, ...) { return -1; }
int close(int fd) { return 0; }
int raise(int s) { return 0; }
void *stderr;
void *stdout;
void *stdin;
int fprintf(void *f, const char *s, ...) { return 0; }
void _start(void) {}
