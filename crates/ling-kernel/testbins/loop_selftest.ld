ENTRY(_start)
SECTIONS {
    . = 0x8000000000;
    .text : { *(.text) }
    .rodata : { *(.rodata) }
    . = ALIGN(4096);
    .data : { *(.data) }
    .bss : { *(.bss) }
    /DISCARD/ : { *(.comment) *(.note.*) *(.eh_frame) }
}
