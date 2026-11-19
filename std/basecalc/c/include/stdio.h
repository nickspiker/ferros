/* basecalc's C (GMP, MPFR and MPC under rug, tools/basecalc-build.sh): stdio as declarations only.
 * The engine never reads or writes a stream.
 * GMP and MPFR name these in their printf and out_str families and in their assertion reports, so they are declared here and answered by std/basecalc's cshim.rs, which traps: a call is a bug, loud. */
#ifndef FERROS_BC_STDIO_H
#define FERROS_BC_STDIO_H
/* The name gmp.h looks for before it declares its FILE functions. */
#define _STDIO_H 1
#include <stddef.h>
#include <stdarg.h>
typedef struct ferros_file FILE;
typedef long fpos_t;
#define EOF (-1)
#define BUFSIZ 1024
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
extern FILE *stdin;
extern FILE *stdout;
extern FILE *stderr;
int fprintf(FILE *f, const char *fmt, ...);
int printf(const char *fmt, ...);
int sprintf(char *s, const char *fmt, ...);
int snprintf(char *s, size_t n, const char *fmt, ...);
int vfprintf(FILE *f, const char *fmt, va_list ap);
int vprintf(const char *fmt, va_list ap);
int vsprintf(char *s, const char *fmt, va_list ap);
int vsnprintf(char *s, size_t n, const char *fmt, va_list ap);
int sscanf(const char *s, const char *fmt, ...);
int fscanf(FILE *f, const char *fmt, ...);
int scanf(const char *fmt, ...);
int vfscanf(FILE *f, const char *fmt, va_list ap);
int vsscanf(const char *s, const char *fmt, va_list ap);
int fputs(const char *s, FILE *f);
int puts(const char *s);
int fputc(int c, FILE *f);
int putc(int c, FILE *f);
int putchar(int c);
int fgetc(FILE *f);
int getc(FILE *f);
int getchar(void);
int ungetc(int c, FILE *f);
size_t fwrite(const void *p, size_t size, size_t n, FILE *f);
size_t fread(void *p, size_t size, size_t n, FILE *f);
int fflush(FILE *f);
int ferror(FILE *f);
int feof(FILE *f);
#endif
