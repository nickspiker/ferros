/* basecalc's C: MPFR's printf family sets errno on an overflowing length; the word is cshim.rs's, one for the process (the engine runs on one thread). */
#ifndef FERROS_BC_ERRNO_H
#define FERROS_BC_ERRNO_H
extern int errno;
#define EDOM 33
#define ERANGE 34
#define EINVAL 22
#define EOVERFLOW 75
#endif
