; ---- the emergency monitor (step 9) ----
; Entered when a key is pressed while the start picture shows, by TRAP #0
; and after an unexpected exception (with the registers it found).
; Commands (hex numbers; an address is SSSS:OOOO or OOOO in the last
; segment):
;   M addr [lines]        show memory, 16 bytes a line (8 lines)
;   E addr bb bb ...      enter bytes
;   F addr count bb       fill
;   D addr [count]        disassemble (16)
;   G addr                call a program there (it returns with RETF)
;   R                     the registers saved at entry
;   S R|W block count seg SD card blocks <-> chip RAM seg:0000
;   B                     boot from the SD card
;   X                     restart
;   ?                     help

monitor_start:
        di
        li      r1, #STACK_SEG
        mts     s2, r1
        li      r12, #$fffe
        seg1    ROM_SEG
        ldv     r1, SV_CONON    ; a program's console output stays
        cmpi    r1, #0
        bne     .keep
        call    con_init
.keep:
        li      r1, #mon_title
        call    con_puts
monitor_loop:
        li      r1, #mon_prompt
        call      con_puts
        call      getline
        call      mon_command
        bra     monitor_loop

; After an exception (registers in SV_REGS): show where, then the loop.
monitor_fault:
        li      r1, #STACK_SEG
        mts     s2, r1
        li      r12, #$fffe
        seg1    ROM_SEG
        call      con_init
        li      r1, #mon_title
        call      con_puts
        li      r1, #exc_text
        call      con_puts
        ldv     r1, SV_CAUSE
        call      con_hex2
        li      r1, #at_text
        call      con_puts
        ldv     r1, SV_REGS+24  ; S0
        call      con_hex4
        movi    r1, #':'
        call      con_putc
        ldv     r1, SV_PC
        call      con_hex4
        call      con_nl
        call      show_regs
        bra     monitor_loop

; Read a line into SV_LINE (upper case, zero-terminated).
getline:
        push    lr
        push    r5
        movi    r5, #0
.k:
        call      key_wait
        cmpi    r1, #13
        beq     .end
        cmpi    r1, #8
        beq     .bs
        cmpi    r1, #32
        bcc     .k
        cmpi    r5, #98
        bcs     .k
        push    r1
        call      con_putc
        pop     r1
        cmpi    r1, #'a'
        bcc     .st
        cmpi    r1, #123
        bcs     .st
        addi    r1, #-32
.st:
        li      r9, #SYS_SEG
        mts     s3, r9
        li      r2, #SV_LINE
        add     r2, r5
        sbx     r1, (r2)
        addi    r5, #1
        bra     .k
.bs:
        cmpi    r5, #0
        beq     .k
        addi    r5, #-1
        movi    r1, #8
        call      con_putc
        bra     .k
.end:
        li      r9, #SYS_SEG
        mts     s3, r9
        li      r2, #SV_LINE
        add     r2, r5
        movi    r1, #0
        sbx     r1, (r2)
        call      con_nl
        pop     r5
        pop     lr
        ret

; R1 = the line's character at R5 (0 at its end). Uses R9.
line_ch:
        li      r9, #SYS_SEG
        mts     s3, r9
        li      r9, #SV_LINE
        add     r9, r5
        lbx     r1, (r9)
        ret

skip_sp:
        push    lr
.s:
        call      line_ch
        cmpi    r1, #' '
        bne     .d
        addi    r5, #1
        bra     .s
.d:
        pop     lr
        ret

; A hex number at R5: R1 low, R2 high word, R3 digits (0: none).
parse_hex:
        push    lr
        push    r6
        push    r7
        call      skip_sp
        movi    r6, #0
        movi    r7, #0
        movi    r3, #0
.d:
        call      line_ch
        cmpi    r1, #'0'
        bcc     .e
        cmpi    r1, #58
        bcs     .letter
        addi    r1, #-48
        bra     .add
.letter:
        cmpi    r1, #'A'
        bcc     .e
        cmpi    r1, #71
        bcs     .e
        addi    r1, #-55
.add:
        mov     r4, r6
        movi    r2, #12
        shr     r4, r2
        movi    r2, #4
        shl     r7, r2
        or      r7, r4
        shl     r6, r2
        or      r6, r1
        addi    r3, #1
        addi    r5, #1
        bra     .d
