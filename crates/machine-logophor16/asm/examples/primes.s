; Primes below 1000 on the debug console: the test program of the
; Logophor 16's step 2 (CPU + assembler). Trial division with REMU,
; decimal output through the stack, then DBGEXIT 0.
;
;   ada-asm build primes.s -o primes.bin && ada-asm run primes.bin

IO_SEG  = $ffff                 ; I/O registers (SPEC.md 4.1)
DBGOUT  = $43f0                 ; debug port (SPEC.md 10.5)
DBGEXIT = $43f2
LIMIT   = 1000

        .global start
start:
        li      r1, #IO_SEG
        mts     s3, r1          ; S3 reaches the I/O segment (SWX/SBX)
        li      r5, #2          ; candidate
.next:
        li      r6, #2          ; divisor
.try:
        mov     r1, r6
        mul     r1, r6          ; divisor squared
        cmp     r5, r1
        bcc     .prime          ; candidate < divisor²: no divisor left
        mov     r1, r5
        remu    r1, r6
        cmpi    r1, #0
        beq     .composite
        addi    r6, #1
        bra     .try
.prime:
        mov     r1, r5
        bl      print_dec
        movi    r1, #' '
        bl      putc
.composite:
        addi    r5, #1
        li      r1, #LIMIT
        cmp     r5, r1
        bcc     .next           ; candidate < LIMIT
        movi    r1, #10         ; newline
        bl      putc
        li      r2, #DBGEXIT
        movi    r1, #0
        swx     r1, (r2)
.stop:
        bra     .stop

; Print R1 as a decimal number. Uses R1–R4.
print_dec:
        push    lr
        movi    r4, #0          ; digits on the stack
.digit:
        mov     r2, r1
        movi    r3, #10
        remu    r2, r3          ; last digit
        divu    r1, r3
        addi    r2, #'0'
        push    r2
        addi    r4, #1
        cmpi    r1, #0
        bne     .digit
.out:
        pop     r1
        bl      putc
        addi    r4, #-1         ; sets Z at the last digit
        bne     .out
        pop     lr
        ret

; Write the low byte of R1 to the debug console. Uses R2.
putc:
        li      r2, #DBGOUT
        sbx     r1, (r2)
        ret
