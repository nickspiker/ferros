#ifndef FERROS_ASSERT_H
#define FERROS_ASSERT_H
/* A failed assertion traps: the kernel's fault report names the process and the thread. Upstream's release builds define NDEBUG, as this one does. */
#ifdef NDEBUG
#define assert(e) ((void)0)
#else
#define assert(e) ((e) ? (void)0 : __builtin_trap())
#endif
#endif
