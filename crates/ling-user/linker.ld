ENTRY(_start)

PHDRS {
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS {
    . = 0x8000000000;

    .text : ALIGN(4K) {
        *(.text._start)
        *(.text .text.* .ltext .ltext.*)
    } :text

    . = ALIGN(4K);
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.* .lrodata .lrodata.*)
    } :rodata

    . = ALIGN(4K);
    .data : ALIGN(4K) {
        *(.data .data.* .ldata .ldata.*)
        *(.got .got.*)
        *(.bss .bss.* .lbss .lbss.*)
        *(COMMON)
    } :data

    /DISCARD/ : {
        *(.comment)
        *(.note.*)
        *(.eh_frame)
    }
}
