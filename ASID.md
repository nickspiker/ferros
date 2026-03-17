Things Linux has to deal with that ferros structurally eliminates:

**Page table complexity:**
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

**Fork:**
```
Linux:   fork() copies entire process address space
         COW fork: complex, racy, source of countless bugs
         Spectre/Meltdown: fundamentally about fork + shared memory

ferros:  no fork()
         new process: new ring, new CSpace, new keys
         no shared state by default
         no fork-related vulnerability class
```

**Signal handling:**
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

**Device drivers in kernel:**
```
Linux:   ~70% of kernel code is drivers
         driver bug: kernel panic
         driver CVE: kernel privilege
         binary blobs: unauditable, unprovable

ferros:  all drivers userspace
         driver bug: process crash, restart
         driver CVE: capability bounded
         no binary blobs: pure Rust
         kernel stays 10K lines, stays proven
```

**The VFS layer:**
```
Linux:   Virtual Filesystem Switch
         dozens of filesystem implementations
         each with own bugs, own CVEs
         ext4, btrfs, xfs, nfs, fuse...
         all in kernel, all kernel privilege

ferros:  one filesystem: Ring FS + HAMT + Vault
         entirely userspace
         kernel knows nothing about filesystems
         VSF is the format, period
```

**Privilege escalation surface:**
```
Linux:   setuid binaries
         capability sets (confusingly named, different from ferros caps)
         namespace tricks
         seccomp bypass chains
         entire industry around privilege escalation

ferros:  no setuid
         no privilege escalation path
         capabilities are cryptographic tokens
         you have what you were granted, nothing more
         escalation requires forging a BLAKE3 hash: 2^-256
```

**The scheduler:**
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

**Syscall surface:**
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

**Boot:**
```
Linux:   initrd, initramfs, pivot_root,
         udev, systemd, dozens of race conditions,
         fsck, journal replay, recovery mode

ferros:  scan vault root → find generation → restore snapshot
         no fsck (BLAKE3 + ring FS + HAMT)
         no recovery mode (always valid state)
         no initrd (ring always bootable)
         300ms, deterministic, proven
```

**The meta-point:**

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

The common thread: Linux accumulated complexity solving problems that ferros eliminates at the architectural level. Not because Linux engineers were bad — because they were constrained by backward compatibility and a 1970s foundation.

ferros has no such constraint.