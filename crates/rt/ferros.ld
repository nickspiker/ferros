/* The linker script of a ferros std process (STD-TARGET.md), passed by the target spec targets/aarch64-unknown-ferros.json. PIE: the entry at offset 0, code PC-relative, every absolute address the linker would bake into data left as an R_AARCH64_RELATIVE entry in .rela.dyn for ferros_rt's relocate() to add the load address to. The kernel enters offset 0 with the RUN ABI, so .text.entry MUST come first; its header (offset 8: "ferrosRT", then .bss plus 16 — the kernel adds that and the heap word after the code — the TLS image size, the TLS block size, the TLS alignment, the heap size) is filled from the absolute symbols below. .bss is last and not in the flat image: the kernel zeroes the tail. Override the heap with --defsym __ferros_heap=<bytes>. */
ENTRY(_entry)
SECTIONS {
    . = 0;
    __image_start = .;
    .text : {
        KEEP(*(.text.entry))
        *(.text*)
    }
    .rodata : { *(.rodata*) }
    .rela.dyn : {
        __rela_start = .;
        *(.rela.dyn) *(.rela*)
        __rela_end = .;
    }
    .data : { *(.data*) *(.got*) *(.got.plt*) }
    .dynamic : { *(.dynamic) }
    . = ALIGN(64);
    .tdata : { __tdata_start = .; KEEP(*(.tdata*)) }
    .tbss : { KEEP(*(.tbss*)) }
    . = ALIGN(16);
    .bss : {
        __bss_start = .;
        *(.bss*) *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }
    /DISCARD/ : { *(.comment) *(.eh_frame*) *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash) *(.interp) }
}
/* The heap's ceiling: what the device holds (docs/NO-CEILINGS.md step 3, owner 2026-11-23: no declared caps on memory), 2^47 as ferros_sys::HEAP_DEVICE is: the kernel takes the least of it and its pool and refuses a grow only when the device is out. Never a reservation: the kernel maps 4 MiB at spawn and the rest as the heap asks (ARCS.md step 2). It was 32 MiB (2 MiB until 2026-11-14), and each app's build declared its own (Photon 512 MiB, Lumis 4 GiB). --defsym __ferros_heap=<bytes> still sets one, for a build that wants a bound of its own. */
__ferros_heap = DEFINED(__ferros_heap) ? __ferros_heap : 1 << 47;
__ferros_tdata_size = SIZEOF(.tdata);
__ferros_tls_size = SIZEOF(.tdata) + SIZEOF(.tbss);
__ferros_tls_align = MAX(ALIGNOF(.tdata), ALIGNOF(.tbss));
__ferros_tail = SIZEOF(.bss) + 16;
