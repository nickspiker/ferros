#ifndef FERROS_STDIO_H
#define FERROS_STDIO_H
#include <stddef.h>
#include <stdarg.h>
/* The formatter is ferros_rt's (stdio.rs, the conversions it lists); stdout and stderr go to the console. There are no files on ferros: fopen answers NULL. */
typedef struct __ferros_file FILE;
#define EOF (-1)
#ifdef __cplusplus
extern "C" {
#endif
extern FILE *stdout;
extern FILE *stderr;
int vsnprintf(char *buf, size_t n, const char *fmt, va_list ap);
int snprintf(char *buf, size_t n, const char *fmt, ...);
int printf(const char *fmt, ...);
int fprintf(FILE *f, const char *fmt, ...);
size_t fwrite(const void *p, size_t size, size_t n, FILE *f);
FILE *fopen(const char *path, const char *mode);
int fclose(FILE *f);
int fflush(FILE *f);
int fseek(FILE *f, long off, int whence);
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#ifdef __cplusplus
}
#endif
#endif
