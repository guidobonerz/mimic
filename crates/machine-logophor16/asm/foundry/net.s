; ---- network services (step 10) ----
; Foundry's driver for Telegraph's network unit: the rings and buffers at
; NETBUF_SEG ($001E): transmit ring 0000, receive ring 0080 (16
; descriptors of 8 bytes each), the transmit buffer 0100, 16 receive
; buffers of 1536 bytes from 0800. Called from the TRAP handler (S1 = ROM,
; the caller's S1 in SV_CS1).

NET_TXR  = $0000
NET_RXR  = $0080
NET_TXB  = $0100
NET_RXB  = $0800
NET_BUFLEN = 1536

; TRAP #7: set the rings up, transmit and receive on. R1 = NETSTAT (bit 0:
; link up). Uses R1-R6, R9, R10.
net_open:
        li      r9, #NETBUF_SEG
        mts     s3, r9
        movi    r2, #NET_TXR    ; transmit ring: all the CPU's
        movi    r1, #0
        movi    r3, #64
.tx:
        swx     r1, (r2)+
        addi    r3, #-1
        bne     .tx
        li      r2, #NET_RXR    ; receive ring: all the hardware's
        li      r4, #NET_RXB
        movi    r3, #16
.rx:
        li      r1, #NETBUF_SEG
        swx     r1, (r2)+
        swx     r4, (r2)+
        li      r1, #NET_BUFLEN
        swx     r1, (r2)+
        li      r1, #$8000
        swx     r1, (r2)+
        li      r1, #NET_BUFLEN
        add     r4, r1
        addi    r3, #-1
        bne     .rx
        li      r1, #$8008      ; Telegraph's DMA
        iowr    DMACON, r1
        li      r1, #NETBUF_SEG
        iowr    TXRSEG, r1
        iowr    RXRSEG, r1
        li      r1, #NET_TXR
        iowr    TXROFS, r1
        li      r1, #NET_RXR
        iowr    RXROFS, r1
        movi    r1, #3
        iowr    NETCON, r1
        iord    r1, TXHEAD      ; continue where the hardware is
        stv     r1, SV_TXI
        iord    r1, RXHEAD
        stv     r1, SV_RXI
        iord    r1, NETSTAT
        ret

; TRAP #8: send the frame at S1:R1 (the caller's), R2 bytes (14-1536).
; R1 = 0, or 1 for an error. Uses R1-R6, R9, R10.
net_send:
        mov     r5, r1
        cmpi    r2, #14
        bcc     .bad
        li      r1, #NET_BUFLEN+1
        cmp     r2, r1
        bcs     .bad
        mov     r6, r2
        li      r3, #NET_TXB
.copy:
        ldv     r4, SV_CS1
        mts     s3, r4
        lbx     r1, (r5)
        li      r9, #NETBUF_SEG
        mts     s3, r9
        sbx     r1, (r3)
        addi    r5, #1
        addi    r3, #1
        addi    r2, #-1
        bne     .copy
        ldv     r5, SV_TXI      ; the descriptor
        mov     r3, r5
        add     r3, r3
        add     r3, r3
        add     r3, r3
        li      r9, #NETBUF_SEG
        mts     s3, r9
        li      r1, #NETBUF_SEG
        swx     r1, (r3)+
        li      r1, #NET_TXB
        swx     r1, (r3)+
        swx     r6, (r3)+
        li      r1, #$8000
        swx     r1, (r3)
        addi    r5, #1
        movi    r1, #15
        and     r5, r1
        stv     r5, SV_TXI
        iowr    TXTAIL, r5      ; send
.wait:
        iord    r1, TXHEAD
        cmp     r1, r5
        bne     .wait
        li      r9, #NETBUF_SEG
        mts     s3, r9
        lwx     r1, (r3)        ; its flags: bit 0 error
        movi    r2, #1
        and     r1, r2
        ret
.bad:
        movi    r1, #1
        ret

; TRAP #9: a received frame to S1:R1 (the caller's, room for 1536
; bytes): R1 = its length (0: none waiting), R2 = its descriptor flags
; (bit 0 cut, bit 1 CRC error). Uses R1-R6, R9, R10.
net_recv:
        mov     r5, r1
        ldv     r6, SV_RXI
        mov     r3, r6
        add     r3, r3
        add     r3, r3
        add     r3, r3
        li      r1, #NET_RXR+6
        add     r3, r1          ; the descriptor's flags
        li      r9, #NETBUF_SEG
        mts     s3, r9
        lwx     r2, (r3)
        li      r1, #$8000
        tst     r2, r1
        beq     .got
        movi    r1, #0
        ret
.got:
        push    r2
        addi    r3, #-2
        lwx     r4, (r3)        ; length
        push    r4
        addi    r3, #-2
        lwx     r3, (r3)        ; buffer offset
.copy:
        cmpi    r4, #0
        beq     .back
        li      r9, #NETBUF_SEG
        mts     s3, r9
        lbx     r1, (r3)
        ldv     r2, SV_CS1
        mts     s3, r2
        sbx     r1, (r5)
        addi    r3, #1
        addi    r5, #1
        addi    r4, #-1
        bra     .copy
.back:
        mov     r3, r6          ; give the descriptor back
        add     r3, r3
        add     r3, r3
        add     r3, r3
        li      r1, #NET_RXR+4
        add     r3, r1
        li      r9, #NETBUF_SEG
        mts     s3, r9
        li      r1, #NET_BUFLEN
        swx     r1, (r3)+
        li      r1, #$8000
        swx     r1, (r3)
        addi    r6, #1
        movi    r1, #15
        and     r6, r1
        stv     r6, SV_RXI
        iowr    RXTAIL, r6
        pop     r1              ; length
        pop     r2              ; flags
        ret
