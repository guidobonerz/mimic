; ---- SD card and boot loader (step 9) ----
; The card is FAT32 (MBR partition of type $0B/$0C, or a volume without a
; partition table), 512-byte sectors. The loader reads BOOT/BOILER.BIN to
; 0040:0000 and starts it there (S0 = S1 = 0040, stack 003F:FFFE, IPL 7),
; as the program slot does. 32-bit numbers are register pairs (high, low).

; Read (sd_read) or write (sd_write) R3 blocks from block R1:R2 to/from
; chip RAM R4:0000; wait. R1 = 0 or the error bit. S3 = IO_SEG after.
; Uses R1-R3, R9, R10.
sd_read:
        movi    r10, #1
        bra     sd_go
sd_write:
        movi    r10, #2
sd_go:
        li      r9, #IO_SEG
        mts     s3, r9
        li      r9, #SDBLKH
        swx     r1, (r9)
        li      r9, #SDBLKL
        swx     r2, (r9)
        li      r9, #SDCOUNT
        swx     r3, (r9)
        li      r9, #SDSEG
        swx     r4, (r9)
        li      r9, #SDOFS
        movi    r1, #0
        swx     r1, (r9)
        li      r9, #SDCMD
        swx     r10, (r9)
        li      r9, #SDSTAT
        movi    r2, #2
.w:
        lwx     r1, (r9)
        tst     r1, r2          ; busy
        bne     .w
        movi    r2, #4
        and     r1, r2          ; error
        ret

; Boot from the card. Returns only when it cannot: R1 = 1 no card, 2 no
; FAT32 volume, 3 no BOOT/BOILER.BIN, 4 read error. Uses R1-R10; S1 is
; the ROM again on return.
boot_sd:
        push    lr
        iord    r1, SDSTAT
        movi    r2, #1
        tst     r1, r2
        bne     .card
        movi    r1, #1
        jmp     .out
.card:
        movi    r1, #0
        movi    r2, #0
        movi    r3, #1
        li      r4, #SDBUF_SEG
        call      sd_read
        cmpi    r1, #0
        jne     .rderr
        seg1    SDBUF_SEG
        movi    r7, #0          ; partition start
        movi    r8, #0
        li      r6, #82         ; a volume without a partition table?
        lb      r1, 0(r6)
        cmpi    r1, #'F'
        bne     .mbr
        lb      r1, 3(r6)
        cmpi    r1, #'3'
        bne     .mbr
        lb      r1, 4(r6)
        cmpi    r1, #'2'
        beq     .bpb
.mbr:
        li      r6, #450        ; partition 1's type
        lb      r1, 0(r6)
        cmpi    r1, #$0b
        beq     .part
        cmpi    r1, #$0c
        jne     .nofat
.part:
        li      r6, #454
        lw      r8, 0(r6)
        lw      r7, 2(r6)
        mov     r1, r7
        mov     r2, r8
        movi    r3, #1
        li      r4, #SDBUF_SEG
        call      sd_read
        cmpi    r1, #0
        jne     .rderr
.bpb:
        li      r6, #510
        lw      r1, 0(r6)
        li      r2, #$aa55
        cmp     r1, r2
        jne     .nofat
        movi    r6, #0
        lb      r1, 11(r6)      ; 512 bytes a sector
        cmpi    r1, #0
        jne     .nofat
        lb      r1, 12(r6)
        cmpi    r1, #2
        jne     .nofat
        lb      r1, 13(r6)      ; sectors a cluster: a power of two
        cmpi    r1, #0
        jeq     .nofat
        stv     r1, SV_SPC
        movi    r2, #0
.log:
        cmpi    r1, #1
        beq     .logged
        movi    r3, #1
        tst     r1, r3
        jne     .nofat
        shr     r1, r3
        addi    r2, #1
        bra     .log
.logged:
        stv     r2, SV_SPCSH
        lb      r3, 14(r6)      ; reserved sectors
        lb      r4, 15(r6)
        movi    r1, #8
        shl     r4, r1
        or      r3, r4
        mov     r1, r7          ; FAT = partition + reserved
        mov     r2, r8
        add     r2, r3
        movi    r3, #0
        adc     r1, r3
        stv     r1, SV_FAT
        stv     r2, SV_FAT+2
        li      r6, #16
        lb      r4, 0(r6)       ; FATs
        li      r6, #32
        lw      r3, 4(r6)       ; FAT size (sectors), low
        lw      r5, 6(r6)       ; high
        cmpi    r4, #0
        jeq     .nofat
