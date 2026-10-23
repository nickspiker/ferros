/* ferros payload linker script, PIE: entry pinned at offset 0, code PC-relative, and every absolute address the linker would bake into data left as an R_AARCH64_RELATIVE entry in .rela.dyn for ferros_sys::relocate() to add the load address to (the first thing _entry does). The kernel calls blob offset 0 with the RUN ABI, so .text.entry MUST come first. */
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
    .bss : { *(.bss*) }
    /DISCARD/ : { *(.comment) *(.eh_frame*) *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash) *(.interp) }
}
