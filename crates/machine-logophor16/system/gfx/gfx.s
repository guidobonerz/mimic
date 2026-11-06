; gfx.run(j *job): wait for the Lithograph, write the job's 18 words into
; its registers (FFFF:0100-0123), start it, wait until it is done.
gfx__run:
        push    r10
        li      r10, #$ffff
        mts     s3, r10
        li      r4, #$0126      ; LTHSTAT
        movi    r3, #1
.w:
        lwx     r9, (r4)
        tst     r9, r3
        bne     .w
        li      r4, #$0100
        movi    r3, #18
.c:
        mts     s3, r1
        lwx     r9, (r2)+
        mts     s3, r10
        swx     r9, (r4)+
        addi    r3, #-1
        bne     .c
        swx     r9, (r4)        ; LTHSTART
        li      r4, #$0126
        movi    r3, #1
.d:
        lwx     r9, (r4)
        tst     r9, r3
        bne     .d
        pop     r10
        retf

; gfx.consoleOff(): print and println go to the debug port only, as
; Panorama owns the screen (Foundry's console would switch to text).
gfx__consoleOff:
        li      r9, #rtd_quiet  ; (R10 holds the return segment)
        movi    r1, #1
        sw      r1, 0(r9)
        retf
