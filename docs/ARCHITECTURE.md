# ARCHITECTURE — ferros Design Principles
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Security is the architecture, not a layer on top.

---

## Why ferros Exists

Linux accumulated complexity solving problems that ferros eliminates
at the architectural level. Not because Linux engineers were bad —
because they were constrained by backward compatibility and a 1970s
foundation.

ferros has no such constraint.

---

## Structural Elimination

Every section below shows a Linux complexity that ferros does not
mitigate, patch, or work around — it structurally eliminates. The
problem does not exist in the architecture.

### Page Tables

```
Linux:   demand paging, swapping, copy-on-write fork,
         huge pages, transparent huge pages,
         mmap, mremap, mprotect, numa, ksm,
         page reclaim, OOM killer,
         thousands of lines, dozens of CVEs

ferros:  process gets a ring, hardware gets a grant
         that's the entire page table story
         no swapping (killswitch ready, no swap partition)
         no demand paging (ring is always valid)
         no OOM killer (ring bounded at spawn)
```

### Fork

```
Linux:   fork() copies entire process address space
         COW fork: complex, racy, source of countless bugs
         Spectre/Meltdown: fundamentally about fork + shared memory

ferros:  no fork()
         new process: new ring, new CSpace, new keys
         no shared state by default
         no fork-related vulnerability class
```

### Signals

```
Linux:   signals are asynchronous, delivered at arbitrary points
         signal handlers: reentrant nightmares
         SIGKILL vs cleanup: entire design problem
         async-signal-safe functions: painful restriction

ferros:  no signals
         capability notifications: synchronous, ordered
         kill: hardware relay, no handler needed
         entire signal complexity: does not exist
```

### Device Drivers

```
Linux:   ~70% of kernel code is drivers
         driver bug: kernel panic
         driver CVE: kernel privilege
         binary blobs: unauditable, unprovable

ferros:  all drivers userspace
         driver bug: process crash, restart
         driver CVE: capability bounded
         no binary blobs: pure Rust
         kernel stays small, stays proven
```

### Filesystem

```
Linux:   Virtual Filesystem Switch
         dozens of filesystem implementations
         each with own bugs, own CVEs
         ext4, btrfs, xfs, nfs, fuse...
         all in kernel, all kernel privilege

ferros:  one storage model: spine + HAMT + tract
         plow-managed log-structured ring, no block allocator
         entirely userspace (after boot)
         kernel knows nothing about filesystems
         VSF is the format, period
         see VAULT.md, RING.md, HAMT.md
```

### Privilege Escalation

```
Linux:   setuid binaries
         capability sets (confusingly named, different from ferros caps)
         namespace tricks
         seccomp bypass chains
         entire industry around privilege escalation

ferros:  no setuid
         no privilege escalation path
         capabilities are cryptographic tokens (BLAKE3)
         you have what you were granted, nothing more
         escalation requires forging a BLAKE3 hash: 2^-256
```

### Scheduler

```
Linux:   CFS, real-time scheduling classes,
         cgroups, namespaces, control groups,
         hundreds of tuning parameters,
         priority inversion, convoy effects

ferros:  simple scheduler
         no cgroups (capability tree handles isolation)
         no namespaces (ring memory handles isolation)
         threads get time, that's it
```

### Syscall Surface

```
Linux:   ~350 syscalls
         each one: kernel attack surface
         seccomp: trying to reduce this after the fact
         io_uring: new syscall mechanism, new CVE class

ferros:  five kernel responsibilities
         syscall surface: tiny
         IPC: capability gated
         no syscall you weren't granted a cap for
```

### Boot

```
Linux:   initrd, initramfs, pivot_root,
         udev, systemd, dozens of race conditions,
         fsck, journal replay, recovery mode

ferros:  seed verifies kernel → jump
         kernel scans spine → binary search → restore snapshot
         no fsck (BLAKE3 + HAMT + plow)
         no recovery mode (always valid state)
         no initrd (vault always bootable)
         deterministic, proven
         see SECURITY_CHAIN.md, RING.md
```

### Trust Model

```
Linux:   Secure Boot → shim → GRUB → kernel → systemd → SELinux
         each link: different maintainer, different threat model
         locked bootloaders protect vendor, not owner
         owner cannot sign their own kernel without penalty

ferros:  developer key → seed → kernel → vault root → userspace
         one chain, one key model, one verification mechanism
         owner CAN replace developer key (full sovereignty)
         no features disabled for using your own key
         see SECURITY_CHAIN.md
```

---

## The Meta-Point

```
Linux missteps fall into two categories:

1. Complexity added to solve real problems
   that ferros's architecture makes not-problems:
   fork, signals, VFS, page table management

2. Retrofitted security on top of insecure foundations:
   seccomp, namespaces, capabilities, SELinux, AppArmor
   all bolted on after the fact
   all leaky abstractions
   all with their own CVE histories

ferros:  security is the architecture
         not a layer on top
         these problems structurally do not exist
```

---

## Five Kernel Responsibilities

The ferros kernel does exactly five things:

```
1. Memory:     ring allocation, grants, bounds enforcement
2. Scheduling: time slicing, IPC dispatch
3. IPC:        capability-gated message passing
4. Boot:       seed verification, vault root scan, state restore
5. Hardware:   interrupt routing to userspace drivers

Everything else is userspace:
  drivers, filesystems, networking, display, audio,
  authentication, encryption, logging, applications
  all capability-gated, all restartable, all isolated
```

---

## Document Map

```
ARCHITECTURE.md     this document — why these decisions
SECURITY_CHAIN.md   boot trust model, signature chain, owner sovereignty
RING.md             ring mechanics, binary search, mirror protocol
VAULT.md            persistent object store: tract, plow, HAMT, spine
HAMT.md             hash array mapped trie, COW versioning, object formats
LEDGER.md           append-only event chain, VSF format
```

---