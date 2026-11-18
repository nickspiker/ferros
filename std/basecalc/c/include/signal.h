/* basecalc's C: no signals on ferros.
 * GMP raises SIGFPE for a division by zero or an invalid operation in its integer layer and aborts after; here the raise traps, which ends the thread in the kernel's fault report.
 * The engine works in MPC and MPFR, which answer NaN rather than raise. */
#ifndef FERROS_BC_SIGNAL_H
#define FERROS_BC_SIGNAL_H
#define SIGFPE 8
#define raise(sig) __builtin_trap()
#endif
