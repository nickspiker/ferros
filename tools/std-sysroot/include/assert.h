#ifndef FERROS_ASSERT_H
#define FERROS_ASSERT_H
#ifdef NDEBUG
#define assert(x) ((void)0)
#else
#define assert(x) ((x) ? (void)0 : __builtin_trap())
#endif
#endif