.fats:
        add     r2, r3
        adc     r1, r5
        addi    r4, #-1
        bne     .fats
        stv     r1, SV_DATA
        stv     r2, SV_DATA+2
        lw      r2, 12(r6)      ; root directory cluster (offset 44)
        lw      r1, 14(r6)
        li      r3, #name_boot
        call      find_entry
        cmpi    r1, #0
        jne     .out
        ldv     r1, SV_CLUS
        ldv     r2, SV_CLUS+2
        li      r3, #name_boiler
        call      find_entry
        cmpi    r1, #0
        jne     .out
        call      load_chain
        cmpi    r1, #0
        jne     .out
        di                      ; start it
        li      r1, #PROG_SEG
        mts     s1, r1
        li      r1, #STACK_SEG
        mts     s2, r1
        li      r12, #$fffe
        li      r1, #PROG_SEG
        movi    r2, #0
        jf      r1:r2
.nofat:
        movi    r1, #2
        bra     .out
.rderr:
        movi    r1, #4
.out:
        seg1    ROM_SEG
        pop     lr
        ret

; Read cluster R7:R8 into SDBUF_SEG:0000. R1 = 0 or an error. Uses
; R1-R4, R9, R10.
read_cluster:
        push    lr
        mov     r1, r7
        mov     r2, r8
        movi    r3, #2          ; cluster - 2
        sub     r2, r3
        movi    r3, #0
        sbc     r1, r3
        ldv     r3, SV_SPCSH
.sh:
        cmpi    r3, #0
        beq     .shifted
        add     r2, r2
        adc     r1, r1
        addi    r3, #-1
        bra     .sh
.shifted:
        ldv     r3, SV_DATA+2   ; + the data area
        add     r2, r3
        ldv     r3, SV_DATA
        adc     r1, r3
        ldv     r3, SV_SPC
        li      r4, #SDBUF_SEG
        call      sd_read
        pop     lr
        ret

; The cluster after R1:R2 in the FAT: R1:R2, R3 = 0, or R3 = 1 at the
; chain's end, 4 on a read error. Uses R1-R4, R9, R10.
next_cluster:
        push    lr
        mov     r4, r2          ; entry offset in its sector: (c & 127) x 4
        li      r3, #127
        and     r4, r3
        add     r4, r4
        add     r4, r4
        mov     r3, r1          ; sector: c >> 7
        movi    r9, #9
        shl     r3, r9
        movi    r9, #7
        shr     r2, r9
        or      r2, r3
        shr     r1, r9
        ldv     r3, SV_FAT+2
        add     r2, r3
        ldv     r3, SV_FAT
        adc     r1, r3
        push    r4
        movi    r3, #1
        li      r4, #FATBUF_SEG
        call      sd_read
        pop     r4
        cmpi    r1, #0
        jne     .err
        seg1    FATBUF_SEG
        lw      r2, 0(r4)
        lw      r1, 2(r4)
        seg1    SDBUF_SEG
        li      r3, #$0fff
        and     r1, r3
        cmp     r1, r3          ; $0FFFFFF8 and up: the end
        bne     .low
        li      r3, #$fff8
        cmp     r2, r3
        bcs     .end
.low:
        cmpi    r1, #0          ; 0 and 1 are no clusters
        bne     .ok
        cmpi    r2, #2
        bcc     .end
.ok:
        movi    r3, #0
        pop     lr
        ret
.end:
        movi    r3, #1
        pop     lr
        ret
.err:
        movi    r3, #4
        pop     lr
        ret

; Find the 8.3 name at R3 (ROM, 11 bytes) in the directory at cluster
; R1:R2: R1 = 0 and its cluster and size in SV_CLUS, SV_SIZE; 3 when it
; is not there, 4 on a read error. S1 = SDBUF_SEG. Uses R1-R10.
find_entry:
        push    lr
        stv     r3, SV_NAME
        mov     r7, r1
        mov     r8, r2
.clus:
        call      read_cluster
        cmpi    r1, #0
        jne     .err
        ldv     r5, SV_SPC      ; entries: sectors x 16
        movi    r1, #4
        shl     r5, r1
        movi    r6, #0
