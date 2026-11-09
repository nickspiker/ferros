/* inject's linker script: the payloads' PIE layout (payloads/capture/payload.ld), with one change: the zeroed data (.bss) is laid into .data, so the file carries it. A plain payload's arc is sized for the file alone and the kernel copies the spawn input right after it, over where a separate .bss would lie: the allocator's own state and the heap would start as the script's bytes. Entry pinned at offset 0, code PC-relative, absolute addresses left in .rela.dyn for ferros_sys::relocate(). */
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
    .data : { *(.data*) *(.got*) *(.got.plt*) *(.bss*) *(COMMON) }
    .dynamic : { *(.dynamic) }
    /DISCARD/ : { *(.comment) *(.eh_frame*) *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash) *(.interp) }
}
/* .bss is carried in .data (the image holds its zeros): the tail is only the 16 every image declares. */
__ferros_tail = 16;
