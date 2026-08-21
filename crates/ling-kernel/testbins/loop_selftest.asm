BITS 64
section .text
global _start
_start:
    xor r12, r12
.loop:
    mov rax, 11             ; SYS_GETPID
    syscall
    add al, 'A'
    mov [rel outbuf], al

    mov rax, 1               ; SYS_WRITE
    mov rdi, 1
    lea rsi, [rel outbuf]
    mov rdx, 1
    syscall

    mov rcx, 30000000
.spin:
    dec rcx
    jnz .spin

    inc r12
    cmp r12, 6
    jl .loop

    mov rax, 0                ; SYS_EXIT
    mov rdi, 7
    syscall
    hlt

section .data
outbuf: db 0
