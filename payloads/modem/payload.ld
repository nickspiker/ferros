/* The modem driver's linker script (ahau's): the payload layout (PIE, entry at offset 0, .rela.dyn for ferros_sys::relocate()), plus the zeroed-tail word of ferros_rt's image header, so the .bss heap is in the arc (process.rs `image_tail`). .bss is last and not in the flat image. */
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
