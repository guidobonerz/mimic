; Brass runtime (BRASS.md 11): memory, the heap, 32-bit and fixed-point
; arithmetic, strings, printing, panics. Called near (JALR R11) from the
; compiler's code; a routine keeps R5-R8, R12, S1 and S2 and may change
; R1-R4, R9, R10, S3. S1 is the program's data segment.

RT_IO      = $ffff
RT_DBGOUT  = $43f0
RT_DBGEXIT = $43f2
RT_ROM     = $fffb
RT_HEAP_END = $0200             ; the heap ends at the end of fast RAM
RT_MAGIC   = $b500              ; block header: magic | size class
RT_FREED   = $b600

; ---- memory ----

; Copy R9 bytes from R3:R4 to R1:R2 (forwards).
@rt_memcpy:
        cmpi    r9, #0
        beq     .done
.l:
        mts     s3, r3
        lbx     r10, (r4)
        mts     s3, r1
        sbx     r10, (r2)
        addi    r4, #1
        bne     .s
        addi    r3, #1
.s:
        addi    r2, #1
        bne     .d
        addi    r1, #1
.d:
        addi    r9, #-1
        bne     .l
.done:
        ret

; Zero R9 bytes at R1:R2.
@rt_memzero:
        cmpi    r9, #0
        beq     .done
        movi    r10, #0
.l:
        mts     s3, r1
        sbx     r10, (r2)
        addi    r2, #1
        bne     .d
        addi    r1, #1
.d:
        addi    r9, #-1
        bne     .l
.done:
        ret

; Zero R3:R4 bytes (32 bits) at R1:R2.
@rt_memzero32:
        movi    r10, #0
.l:
        mov     r9, r3
        or      r9, r4
        beq     .done
        mts     s3, r1
        sbx     r10, (r2)
        addi    r2, #1
        bne     .d
        addi    r1, #1
.d:
        movi    r9, #1
        sub     r4, r9
        movi    r9, #0
        sbc     r3, r9
        bra     .l
.done:
        ret

; ---- the heap: blocks of 2^class bytes (16 and up), a 4-byte header
; (magic | class), free lists per class; new blocks from the top ----

@rt_heap_init:
        li      r9, #rtd_heap_top
        li      r1, #seg(rtd_data_end)
        addi    r1, #1
        movi    r2, #0
        sw      r2, 0(r9)
        sw      r1, 2(r9)
        li      r9, #rtd_free_lists
        movi    r1, #0
        movi    r10, #64
.c:
        sw      r1, 0(r9)
        addi    r9, #2
        addi    r10, #-1
        bne     .c
        ret

; R1:R2 = a new zeroed block of R1:R2 bytes.
@rt_alloc:
        push    r11
        push    r5
        push    r6
        push    r7
        push    r8
        mov     r5, r1          ; total = size + 4
        mov     r6, r2
        movi    r9, #4
        add     r6, r9
        movi    r9, #0
        adc     r5, r9
        movi    r3, #4          ; class, its size in r7:r8
        movi    r7, #0
        movi    r8, #16
.cls:
        cmp     r7, r5
        bhi     .found
        bcc     .grow
        cmp     r8, r6
        bcs     .found
.grow:
        add     r8, r8
        adc     r7, r7
        addi    r3, #1
        cmpi    r3, #26
        bcs     .oom
        bra     .cls
.found:
        li      r9, #rtd_free_lists
        mov     r4, r3
        add     r4, r4
        add     r4, r4
        add     r9, r4
        lw      r2, 0(r9)
        lw      r1, 2(r9)
        mov     r4, r1
        or      r4, r2
        beq     .bump
        mts     s3, r1          ; pop: the next block's address is in this one
        mov     r10, r2
        lwx     r5, (r10)+
        lwx     r6, (r10)
        sw      r5, 0(r9)
        sw      r6, 2(r9)
        bra     .have
