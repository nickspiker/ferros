/* glibc's feature macros: only the compiler-version test PQClean's compat.h asks of it. */
#ifndef FERROS_FEATURES_H
#define FERROS_FEATURES_H
#define __GNUC_PREREQ(maj, min) ((__GNUC__ << 16) + __GNUC_MINOR__ >= ((maj) << 16) + (min))
#endif
