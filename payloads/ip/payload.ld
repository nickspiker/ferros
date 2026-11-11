/* ferros payload linker script — entry pinned at offset 0, everything PC-relative. The kernel calls blob offset 0 with the RUN ABI, so .text.entry MUST come first. */
ENTRY(_entry)
SECTIONS {
    . = 0;
    .text : {
        KEEP(*(.text.entry))
        *(.text*)
    }
    .rodata : { *(.rodata*) }
    .data : { *(.data*) }
    __file_end = .;
    . = ALIGN(64);
    .bss : { *(.bss*) *(COMMON) }
    /* The image's end, .bss included: the heap the kernel maps for a growing image starts 16 bytes past it (process.rs `image_tail`), where ip keeps its first connections' buffers. */
    __image_end = .;
    /DISCARD/ : { *(.comment) *(.eh_frame*) }
}
/* The zeroed tail from the file's end (alignment padding included) to .bss's end, plus 16. */
__ferros_tail = ADDR(.bss) + SIZEOF(.bss) - __file_end + 16;