.bump:
        li      r9, #rtd_heap_top
        lw      r2, 0(r9)
        lw      r1, 2(r9)
        mov     r5, r1
        mov     r6, r2
        add     r6, r8
        adc     r5, r7
        li      r4, #RT_HEAP_END
        cmp     r5, r4
        bhi     .oom
        bne     .room
        cmpi    r6, #0
        bne     .oom
.room:
        sw      r6, 0(r9)
        sw      r5, 2(r9)
.have:
        mts     s3, r1          ; header
        mov     r10, r2
        li      r4, #RT_MAGIC
        or      r4, r3
        swx     r4, (r10)+
        movi    r4, #0
        swx     r4, (r10)
        movi    r4, #4
        add     r2, r4
        movi    r4, #0
        adc     r1, r4
        push    r1
        push    r2
        mov     r3, r7          ; zero size - 4 bytes
        mov     r4, r8
        movi    r9, #4
        sub     r4, r9
        movi    r9, #0
        sbc     r3, r9
        li      r10, #@rt_memzero32
        jalr    r11, r10
        pop     r2
        pop     r1
        pop     r8
        pop     r7
        pop     r6
        pop     r5
        pop     r11
        ret
.oom:
        li      r1, #seg(rtd_msg_oom)
        li      r2, #off(rtd_msg_oom)
        movi    r3, #13
        li      r10, #@rt_panic
        jr      r10

; Give back the block at R1:R2 (nil, or memory not from the heap: nothing).
@rt_free:
        push    r5
        push    r6
        mov     r9, r1
        or      r9, r2
        beq     .done
        movi    r9, #4          ; the header: address - 4
        sub     r2, r9
        movi    r9, #0
        sbc     r1, r9
        li      r9, #seg(rtd_data_end)
        cmp     r1, r9
        bls     .done           ; below the heap
        mts     s3, r1
        mov     r10, r2
        lwx     r3, (r10)
        mov     r4, r3
        li      r9, #$ff00
        and     r4, r9
        li      r9, #RT_MAGIC
        cmp     r4, r9
        bne     .done           ; not a live heap block
        li      r9, #$ff
        and     r3, r9          ; class
        li      r4, #RT_FREED
        or      r4, r3
        swx     r4, (r10)
        ; block: next = head; head = block (the link goes after the header)
        li      r9, #rtd_free_lists
        add     r3, r3
        add     r3, r3
        add     r9, r3
        lw      r5, 0(r9)
        lw      r6, 2(r9)
        mts     s3, r1
        mov     r10, r2
        swx     r5, (r10)+
        swx     r6, (r10)
        sw      r2, 0(r9)
        sw      r1, 2(r9)
.done:
        pop     r6
        pop     r5
        ret

; ---- 32-bit arithmetic: R1:R2 op R3:R4 -> R1:R2 (high, low) ----

@rt_mul32:
        push    r5
        push    r6
        mov     r5, r2
        mulhu   r5, r4          ; high of lo x lo
        mov     r6, r1
        mul     r6, r4
        add     r5, r6
        mov     r6, r3
        mul     r6, r2
        add     r5, r6
        mul     r2, r4
        mov     r1, r5
        pop     r6
        pop     r5
        ret

; Unsigned R1:R2 / R3:R4: quotient R1:R2, remainder R3:R4.
@rt_udivmod:
        push    r11
        push    r5
        push    r6
        push    r7
        push    r8
        mov     r9, r3
        or      r9, r4
        bne     .ok
        li      r1, #seg(rtd_msg_div)
        li      r2, #off(rtd_msg_div)
        movi    r3, #16
        li      r10, #@rt_panic
        jr      r10
.ok:
        movi    r5, #0
        movi    r6, #0
        movi    r7, #32
.l:
        add     r2, r2
        adc     r1, r1
        adc     r6, r6
        adc     r5, r5
        cmp     r5, r3
        bhi     .sub
        bcc     .next
        cmp     r6, r4
        bcc     .next
