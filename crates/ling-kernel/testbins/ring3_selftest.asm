BITS 64
section .text
global _start
_start:
    mov rax, 1              ; SYS_WRITE
    mov rdi, 1              ; fd = stdout
    lea rsi, [rel msg]
    mov rdx, msg_len
    syscall

    mov rax, 0               ; SYS_EXIT
    mov rdi, 42
    syscall

    ; unreachable: exit never returns to ring 3
    hlt

section .rodata
msg: db "hello from ring 3", 10
msg_len equ $ - msg