.ent:
        lb      r1, 0(r6)
        cmpi    r1, #0
        beq     .none           ; the directory's end
        cmpi    r1, #$e5
        beq     .skip           ; deleted
        lb      r1, 11(r6)
        movi    r2, #$0f
        and     r2, r1
        cmpi    r2, #$0f
        beq     .skip           ; a long-name entry
        movi    r2, #8
        tst     r1, r2
        bne     .skip           ; the volume label
        ldv     r3, SV_NAME
        li      r9, #ROM_SEG
        mts     s3, r9
        movi    r4, #0
.cmp:
        mov     r1, r6
        add     r1, r4
        lb      r1, 0(r1)
        mov     r2, r3
        add     r2, r4
        lbx     r2, (r2)
        cmp     r1, r2
        bne     .skip
        addi    r4, #1
        cmpi    r4, #11
        bne     .cmp
        lw      r1, 20(r6)      ; found: cluster
        stv     r1, SV_CLUS
        lw      r1, 26(r6)
        stv     r1, SV_CLUS+2
        lw      r1, 30(r6)      ; size
        stv     r1, SV_SIZE
        lw      r1, 28(r6)
        stv     r1, SV_SIZE+2
        movi    r1, #0
        pop     lr
        ret
.skip:
        addi    r6, #32
        addi    r5, #-1
        bne     .ent
        mov     r1, r7          ; the directory's next cluster
        mov     r2, r8
        call      next_cluster
        cmpi    r3, #4
        beq     .err
        cmpi    r3, #0
        bne     .none
        mov     r7, r1
        mov     r8, r2
        bra     .clus
.none:
        movi    r1, #3
        pop     lr
        ret
.err:
        movi    r1, #4
        pop     lr
        ret

; Load the file at SV_CLUS with SV_SIZE bytes to 0040:0000. R1 = 0, 3
; for an empty file, 4 on a read error or a chain too short. S1 =
; SDBUF_SEG. Uses R1-R10.
load_chain:
        push    lr
        li      r1, #PROG_SEG
        stv     r1, SV_DSEG
        movi    r1, #0
        stv     r1, SV_DOFS
        ldv     r1, SV_SIZE
        ldv     r2, SV_SIZE+2
        or      r1, r2
        jeq     .empty
        ldv     r7, SV_CLUS
        ldv     r8, SV_CLUS+2
.next:
        call      read_cluster
        cmpi    r1, #0
        jne     .err
        ldv     r5, SV_SPC      ; words a cluster
        li      r1, #256
        mul     r5, r1
        ldv     r1, SV_SIZE     ; fewer left?
        cmpi    r1, #0
        bne     .full
        ldv     r2, SV_SIZE+2
        mov     r3, r2          ; words left: (bytes + 1) / 2
        movi    r4, #1
        shr     r3, r4
        tst     r2, r4
        beq     .even
        addi    r3, #1
.even:
        cmp     r3, r5
        bcs     .full
        mov     r5, r3
.full:
        ldv     r1, SV_SIZE     ; size -= 2 x words, not below 0
        ldv     r2, SV_SIZE+2
        sub     r2, r5
        movi    r3, #0
        sbc     r1, r3
        bcc     .zero
        sub     r2, r5
        sbc     r1, r3
        bcs     .left
.zero:
        movi    r1, #0
        movi    r2, #0
.left:
        stv     r1, SV_SIZE
        stv     r2, SV_SIZE+2
        ldv     r1, SV_DSEG     ; copy R5 words on, across segments
        ldv     r2, SV_DOFS
        mts     s3, r1
        movi    r6, #0
.cp:
        lw      r4, 0(r6)
        swx     r4, (r2)+
        addi    r6, #2
        addi    r5, #-1
        bne     .cp
        mfs     r1, s3
        mov     r3, r2
        stv     r1, SV_DSEG
        stv     r3, SV_DOFS
        ldv     r1, SV_SIZE
        ldv     r2, SV_SIZE+2
        or      r1, r2
        beq     .done
        mov     r1, r7
        mov     r2, r8
        call      next_cluster
        cmpi    r3, #0
        bne     .err            ; the chain ends before the file
        mov     r7, r1
        mov     r8, r2
        jmp     .next
.done:
        movi    r1, #0
        pop     lr
        ret
.empty:
        movi    r1, #3
        pop     lr
        ret
.err:
        movi    r1, #4
        pop     lr
        ret

name_boot:   .ascii "BOOT       "
name_boiler: .ascii "BOILER  BIN"