.sub:
        sub     r6, r4
        sbc     r5, r3
        movi    r8, #1
        or      r2, r8
.next:
        addi    r7, #-1
        bne     .l
        mov     r3, r5
        mov     r4, r6
        pop     r8
        pop     r7
        pop     r6
        pop     r5
        pop     r11
        ret

@rt_divu32:
        push    r11
        li      r10, #@rt_udivmod
        jalr    r11, r10
        pop     r11
        ret

@rt_modu32:
        push    r11
        li      r10, #@rt_udivmod
        jalr    r11, r10
        mov     r1, r3
        mov     r2, r4
        pop     r11
        ret

; R1:R2 = -R1:R2.
@rt_neg32:
        not     r1, r1
        not     r2, r2
        movi    r9, #1
        add     r2, r9
        movi    r9, #0
        adc     r1, r9
        ret

; Signed: make both positive, divide, set the signs (quotient: the
; signs differ; remainder: the dividend's).
@rt_divs32:
        movi    r10, #0
        bra     @rt_sdiv
@rt_mods32:
        movi    r10, #1
@rt_sdiv:
        push    r11
        push    r5
        push    r6
        mov     r5, r10         ; 0 quotient, 1 remainder
        movi    r6, #0          ; bit 0: dividend negative, bit 1: divisor
        cmpi    r1, #0
        bge     .a
        li      r10, #@rt_neg32
        jalr    r11, r10
        movi    r6, #1
.a:
        li      r9, #$8000
        tst     r3, r9
        beq     .b
        push    r1
        push    r2
        mov     r1, r3
        mov     r2, r4
        li      r10, #@rt_neg32
        jalr    r11, r10
        mov     r3, r1
        mov     r4, r2
        pop     r2
        pop     r1
        movi    r9, #2
        or      r6, r9
.b:
        li      r10, #@rt_udivmod
        jalr    r11, r10
        cmpi    r5, #0
        beq     .quot
        mov     r1, r3
        mov     r2, r4
        movi    r9, #1
        and     r6, r9          ; the remainder takes the dividend's sign
        bra     .sign
.quot:
        mov     r9, r6          ; the quotient is negative if the signs differ
        movi    r10, #1
        and     r9, r10
        shr     r6, r10
        xor     r6, r9
.sign:
        cmpi    r6, #0
        beq     .done
        li      r10, #@rt_neg32
        jalr    r11, r10
.done:
        pop     r6
        pop     r5
        pop     r11
        ret

; R1:R2 << R3, >> R3 (logical), >> R3 (arithmetic).
@rt_shl32:
        cmpi    r3, #32
        bcs     .zero
        cmpi    r3, #0
        beq     .done
.l:
        add     r2, r2
        adc     r1, r1
        addi    r3, #-1
        bne     .l
.done:
        ret
.zero:
        movi    r1, #0
        movi    r2, #0
        ret

@rt_shr32:
        cmpi    r3, #32
        bcs     .zero
        movi    r9, #1
.l:
        cmpi    r3, #0
        beq     .done
        mov     r10, r1         ; low = low >> 1 | high << 15
        movi    r4, #15
        shl     r10, r4
        shr     r2, r9
        or      r2, r10
        shr     r1, r9
        addi    r3, #-1
        bra     .l
.done:
        ret
.zero:
        movi    r1, #0
        movi    r2, #0
        ret

@rt_sar32:
        cmpi    r3, #32
        bcc     .go
        movi    r3, #31
.go:
        movi    r9, #1
.l:
        cmpi    r3, #0
        beq     .done
        mov     r10, r1
        movi    r4, #15
        shl     r10, r4
        shr     r2, r9
        or      r2, r10
        asr     r1, r9
        addi    r3, #-1
        bra     .l
.done:
        ret

; ---- fixed point (16.16) ----

; R1:R2 x R3:R4 >> 16 (signed).
@rt_fixmul:
        push    r11
        push    r5
        push    r6
        push    r7
        push    r8
        movi    r8, #0          ; result sign
        li      r9, #$8000
        tst     r1, r9
        beq     .a
        li      r10, #@rt_neg32
        jalr    r11, r10
        movi    r9, #1
        xor     r8, r9
.a:
        li      r9, #$8000
        tst     r3, r9
        beq     .b
        push    r1
        push    r2
        mov     r1, r3
        mov     r2, r4
        li      r10, #@rt_neg32
        jalr    r11, r10
        mov     r3, r1
        mov     r4, r2
        pop     r2
        pop     r1
        movi    r9, #1
        xor     r8, r9
.b:
        ; a = r1:r2, b = r3:r4: bits 16-47 of the product
        mov     r5, r2
        mulhu   r5, r4          ; r5 = high(al x bl)  (bits 16-31)
        movi    r6, #0          ; r6 = bits 32-47
        mov     r7, r1
        mul     r7, r4          ; low(ah x bl)
        add     r5, r7
        mov     r7, r1
        mulhu   r7, r4          ; high(ah x bl)
        adc     r6, r7
        mov     r7, r2
        mul     r7, r3          ; low(al x bh)
        add     r5, r7
        mov     r7, r2
        mulhu   r7, r3          ; high(al x bh)
        adc     r6, r7
        mov     r7, r1
        mul     r7, r3          ; low(ah x bh)
        add     r6, r7
        mov     r1, r6
        mov     r2, r5
        cmpi    r8, #0
        beq     .done
        li      r10, #@rt_neg32
        jalr    r11, r10
.done:
        pop     r8
        pop     r7
        pop     r6
        pop     r5
        pop     r11
        ret

; (R1:R2 << 16) / R3:R4 (signed).
@rt_fixdiv:
        push    r11
        push    r5
        push    r6
        push    r7
        push    r8
        mov     r9, r3
        or      r9, r4
        bne     .nz
        li      r1, #seg(rtd_msg_div)
        li      r2, #off(rtd_msg_div)
        movi    r3, #16
        li      r10, #@rt_panic
        jr      r10
.nz:
        movi    r8, #0
        li      r9, #$8000
        tst     r1, r9
        beq     .a
        li      r10, #@rt_neg32
        jalr    r11, r10
        movi    r9, #1
        xor     r8, r9
.a:
        li      r9, #$8000
        tst     r3, r9
        beq     .b
        push    r1
        push    r2
        mov     r1, r3
        mov     r2, r4
        li      r10, #@rt_neg32
        jalr    r11, r10
        mov     r3, r1
        mov     r4, r2
        pop     r2
        pop     r1
        movi    r9, #1
        xor     r8, r9
.b:
        ; dividend r1:r2:r9 (48 bits: a << 16), remainder r5:r6
        movi    r9, #0
        movi    r5, #0
        movi    r6, #0
        movi    r7, #48
.l:
        add     r9, r9
        adc     r2, r2
        adc     r1, r1
        adc     r6, r6
        adc     r5, r5
        cmp     r5, r3
        bhi     .sub
        bcc     .next
        cmp     r6, r4
        bcc     .next
.sub:
        sub     r6, r4
        sbc     r5, r3
        movi    r10, #1
        or      r9, r10
.next:
        addi    r7, #-1
        bne     .l
        mov     r1, r2          ; the low 32 bits of the quotient
        mov     r2, r9
        cmpi    r8, #0
        beq     .done
        li      r10, #@rt_neg32
        jalr    r11, r10
.done:
        pop     r8
        pop     r7
        pop     r6
        pop     r5
        pop     r11
        ret

; ---- strings and slices ----

; Compare the strings whose 6-byte values are at R1:R2 and R3:R4:
; R1 = -1, 0, 1.
@rt_strcmp:
        push    r5
        push    r6
        push    r7
        push    r8
        mts     s3, r3          ; b: pointer r3:r4, length r8 (as spilled:
        mov     r9, r4          ; segment, offset, length)
        lwx     r3, (r9)+
        lwx     r4, (r9)+
        lwx     r8, (r9)
        mts     s3, r1          ; a: pointer r5:r6, length r7
        mov     r9, r2
        lwx     r5, (r9)+
        lwx     r6, (r9)+
        lwx     r7, (r9)
.l:
        cmpi    r7, #0
        beq     .aend
        cmpi    r8, #0
        beq     .gt
        mts     s3, r5
        lbx     r1, (r6)
        mts     s3, r3
        lbx     r2, (r4)
        cmp     r1, r2
        bcc     .lt
        bhi     .gt
        addi    r6, #1
        bne     .a1
        addi    r5, #1
.a1:
        addi    r4, #1
        bne     .b1
        addi    r3, #1
.b1:
        addi    r7, #-1
        addi    r8, #-1
        bra     .l
.aend:
        cmpi    r8, #0
        beq     .eq
.lt:
        li      r1, #$ffff
        bra     .done
.gt:
        movi    r1, #1
        bra     .done
.eq:
        movi    r1, #0
.done:
        pop     r8
        pop     r7
        pop     r6
        pop     r5
        ret

; A string (R1:R2, R3) from a byte slice (R1:R2, R3, R4): a copy.
@rt_str_from_bytes:
        push    r11
        push    r5
        push    r6
        push    r7
        mov     r5, r1
        mov     r6, r2
        mov     r7, r3
        movi    r1, #0
        mov     r2, r3
        li      r10, #@rt_alloc
        jalr    r11, r10
        push    r1
        push    r2
        mov     r3, r5
        mov     r4, r6
        mov     r9, r7
        li      r10, #@rt_memcpy
        jalr    r11, r10
        pop     r2
        pop     r1
        mov     r3, r7
        pop     r7
        pop     r6
        pop     r5
        pop     r11
        ret

; A byte slice from a string (R1:R2, R3): a copy, length = capacity.
@rt_bytes_from_str:
        push    r11
        li      r10, #@rt_str_from_bytes
        jalr    r11, r10
        mov     r4, r3
        pop     r11
        ret

; Make room in the slice R1..R4 for more elements: R9 = the frame offset
; (from the caller's R12) of [element size, how many more]. The slice
; comes back in R1..R4 with the same length (a new array when it moved;
; the old one is freed if it came from the heap).
@rt_grow:
        push    r11
        push    r5
        push    r6
        push    r7
        push    r8
        mfs     r10, s2         ; the parameters (R12 is 10 lower now)
        mts     s3, r10
        add     r9, r12
        addi    r9, #10
        lwx     r5, (r9)+       ; element size
        lwx     r6, (r9)        ; how many more
        mov     r7, r3          ; need = len + more
        add     r7, r6
        cmp     r7, r4
        bls     .fits
        mov     r8, r4          ; new cap = max(2 cap, need, 4)
        add     r8, r8
        cmp     r8, r7
        bcs     .c1
        mov     r8, r7
.c1:
        cmpi    r8, #4
        bcs     .c2
        movi    r8, #4
.c2:
        push    r8              ; 6: cap
        push    r3              ; 4: len
        push    r1              ; 2: old pointer, high
        push    r2              ; 0: low
        mov     r2, r8          ; bytes = cap x size (32 bits)
        mul     r2, r5
        mov     r1, r8
        mulhu   r1, r5
        li      r10, #@rt_alloc
        jalr    r11, r10
        push    r1              ; now: 2 new high, 0 new low, 6/4 old, 8 len, 10 cap
        push    r2
        lw      r8, 8(r12)      ; bytes to copy r7:r8 = len x size
        mov     r7, r8
        mul     r8, r5
        mulhu   r7, r5
        lw      r4, 4(r12)
        lw      r3, 6(r12)
.cp:
        mov     r9, r7
        or      r9, r8
        beq     .copied
        mts     s3, r3
        lbx     r10, (r4)
        mts     s3, r1
        sbx     r10, (r2)
        addi    r4, #1
        bne     .a
        addi    r3, #1
.a:
        addi    r2, #1
        bne     .b
        addi    r1, #1
.b:
        movi    r9, #1
        sub     r8, r9
        movi    r9, #0
        sbc     r7, r9
        bra     .cp
.copied:
        lw      r2, 4(r12)
        lw      r1, 6(r12)
        li      r10, #@rt_free
        jalr    r11, r10
        pop     r2              ; the new pointer
        pop     r1
        addi    r12, #4         ; the old one
        pop     r3              ; len
        pop     r4              ; cap
.fits:
        pop     r8
        pop     r7
        pop     r6
        pop     r5
        pop     r11
        ret

; ---- printing: the debug port and, on the Logophor (the ROM is there),
; the console (TRAP #1) ----

; Put character R1.
@rt_print_char:
        push    r11
        li      r9, #RT_IO
        mts     s3, r9
        li      r9, #RT_DBGOUT
        sbx     r1, (r9)
        li      r9, #rtd_quiet  ; a desktop owns the screen: the debug
        lw      r9, 0(r9)       ; port only
        cmpi    r9, #0
        bne     .done
        li      r9, #RT_ROM
        mts     s3, r9
        movi    r9, #0
        lwx     r9, (r9)
        li      r10, #$ffff
        cmp     r9, r10
        beq     .done
        trap    #1
.done:
        pop     r11
        ret

; The string R1:R2, R3.
@rt_print_str:
        push    r11
        push    r5
        push    r6
        push    r7
        mov     r5, r1
        mov     r6, r2
        mov     r7, r3
.l:
        cmpi    r7, #0
        beq     .done
        mts     s3, r5
        lbx     r1, (r6)
        li      r10, #@rt_print_char
        jalr    r11, r10
        addi    r6, #1
        bne     .n
        addi    r5, #1
.n:
        addi    r7, #-1
        bra     .l
.done:
        pop     r7
        pop     r6
        pop     r5
        pop     r11
        ret

@rt_print_bool:
        cmpi    r1, #0
        beq     .f
        li      r1, #seg(rtd_msg_true)
        li      r2, #off(rtd_msg_true)
        movi    r3, #4
        bra     @rt_print_str
.f:
        li      r1, #seg(rtd_msg_false)
        li      r2, #off(rtd_msg_false)
        movi    r3, #5
        bra     @rt_print_str

@rt_print_int:
        mov     r2, r1
        movi    r9, #15
        asr     r1, r9
        bra     @rt_print_int32

@rt_print_uint:
        mov     r2, r1
        movi    r1, #0
        bra     @rt_print_uint32

@rt_print_int32:
        li      r9, #$8000
        tst     r1, r9
        beq     @rt_print_uint32
        push    r11
        push    r1
        push    r2
        movi    r1, #'-'
        li      r10, #@rt_print_char
        jalr    r11, r10
        pop     r2
        pop     r1
        li      r10, #@rt_neg32
        jalr    r11, r10
        pop     r11
        bra     @rt_print_uint32

; Unsigned R1:R2 in decimal.
@rt_print_uint32:
        push    r11
        push    r5
        movi    r5, #0          ; digits pushed
.d:
        movi    r3, #0
        movi    r4, #10
        li      r10, #@rt_udivmod
        jalr    r11, r10
        addi    r4, #'0'
        push    r4
        addi    r5, #1
        mov     r9, r1
        or      r9, r2
        bne     .d
.p:
        pop     r1
        li      r10, #@rt_print_char
        jalr    r11, r10
        addi    r5, #-1
        bne     .p
        pop     r5
        pop     r11
        ret

; R1:R2 as 8 hex digits.
@rt_print_hex32:
        push    r11
        push    r5
        push    r6
        push    r7
        mov     r5, r1
        mov     r6, r2
        movi    r7, #8
.d:
        mov     r1, r5
        movi    r9, #12
        shr     r1, r9
        li      r9, #rtd_hex
        add     r9, r1
        lb      r1, 0(r9)
        li      r10, #@rt_print_char
        jalr    r11, r10
        movi    r9, #4          ; shift the 32 bits left by 4
        shl     r5, r9
        mov     r10, r6
        movi    r9, #12
        shr     r10, r9
        or      r5, r10
        movi    r9, #4
        shl     r6, r9
        addi    r7, #-1
        bne     .d
        pop     r7
        pop     r6
        pop     r5
        pop     r11
        ret

; A fixed value as integer.dddd (rounded down to 4 decimals).
@rt_print_fixed:
        push    r11
        push    r5
        li      r9, #$8000
        tst     r1, r9
        beq     .pos
        push    r1
        push    r2
        movi    r1, #'-'
        li      r10, #@rt_print_char
        jalr    r11, r10
        pop     r2
        pop     r1
        li      r10, #@rt_neg32
        jalr    r11, r10
.pos:
        mov     r5, r2          ; the fraction
        mov     r2, r1
        movi    r1, #0
        li      r10, #@rt_print_uint32
        jalr    r11, r10
        movi    r1, #'.'
        li      r10, #@rt_print_char
        jalr    r11, r10
        movi    r9, #4
.f:
        push    r9              ; next digit = high(frac x 10)
        mov     r1, r5
        movi    r10, #10
        mulhu   r1, r10
        mul     r5, r10
        addi    r1, #'0'
        li      r10, #@rt_print_char
        jalr    r11, r10
        pop     r9
        addi    r9, #-1
        bne     .f
        pop     r5
        pop     r11
        ret

; ---- panics and the end ----

; panic(R1:R2, R3): "panic: " message, then the end with code 2.
@rt_panic:
        push    r1
        push    r2
        push    r3
        li      r1, #seg(rtd_msg_panic)
        li      r2, #off(rtd_msg_panic)
        movi    r3, #7
        li      r10, #@rt_print_str
        jalr    r11, r10
        pop     r3
        pop     r2
        pop     r1
        li      r10, #@rt_print_str
        jalr    r11, r10
        movi    r1, #10
        li      r10, #@rt_print_char
        jalr    r11, r10
        movi    r1, #2
        li      r10, #@rt_exit
        jr      r10

@rt_panic_index:
        li      r1, #seg(rtd_msg_index)
        li      r2, #off(rtd_msg_index)
        movi    r3, #18
        bra     @rt_panic

@rt_panic_nil:
        li      r1, #seg(rtd_msg_nil)
        li      r2, #off(rtd_msg_nil)
        movi    r3, #23
        bra     @rt_panic

; The end with code R1: the debug port, then the monitor (on the
; Logophor) or a halt.
@rt_exit:
        li      r9, #RT_IO
        mts     s3, r9
        li      r9, #RT_DBGEXIT
        swx     r1, (r9)
        li      r9, #RT_ROM
        mts     s3, r9
        movi    r9, #0
        lwx     r9, (r9)
        li      r10, #$ffff
        cmp     r9, r10
        beq     .halt
        trap    #0
.halt:
        di
        halt
        bra     .halt


; ---- concurrency: Boiler's implementations through vectors (package
; boiler sets them at its start; BRASS.md 9) ----

@rt_chan_make:
        li      r10, #rtd_vec_chan_make
        bra     @rt_vec_call
@rt_chan_send:
        li      r10, #rtd_vec_chan_send
        bra     @rt_vec_call
@rt_chan_recv:
        li      r10, #rtd_vec_chan_recv
        bra     @rt_vec_call
@rt_chan_close:
        li      r10, #rtd_vec_chan_close
        bra     @rt_vec_call
@rt_go:
        li      r10, #rtd_vec_go
        bra     @rt_vec_call
@rt_select:
        li      r10, #rtd_vec_select
; Far call through the vector at S1:R10 (offset, segment); the arguments
; stay in R1-R4.
@rt_vec_call:
        push    r11
        lw      r11, 2(r10)
        lw      r10, 0(r10)
        cmpi    r11, #0
        bne     .ok
        li      r1, #seg(rtd_msg_boiler)
        li      r2, #off(rtd_msg_boiler)
        movi    r3, #38
        li      r10, #@rt_panic
        jr      r10
.ok:
        callf   r11:r10
        pop     r11
        ret

; ---- interfaces ----
; (left out of a program that calls none of them; compile/iface.rs has
; the layout of interface values and type descriptors)

; The method with name number R1 in the type descriptor at R3:R4 (count,
; value size, then per method: name number, offset, segment): R1:R2 = its
; far address (segment, offset) and R9 = 1; R9 = 0 when the descriptor has
; no such method or R3:R4 is 0:0 (a nil interface).
@rt_ilookup:
        movi    r9, #0
        mov     r10, r3
        or      r10, r4
        beq     .done
        mts     s3, r3
        mov     r10, r4
        lwx     r2, (r10)+      ; count
        lwx     r3, (r10)+      ; (the value's size)
.l:
        cmpi    r2, #0
        beq     .done
        lwx     r3, (r10)+
        cmp     r3, r1
        beq     .hit
        lwx     r3, (r10)+
        lwx     r3, (r10)+
        addi    r2, #-1
        bra     .l
.hit:
        lwx     r2, (r10)+
        lwx     r1, (r10)
        movi    r9, #1
.done:
        ret

; As rt_ilookup for a call: a panic when the interface is nil.
@rt_ifind:
        push    r11
        li      r10, #@rt_ilookup
        jalr    r11, r10
        pop     r11
        cmpi    r9, #0
        beq     .nil
        ret
.nil:
        li      r1, #seg(@rt_msg_inil)
        li      r2, #off(@rt_msg_inil)
        movi    r3, #25
        li      r10, #@rt_panic
        jr      r10

; R1 = 1 when the R9 bytes at R1:R2 and R3:R4 are equal, else 0.
@rt_memeq:
        push    r5
        push    r6
.l:
        cmpi    r9, #0
        beq     .eq
        mts     s3, r1
        lbx     r5, (r2)
        mts     s3, r3
        lbx     r6, (r4)
        cmp     r5, r6
        bne     .ne
        addi    r2, #1
        bne     .a
        addi    r1, #1
.a:
        addi    r4, #1
        bne     .b
        addi    r3, #1
.b:
        addi    r9, #-1
        bra     .l
.eq:
        movi    r1, #1
        bra     .done
.ne:
        movi    r1, #0
.done:
        pop     r6
        pop     r5
        ret

@rt_msg_inil:
        .ascii  "nil interface method call"
        .align  2
; ---- end of interfaces ----

; ---- data ----
rtd_free_lists:  .space 128
rtd_heap_top:    .space 4
rtd_hex:         .ascii "0123456789ABCDEF"
rtd_msg_true:    .ascii "true"
rtd_msg_false:   .ascii "false"
rtd_msg_panic:   .ascii "panic: "
rtd_msg_oom:     .ascii "out of memory"
rtd_msg_div:     .ascii "division by zero"
rtd_msg_index:   .ascii "index out of range"
rtd_msg_nil:     .ascii "nil pointer dereference"
                .align 2

rtd_quiet:          .space 4   ; non-zero: print to the debug port only
rtd_vec_chan_make:  .space 4
rtd_vec_chan_send:  .space 4
rtd_vec_chan_recv:  .space 4
rtd_vec_chan_close: .space 4
rtd_vec_go:         .space 4
rtd_vec_select:     .space 4
rtd_msg_boiler:     .ascii "go, channels and select need Boiler OS"
                    .align 2
