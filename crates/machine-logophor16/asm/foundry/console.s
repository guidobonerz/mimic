; ---- console (step 9): text mode 4, 100 x 37, and the keyboard ----
; Routines keep R5-R12 and S1 (S1 must be the ROM: they read its tables);
; they use R1-R4, R9, R10 and leave S3 at whatever they last used.

; Set the console up: text colours, font, cleared screen in mode 4,
; cursor at the top. Uses R1-R8.
con_init:
        push    lr
        seg3    IO_SEG
        call      set_palette
        call      copy_font
        li      r1, #$c001      ; master and framebuffer DMA (and Telegraph's)
        iowr    DMACON, r1
        li      r1, #$8008
        iowr    DMACON, r1
        li      r1, #TEXT_SEG
        iowr    FBSEG, r1
        li      r1, #TEXT_OFS
        iowr    FBOFS, r1
        li      r1, #TEXT_PITCH
        iowr    PITCH, r1
        li      r1, #FONT_SEG
        iowr    FONTSEG, r1
        movi    r1, #0
        iowr    FONTOFS, r1
        iowr    TXTBANK, r1
        iowr    BORDER, r1
        li      r1, #$fe
        iowr    CURSHAPE, r1
        li      r9, #TEXT_SEG
        mts     s3, r9
        li      r2, #TEXT_OFS
        li      r3, #3700
        movi    r4, #' '
        movi    r1, #CON_ATTR
.clear:
        sbx     r4, (r2)+
        sbx     r1, (r2)+
        addi    r3, #-1
        bne     .clear
        movi    r1, #0
        stv     r1, SV_CUR
        movi    r1, #1
        stv     r1, SV_CONON
        movi    r1, #0
        iowr    CURSOR, r1
        seg3    IO_SEG
        movi    r5, #1
        call      wait_fields     ; the pointer is taken over at the VBlank
        li      r1, #$8004
        iowr    VMODE, r1
        pop     lr
        ret

; Put character R1 out: 10 = new line, 8 = back one cell; at the end
; the screen scrolls up a line.
con_putc:
        push    lr
        ldv     r2, SV_CONON
        cmpi    r2, #0
        bne     .on
        push    r1
        push    r5
        push    r6
        push    r7
        push    r8
        call      con_init
        pop     r8
        pop     r7
        pop     r6
        pop     r5
        pop     r1
.on:
        ldv     r2, SV_CUR
        cmpi    r1, #10
        beq     .nl
        cmpi    r1, #13
        beq     .nl
        cmpi    r1, #8
        beq     .bs
        mov     r3, r2
        add     r3, r3
        li      r4, #TEXT_OFS
        add     r3, r4
        li      r9, #TEXT_SEG
        mts     s3, r9
        sbx     r1, (r3)+
        movi    r4, #CON_ATTR
        sbx     r4, (r3)
        addi    r2, #1
        bra     .check
.nl:
        movi    r3, #100
        divu    r2, r3
        addi    r2, #1
        mul     r2, r3
        bra     .check
.bs:
        cmpi    r2, #0
        beq     .store
        addi    r2, #-1
        mov     r3, r2
        add     r3, r3
        li      r4, #TEXT_OFS
        add     r3, r4
        li      r9, #TEXT_SEG
        mts     s3, r9
        movi    r4, #' '
        sbx     r4, (r3)
        bra     .store
.check:
        li      r3, #3700
        cmp     r2, r3
        bcc     .store          ; below the end
        li      r9, #TEXT_SEG   ; scroll: rows 1-36 up, row 36 cleared
        mts     s3, r9
        li      r3, #TEXT_OFS
        li      r4, #TEXT_OFS+200
        li      r2, #3600
.up:
        lwx     r1, (r4)+
        swx     r1, (r3)+
        addi    r2, #-1
        bne     .up
        movi    r2, #100
        li      r1, #CON_ATTR<<8|' '
.blank:
        swx     r1, (r3)+
        addi    r2, #-1
        bne     .blank
        li      r2, #3600
.store:
        stv     r2, SV_CUR
        iowr    CURSOR, r2
        pop     lr
        ret

; Put the zero-terminated string at R1 (ROM) out.
con_puts:
        push    lr
        push    r5
        mov     r5, r1
.c:
        lb      r1, 0(r5)
        cmpi    r1, #0
        beq     .done
        call      con_putc
        addi    r5, #1
        bra     .c
.done:
        pop     r5
        pop     lr
        ret

; Put R1 out as R2 hex digits (1-4, the low ones).
con_hex:
        push    lr
        push    r5
        push    r6
        mov     r5, r1
        mov     r6, r2
        movi    r1, #4          ; shift the wanted digits to the top
        sub     r1, r6
        add     r1, r1
        add     r1, r1
        shl     r5, r1
.d:
        mov     r1, r5
        movi    r2, #12
        shr     r1, r2
        li      r2, #hex
        add     r1, r2
        lb      r1, 0(r1)
        call      con_putc
        movi    r2, #4
        shl     r5, r2
        addi    r6, #-1
        bne     .d
        pop     r6
        pop     r5
        pop     lr
        ret

con_hex4:
        movi    r2, #4
        jmp     con_hex
con_hex2:
        movi    r2, #2
        jmp     con_hex

con_space:
        movi    r1, #' '
        jmp     con_putc

con_nl:
        movi    r1, #10
        jmp     con_putc

; The next character typed, in R1 (0: none). Shift keys are followed;
; releases and keys without a character are skipped.
key_poll:
.next:
        iord    r1, KBDDATA
        cmpi    r1, #0
        beq     .none
        mov     r4, r1
        li      r3, #$ff
        and     r4, r3          ; usage
        movi    r3, #1          ; shift bit: left
        cmpi    r4, #$e1
        beq     .shift
        movi    r3, #2          ; right
        cmpi    r4, #$e5
        beq     .shift
        li      r3, #$8000
        tst     r1, r3
        bne     .next           ; released
        cmpi    r4, #$39
        bcs     .next           ; no character
        ldv     r3, SV_SHIFT
        li      r2, #keymap
        cmpi    r3, #0
        beq     .plain
        li      r2, #keymap_shift
.plain:
        add     r2, r4
        lb      r1, 0(r2)
        cmpi    r1, #0
        beq     .next
        ret
.shift:
        ldv     r2, SV_SHIFT
        li      r4, #$8000
        tst     r1, r4
        bne     .up
        or      r2, r3
        bra     .set
.up:
        not     r3, r3
        and     r2, r3
.set:
        stv     r2, SV_SHIFT
        bra     .next
.none:
        movi    r1, #0
        ret

; Wait for a character (R1).
key_wait:
        push    lr
.w:
        call      key_poll
        cmpi    r1, #0
        beq     .w
        pop     lr
        ret
