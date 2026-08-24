ENTRY(_start)
SECTIONS {
    . = 0x8000000000;
    .text : { *(.text) }
    .rodata : { *(.rodata) }
    /DISCARD/ : { *(.comment) *(.note.*) *(.eh_frame) }
}
