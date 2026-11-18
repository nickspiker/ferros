/* basecalc's C: the few double functions MPC's error bounds call (eta.c, radius.c); compiler_builtins' libm answers them on ferros, which is not a unix target. */
#ifndef FERROS_BC_MATH_H
#define FERROS_BC_MATH_H
double sqrt(double x);
double floor(double x);
double ceil(double x);
double fabs(double x);
double exp(double x);
double log(double x);
double frexp(double x, int *e);
double ldexp(double x, int e);
#endif
