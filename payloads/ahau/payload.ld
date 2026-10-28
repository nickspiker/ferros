/* ahau's linker script: the payload layout (PIE, entry at offset 0, .rela.dyn for ferros_sys::relocate()), plus the zeroed-tail word of ferros_rt's image header. The entry's header (offset 8: "ferrosRT", then .bss + 16, three unused words, a heap of 0) tells the kernel how much zeroed memory follows the code (process.rs `image_tail`), so the 25 MiB .bss that holds the handle proof's scratch is in the arc. .bss is last and not in the flat image. */
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
    .bss : { *(.bss*) *(COMMON) }
    /DISCARD/ : { *(.comment) *(.eh_frame*) *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash) *(.interp) }
}
__ferros_tail = SIZEOF(.bss) + 16;
