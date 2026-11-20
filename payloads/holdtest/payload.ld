/* ferros payload linker script, PIE: entry pinned at offset 0, code PC-relative, and every absolute address the linker would bake into data left as an R_AARCH64_RELATIVE entry in .rela.dyn for ferros_sys::relocate() to add the load address to (the first thing _entry does). The kernel calls blob offset 0 with the RUN ABI, so .text.entry MUST come first. Its header (offset 8: "ferrosRT", then .bss + 16) tells the kernel how much zeroed memory follows the code: the heap and the work thread's stack live there (process.rs `image_tail`). */
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