.e:
        mov     r1, r6
        mov     r2, r7
        pop     r7
        pop     r6
        pop     lr
        ret

; An address at R5: R1 offset, R2 segment, R3 = 1 (0: none). The segment
; becomes the current one.
arg_addr:
        push    lr
        call      parse_hex
        cmpi    r3, #0
        beq     .none
        mov     r4, r1
        call      line_ch
        cmpi    r1, #':'
        bne     .off
        addi    r5, #1
        push    r4
        call      parse_hex
        pop     r2
        cmpi    r3, #0
        beq     .none
        bra     .set
.off:
        mov     r1, r4
        ldv     r2, SV_MSEG
.set:
        mov     r4, r1
        stv     r2, SV_MSEG
        mov     r1, r4
        movi    r3, #1
        pop     lr
        ret
.none:
        movi    r3, #0
        pop     lr
        ret

mon_command:
        push    lr
        movi    r5, #0
        call      skip_sp
        call      line_ch
        cmpi    r1, #0
        beq     .done
        addi    r5, #1
        cmpi    r1, #'M'
        bne     .e
        call      cmd_dump
        jmp     .done
.e:
        cmpi    r1, #'E'
        bne     .f
        call      cmd_enter
        jmp     .done
.f:
        cmpi    r1, #'F'
        bne     .d
        call      cmd_fill
        jmp     .done
.d:
        cmpi    r1, #'D'
        bne     .g
        call      cmd_disasm
        jmp     .done
.g:
        cmpi    r1, #'G'
        bne     .r
        call      cmd_go
        jmp     .done
.r:
        cmpi    r1, #'R'
        bne     .s
        call      show_regs
        jmp     .done
.s:
        cmpi    r1, #'S'
        bne     .b
        call      cmd_sd
        jmp     .done
.b:
        cmpi    r1, #'B'
        bne     .x
        call      boot_sd
        call      boot_message
        jmp     .done
.x:
        cmpi    r1, #'X'
        bne     .h
        li      r1, #ROM_SEG
        li      r2, #reboot
        jf      r1:r2
.h:
        cmpi    r1, #'?'
        bne     .what
        li      r1, #mon_help
        call      con_puts
        jmp     .done
.what:
        call      what
.done:
        pop     lr
        ret

what:
        li      r1, #what_text
        jmp     con_puts

; R1 = the byte at R7:R1.
peek_byte:
        mts     s3, r7
        lbx     r1, (r1)
        ret

cmd_dump:
        push    lr
        push    r6
        push    r7
        push    r8
        call      arg_addr
        cmpi    r3, #0
        beq     .bad
        mov     r7, r2
        mov     r6, r1
        call      parse_hex
        mov     r8, r1
        cmpi    r3, #0
        bne     .line
        movi    r8, #8
.line:
        mov     r1, r7
        call      con_hex4
        movi    r1, #':'
        call      con_putc
        mov     r1, r6
        call      con_hex4
        call      con_space
        movi    r5, #0
.hx:
        call      con_space
        mov     r1, r6
        add     r1, r5
        call      peek_byte
        call      con_hex2
        addi    r5, #1
        cmpi    r5, #16
        bne     .hx
        call      con_space
        call      con_space
        movi    r5, #0
.as:
        mov     r1, r6
        add     r1, r5
        call      peek_byte
        cmpi    r1, #32
        bcc     .dot
        cmpi    r1, #127
        bcc     .pr
.dot:
        movi    r1, #'.'
.pr:
        call      con_putc
        addi    r5, #1
        cmpi    r5, #16
        bne     .as
        call      con_nl
        addi    r6, #16
        addi    r8, #-1
        bne     .line
        bra     .out
.bad:
        call      what
.out:
        pop     r8
        pop     r7
        pop     r6
        pop     lr
        ret

cmd_enter:
        push    lr
        push    r6
        push    r7
        call      arg_addr
        cmpi    r3, #0
        beq     .bad
        mov     r7, r2
        mov     r6, r1
