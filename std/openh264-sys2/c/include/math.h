#ifndef FERROS_MATH_H
#define FERROS_MATH_H
/* compiler_builtins' libm subset; sqrt, floor, ceil and fabs become single instructions (-fno-math-errno). */
#ifdef __cplusplus
extern "C" {
#endif
double sqrt(double x);
float sqrtf(float x);
double floor(double x);
double ceil(double x);
double fabs(double x);
double pow(double x, double y);
double log(double x);
double log10(double x);
double exp(double x);
#ifdef __cplusplus
}
#endif
#endif
