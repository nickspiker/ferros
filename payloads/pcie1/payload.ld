/* ferros payload linker script, PIE (as the display's): entry pinned at offset 0, code PC-relative, and every absolute address the linker would bake into data left as an R_AARCH64_RELATIVE entry in .rela.dyn for ferros_sys::relocate() to add the load address to (the first thing the entry does). Linked position-dependent until 2026-11-09, a static holding a pointer (the link detail's field names) kept its link-time address and the driver faulted reading "ssid" at G#4_471C on akita. The kernel calls blob offset 0, so .text.entry MUST come first. */
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
    __file_end = .;
    . = ALIGN(64);
    .bss : { *(.bss*) *(COMMON) }
    /DISCARD/ : { *(.comment) *(.eh_frame*) *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash) *(.interp) }
}
/* The zeroed tail from the file's end (alignment padding included) to .bss's end, plus 16. */
__ferros_tail = ADDR(.bss) + SIZEOF(.bss) - __file_end + 16;