.b:
        call      parse_hex
        cmpi    r3, #0
        beq     .out
        mts     s3, r7
        sbx     r1, (r6)
        addi    r6, #1
        bra     .b
.bad:
        call      what
.out:
        pop     r7
        pop     r6
        pop     lr
        ret

cmd_fill:
        push    lr
        push    r6
        push    r7
        push    r8
        call      arg_addr
        cmpi    r3, #0
        beq     .bad
        mov     r7, r2
        mov     r6, r1
        call      parse_hex
        cmpi    r3, #0
        beq     .bad
        mov     r8, r1
        call      parse_hex
        cmpi    r3, #0
        beq     .bad
        mts     s3, r7
        cmpi    r8, #0
        beq     .out
.f:
        sbx     r1, (r6)+
        addi    r8, #-1
        bne     .f
        bra     .out
.bad:
        call      what
.out:
        pop     r8
        pop     r7
        pop     r6
        pop     lr
        ret

cmd_go:
        push    lr
        call      arg_addr
        cmpi    r3, #0
        beq     .bad
        mts     s1, r2
        callf   r2:r1
        seg1    ROM_SEG
        li      r1, #STACK_SEG
        mts     s2, r1
        li      r1, #back_text
        call      con_puts
        pop     lr
        ret
.bad:
        call      what
        pop     lr
        ret

; S R|W block count seg
cmd_sd:
        push    lr
        push    r6
        push    r7
        push    r8
        call      skip_sp
        call      line_ch
        mov     r8, r1
        addi    r5, #1
        call      parse_hex       ; block
        cmpi    r3, #0
        beq     .bad
        mov     r6, r1
        mov     r7, r2
        call      parse_hex       ; count
        cmpi    r3, #0
        beq     .bad
        push    r1
        call      parse_hex       ; segment
        mov     r4, r1
        pop     r1
        cmpi    r3, #0
        beq     .bad
        mov     r3, r1
        mov     r1, r7
        mov     r2, r6
        cmpi    r8, #'R'
        bne     .w
        call      sd_read
        bra     .res
.w:
        cmpi    r8, #'W'
        bne     .bad
        call      sd_write
.res:
        cmpi    r1, #0
        bne     .err
        li      r1, #ok_text
        call      con_puts
        bra     .out
.err:
        li      r1, #sd_err_text
        call      con_puts
        bra     .out
.bad:
        call      what
.out:
        pop     r8
        pop     r7
        pop     r6
        pop     lr
        ret

; The registers saved in SV_REGS, six a line.
show_regs:
        push    lr
        push    r5
        push    r6
        movi    r5, #0
.r:
        mov     r1, r5
        movi    r2, #6
        mul     r1, r2
        li      r2, #reg_names
        add     r1, r2
        call      con_puts
        movi    r1, #'='
        call      con_putc
        mov     r1, r5
        add     r1, r1
        li      r2, #SV_REGS
        add     r1, r2
        li      r9, #SYS_SEG
        mts     s3, r9
        lwx     r1, (r1)
        call      con_hex4
        call      con_space
        addi    r5, #1
        mov     r1, r5
        movi    r2, #6
        remu    r1, r2
        bne     .same
        call      con_nl
.same:
        cmpi    r5, #19
        bne     .r
        call      con_nl
        pop     r6
        pop     r5
        pop     lr
        ret

