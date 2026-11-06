; ---- ROM services (TRAP #n) and unexpected exceptions (step 9) ----
; TRAP #n (vector 4), arguments in R1-R4, result in R1; R5-R12, S0-S3
; are kept, R2-R4 are not. A TRAP runs with interrupts off (IPL 7):
;   0  the monitor (does not return)
;   1  put character R1 on the console (text mode, set up at first use)
;   2  wait for a key: R1 = its character (ISO 8859-1)
;   3  a key if one is waiting: R1 = its character or 0
;   4  put the zero-terminated string at S1:R1 on the console
;   5  read R3 SD blocks from block R1:R2 (high, low) into chip RAM
;      R4:0000; R1 = 0 or an error
;   6  write R3 SD blocks likewise
; Other numbers return R1 = $FFFF.

trap_entry:
        push    r11
        push    r9
        push    r10
        push    r5
        push    r6
        push    r7
        push    r8
        mfs     r10, s1
        stv     r10, SV_CS1
        mfs     r10, s3
        push    r10
        seg1    ROM_SEG
        mfsc    r10, #3         ; CAUSE: n << 8 | 4
        movi    r9, #8
        shr     r10, r9
        cmpi    r10, #0
        bne     .putc
        li      r1, #ROM_SEG
        li      r2, #monitor_start
        jf      r1:r2
.putc:
        cmpi    r10, #1
        bne     .wait
        call      con_putc
        bra     .out
.wait:
        cmpi    r10, #2
        bne     .poll
        call      key_wait
        bra     .out
.poll:
        cmpi    r10, #3
        bne     .puts
        call      key_poll
        bra     .out
.puts:
        cmpi    r10, #4
        bne     .read
        mov     r5, r1
.c:
        ldv     r6, SV_CS1
        mts     s3, r6
        lbx     r1, (r5)
        cmpi    r1, #0
        beq     .out
        call      con_putc
        addi    r5, #1
        bra     .c
.read:
        cmpi    r10, #5
        bne     .write
        call      sd_read
        bra     .out
.write:
        cmpi    r10, #6
        bne     .unknown
        call      sd_write
        bra     .out
.unknown:
        li      r1, #$ffff
.out:
        ldv     r10, SV_CS1
        mts     s1, r10
        pop     r10
        mts     s3, r10
        pop     r8
        pop     r7
        pop     r6
        pop     r5
        pop     r10
        pop     r9
        pop     r11
        rti

; An exception or interrupt nobody handles: save every register into
; SV_REGS and show them in the monitor.
fault:
        push    r9
        push    r10
        mfs     r9, s3
        push    r9
        li      r9, #SYS_SEG
        mts     s3, r9
        li      r9, #SV_REGS
        swx     r1, (r9)+
        swx     r2, (r9)+
        swx     r3, (r9)+
        swx     r4, (r9)+
        swx     r5, (r9)+
        swx     r6, (r9)+
        swx     r7, (r9)+
        swx     r8, (r9)+
        mov     r1, r9
        pop     r2              ; S3
        pop     r3              ; R10
        pop     r4              ; R9
        swx     r4, (r1)+
        swx     r3, (r1)+
        swx     r11, (r1)+
        swx     r12, (r1)+
        mfsc    r3, #1          ; S0 when it happened
        swx     r3, (r1)+
        mfs     r3, s1
        swx     r3, (r1)+
        mfs     r3, s2
        swx     r3, (r1)+
        swx     r2, (r1)+
        mfsc    r3, #0          ; PC
        swx     r3, (r1)+
        mfsc    r3, #2          ; SR
        swx     r3, (r1)+
        mfsc    r3, #3          ; CAUSE
        swx     r3, (r1)+
        li      r1, #ROM_SEG
        li      r2, #monitor_fault
        jf      r1:r2