; What a failed boot found (R1 = boot_sd's code).
boot_message:
        cmpi    r1, #0
        beq     .none
        addi    r1, #-1
        add     r1, r1
        li      r2, #boot_msgs
        add     r1, r2
        lw      r1, 0(r1)
        jmp     con_puts
.none:
        ret

; ---- disassembler ----
cmd_disasm:
        push    lr
        push    r6
        push    r7
        push    r8
        call      arg_addr
        cmpi    r3, #0
        beq     .bad
        mov     r7, r2
        mov     r6, r1
        call      parse_hex
        mov     r8, r1
        cmpi    r3, #0
        bne     .i
        movi    r8, #16
.i:
        call      dis_one
        addi    r8, #-1
        bne     .i
        bra     .out
.bad:
        call      what
.out:
        pop     r8
        pop     r7
        pop     r6
        pop     lr
        ret

; One instruction at R7:R6 on a line; R6 moves on. Uses R1-R5, R8? no:
; keeps R8 (saved).
dis_one:
        push    lr
        push    r5
        push    r8
        mov     r1, r7
        call      con_hex4
        movi    r1, #':'
        call      con_putc
        mov     r1, r6
        call      con_hex4
        call      con_space
        call      con_space
        mts     s3, r7
        lwx     r8, (r6)
        mov     r1, r8
        call      con_hex4
        call      con_space
        call      con_space
        addi    r6, #2
        mov     r5, r8          ; R5 = op
        movi    r1, #12
        shr     r5, r1
        cmpi    r5, #0
        bne     .imm
        call      d_fn
        li      r2, #t_alu
        call      d_name
        call      d_rd_rs
        jmp     .done
.imm:
        cmpi    r5, #5
        bcs     .mem
        mov     r1, r5
        addi    r1, #-1
        li      r2, #t_imm
        call      d_name
        call      d_rd
        call      d_comma
        movi    r1, #'#'
        call      con_putc
        call      d_dollar
        li      r1, #$ff
        and     r1, r8
        call      con_hex2
        jmp     .done
.mem:
        cmpi    r5, #9
        bcs     .x
        mov     r1, r5
        addi    r1, #-5
        li      r2, #t_mem
        call      d_name
        call      d_rd
        call      d_comma
        call      d_dollar
        call      d_fn
        cmpi    r5, #7
        bcc     .scaled         ; LW, SW: 2 x Off4
        bra     .off
.scaled:
        add     r1, r1
.off:
        call      con_hex2
        movi    r1, #'('
        call      con_putc
        call      d_rs_n
        call      d_reg
        movi    r1, #')'
        call      con_putc
        jmp     .done
.x:
        cmpi    r5, #9
        bne     .br
        call      d_fn
        cmpi    r1, #8
        bcc     .ok1
        jmp     .ill
.ok1:
        movi    r2, #3
        and     r1, r2
        li      r2, #t_x
        call      d_name
        call      d_rd
        call      d_comma
        movi    r1, #'('
        call      con_putc
        call      d_rs_n
        call      d_reg
        movi    r1, #')'
        call      con_putc
        call      d_fn
        movi    r2, #4
        tst     r1, r2
        beq     .done
        movi    r1, #'+'
        call      con_putc
        jmp     .done
.br:
        cmpi    r5, #10
        bne     .bl
        call      d_rd_n
        cmpi    r1, #15
        bne     .ok2
        jmp     .ill
.ok2:
        li      r2, #t_cond
        call      d_name
        li      r1, #$ff        ; target: next + 2 x sign-extended Off8
        and     r1, r8
        movi    r2, #$80
        tst     r1, r2
        beq     .pos8
        li      r2, #$ff00
        or      r1, r2
.pos8:
        bra     .target
.bl:
        cmpi    r5, #11
        bne     .c
        li      r1, #t_bl
        call      d_text
        li      r1, #$0fff
        and     r1, r8
        li      r2, #$0800
        tst     r1, r2
        beq     .target
        li      r2, #$f000
        or      r1, r2
.target:
        add     r1, r1
        add     r1, r6
        push    r1
        call      d_dollar
        pop     r1
        call      con_hex4
        jmp     .done
.c:
        cmpi    r5, #12
        bne     .md
        call      dis_c
        jmp     .done
.md:
        cmpi    r5, #13
        bne     .sys
        call      d_fn
        cmpi    r1, #10
        bcc     .mdok
        jmp     .ill
.mdok:
        li      r2, #t_md
        call      d_name
        call      d_rd_rs
        jmp     .done
.sys:
        cmpi    r5, #14
        beq     .ok3
        jmp     .ill
.ok3:
        call      dis_e
        jmp     .done
.ill:
        call      d_illegal
.done:
        call      con_nl
        pop     r8
        pop     r5
        pop     lr
        ret

; Op C: jumps and segments (R8 the word).
dis_c:
        push    lr
        call      d_fn
        cmpi    r1, #6
        bcs     .ill
        mov     r5, r1
        li      r2, #t_c
        call      d_name
        cmpi    r5, #0          ; jr rs
        bne     .jalr
        call      d_rs_n
        call      d_reg
        bra     .out
.jalr:
        cmpi    r5, #1
        bne     .mts
        call      d_rd_rs
        bra     .out
.mts:
        cmpi    r5, #2
        bne     .mfs
        call      d_rd_n
        movi    r2, #3
        and     r1, r2
        call      d_seg
        call      d_comma
        call      d_rs_n
        call      d_reg
        bra     .out
.mfs:
        cmpi    r5, #3
        bne     .far
        call      d_rd
        call      d_comma
        call      d_rs_n
        movi    r2, #3
        and     r1, r2
        call      d_seg
        bra     .out
.far:
        call      d_rd            ; jf, callf: rd:rs
        movi    r1, #':'
        call      con_putc
        call      d_rs_n
        call      d_reg
        bra     .out
.ill:
        call      d_illegal
.out:
        pop     lr
        ret

; Op E: system (R8 the word).
dis_e:
        push    lr
        call      d_fn
        cmpi    r1, #11
        bcs     .ill
        mov     r5, r1
        li      r2, #t_sys
        call      d_name
        cmpi    r5, #0          ; trap #n
        bne     .mtsr
        movi    r1, #'#'
        call      con_putc
        call      d_dollar
        mov     r1, r8
        movi    r2, #4
        shr     r1, r2
        li      r2, #$ff
        and     r1, r2
        call      con_hex2
        bra     .out
.mtsr:
        cmpi    r5, #5
        bne     .mfsr
        call      d_rs_n
        call      d_reg
        bra     .out
.mfsr:
        cmpi    r5, #6
        bne     .mfsc
        call      d_rd
        bra     .out
.mfsc:
        cmpi    r5, #7
        bne     .mtsc
        call      d_rd
        call      d_comma
        movi    r1, #'#'
        call      con_putc
        call      d_rs_n
        call      con_digit
        bra     .out
.mtsc:
        cmpi    r5, #8
        bne     .out            ; rti, ei, di, halt, nop, brk
        movi    r1, #'#'
        call      con_putc
        call      d_rd_n
        call      con_digit
        call      d_comma
        call      d_rs_n
        call      d_reg
        bra     .out
.ill:
        call      d_illegal
.out:
        pop     lr
        ret

; Fields of R8: d_fn R1 = bits 3-0, d_rs_n bits 7-4, d_rd_n bits 11-8.
d_fn:
        movi    r1, #15
        and     r1, r8
        ret
d_rs_n:
        mov     r1, r8
        movi    r2, #4
        shr     r1, r2
        movi    r2, #15
        and     r1, r2
        ret
d_rd_n:
        mov     r1, r8
        movi    r2, #8
        shr     r1, r2
        movi    r2, #15
        and     r1, r2
        ret

; Mnemonic R1 of table R2 (6-byte entries) and a space.
d_name:
        movi    r3, #6
        mul     r1, r3
        add     r1, r2
d_text:
        push    lr
        call      con_puts
        call      con_space
        pop     lr
        ret

d_rd:
        push    lr
        call      d_rd_n
        call      d_reg
        pop     lr
        ret

d_rd_rs:
        push    lr
        call      d_rd
        call      d_comma
        call      d_rs_n
        call      d_reg
        pop     lr
        ret

; "rN" for R1 = N.
d_reg:
        push    lr
        push    r1
        movi    r1, #'r'
        call      con_putc
        pop     r1
        cmpi    r1, #10
        bcc     .one
        push    r1
        movi    r1, #'1'
        call      con_putc
        pop     r1
        addi    r1, #-10
.one:
        call      con_digit
        pop     lr
        ret

; "sN" for R1 = N.
d_seg:
        push    lr
        push    r1
        movi    r1, #'s'
        call      con_putc
        pop     r1
        call      con_digit
        pop     lr
        ret

con_digit:
        addi    r1, #'0'
        jmp     con_putc

d_comma:
        push    lr
        movi    r1, #','
        call      con_putc
        call      con_space
        pop     lr
        ret

d_dollar:
        movi    r1, #'$'
        jmp     con_putc

d_illegal:
        push    lr
        li      r1, #t_word
        call      d_text
        call      d_dollar
        mov     r1, r8
        call      con_hex4
        pop     lr
        ret

; ---- texts ----
mon_title:   .asciz "Logophor 16 - Foundry emergency monitor (? for help)\n"
mon_prompt:  .asciz "> "
mon_help:    .ascii "M addr [lines]  memory      E addr bb ..  enter bytes   F addr count bb  fill\n"
             .ascii "D addr [count]  disassemble G addr        call (RETF)   R  registers\n"
             .ascii "S R|W block count seg  SD card <-> chip RAM seg:0000    B  boot   X  restart\n"
             .asciz "Numbers are hex; an address is SSSS:OOOO or OOOO in the last segment.\n"
what_text:   .asciz "?\n"
ok_text:     .asciz "ok\n"
sd_err_text: .asciz "SD card error\n"
back_text:   .asciz "back in the monitor\n"
exc_text:    .asciz "Exception $"
at_text:     .asciz " at "
boot_msgs:   .word  boot_msg1, boot_msg2, boot_msg3, boot_msg4
boot_msg1:   .asciz "No SD card.\n"
boot_msg2:   .asciz "The SD card has no FAT32 volume.\n"
boot_msg3:   .asciz "No BOOT/BOILER.BIN on the SD card.\n"
boot_msg4:   .asciz "The SD card could not be read.\n"
reg_names:   .asciz "R1   "
             .asciz "R2   "
             .asciz "R3   "
             .asciz "R4   "
             .asciz "R5   "
             .asciz "R6   "
             .asciz "R7   "
             .asciz "R8   "
             .asciz "R9   "
             .asciz "R10  "
             .asciz "R11  "
             .asciz "R12  "
             .asciz "S0   "
             .asciz "S1   "
             .asciz "S2   "
             .asciz "S3   "
             .asciz "PC   "
             .asciz "SR   "
             .asciz "CAUSE"
t_alu:  .asciz  "add  "
        .asciz  "sub  "
        .asciz  "adc  "
        .asciz  "sbc  "
        .asciz  "and  "
        .asciz  "or   "
        .asciz  "xor  "
        .asciz  "not  "
        .asciz  "shl  "
        .asciz  "shr  "
        .asciz  "asr  "
        .asciz  "rol  "
        .asciz  "mov  "
        .asciz  "cmp  "
        .asciz  "tst  "
        .asciz  "mul  "
t_imm:  .asciz  "addi "
        .asciz  "movi "
        .asciz  "lui  "
        .asciz  "cmpi "
t_mem:  .asciz  "lw   "
        .asciz  "sw   "
        .asciz  "lb   "
        .asciz  "sb   "
t_x:    .asciz  "lwx  "
        .asciz  "swx  "
        .asciz  "lbx  "
        .asciz  "sbx  "
t_cond: .asciz  "beq  "
        .asciz  "bne  "
        .asciz  "bcs  "
        .asciz  "bcc  "
        .asciz  "bmi  "
        .asciz  "bpl  "
        .asciz  "bvs  "
        .asciz  "bvc  "
        .asciz  "bhi  "
        .asciz  "bls  "
        .asciz  "bge  "
        .asciz  "blt  "
        .asciz  "bgt  "
        .asciz  "ble  "
        .asciz  "bra  "
t_bl:   .asciz  "bl   "
t_c:    .asciz  "jr   "
        .asciz  "jalr "
        .asciz  "mts  "
        .asciz  "mfs  "
        .asciz  "jf   "
        .asciz  "callf"
t_md:   .asciz  "mulhu"
        .asciz  "mulhs"
        .asciz  "divu "
        .asciz  "divs "
        .asciz  "remu "
        .asciz  "rems "
        .asciz  "neg  "
        .asciz  "sxb  "
        .asciz  "zxb  "
        .asciz  "swab "
t_sys:  .asciz  "trap "
        .asciz  "rti  "
        .asciz  "ei   "
        .asciz  "di   "
        .asciz  "halt "
        .asciz  "mtsr "
        .asciz  "mfsr "
        .asciz  "mfsc "
        .asciz  "mtsc "
        .asciz  "nop  "
        .asciz  "brk  "
t_word: .asciz  ".word"
        .align  2
