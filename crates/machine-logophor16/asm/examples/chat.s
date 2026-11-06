; A chat between Logophor 16s over the network (step 10). Every line
; typed goes out as a broadcast frame of type $4C31 ("L1") with the
; sender's name; lines from the others show up as they come.
;
;   cargo run --release -p asm-ada -- hub        (one, in its own window)
;   cargo run --release -p asm-ada -- build crates/machine-logophor16/asm/examples/chat.s -o chat.bin
;   cargo run --release -p asm-ada -- sd chat.img --boot chat.bin
;   cargo run --release -p emu-desktop -- --profile logophor16 --disk chat.img --option net=127.0.0.1:6516
;   (a second machine the same way, with its own copy of the card)

NAME_MAX = 12
LINE_MAX = 60

        .section text
        .org    $00400000
start:
        trap    #7              ; network on
        li      r1, #title
        trap    #4
        li      r1, #ask_name
        trap    #4
        li      r5, #name       ; read the name
        movi    r6, #0
.name:
        trap    #2
        cmpi    r1, #13
        beq     .named
        cmpi    r1, #8
        beq     .nameback
        cmpi    r6, #NAME_MAX
        bcs     .name
        cmpi    r1, #32
        bcc     .name
        sb      r1, 0(r5)
        addi    r5, #1
        addi    r6, #1
        trap    #1
        bra     .name
.nameback:
        cmpi    r6, #0
        beq     .name
        addi    r5, #-1
        addi    r6, #-1
        trap    #1
        bra     .name
.named:
        cmpi    r6, #0
        beq     .name
        movi    r1, #0
        sb      r1, 0(r5)
        li      r1, #hint
        trap    #4
        bl      prompt
main:
        li      r1, #rxbuf      ; a line from someone?
        trap    #9
        cmpi    r1, #0
        beq     .key
        li      r2, #rxbuf+12   ; our type only
        lb      r3, 0(r2)
        cmpi    r3, #$4c
        bne     .key
        lb      r3, 1(r2)
        cmpi    r3, #$31
        bne     .key
        li      r2, #rxbuf      ; end it: zero after the frame
        add     r2, r1
        movi    r3, #0
        sb      r3, 0(r2)
        bl      erase_line      ; the prompt and what was typed away
        li      r1, #rxbuf+14
        trap    #4
        movi    r1, #10
        trap    #1
        bl      prompt
        bl      show_line       ; and back again
.key:
        trap    #3
        cmpi    r1, #0
        beq     main
        cmpi    r1, #13
        beq     .send
        cmpi    r1, #8
        beq     .back
        cmpi    r1, #32
        bcc     main
        li      r2, #len
        lw      r3, 0(r2)
        cmpi    r3, #LINE_MAX
        bcs     main
        li      r4, #line
        add     r4, r3
        sb      r1, 0(r4)
        addi    r3, #1
        sw      r3, 0(r2)
        trap    #1
        bra     main
.back:
        li      r2, #len
        lw      r3, 0(r2)
        cmpi    r3, #0
        beq     main
        addi    r3, #-1
        sw      r3, 0(r2)
        trap    #1
        bra     main
.send:
        bl      send_line
        movi    r1, #10
        trap    #1
        bl      prompt
        bra     main

; Frame: broadcast, type "L1", "name: line", 0. Uses R1-R6.
send_line:
        push    lr
        li      r4, #txbuf+14
        li      r5, #name
.n:
        lb      r1, 0(r5)
        cmpi    r1, #0
        beq     .colon
        sb      r1, 0(r4)
        addi    r4, #1
        addi    r5, #1
        bra     .n
.colon:
        movi    r1, #':'
        sb      r1, 0(r4)
        movi    r1, #' '
        sb      r1, 1(r4)
        addi    r4, #2
        li      r2, #len
        lw      r6, 0(r2)
        li      r5, #line
.l:
        cmpi    r6, #0
        beq     .end
        lb      r1, 0(r5)
        sb      r1, 0(r4)
        addi    r4, #1
        addi    r5, #1
        addi    r6, #-1
        bra     .l
.end:
        movi    r1, #0
        sb      r1, 0(r4)
        addi    r4, #1
        li      r1, #txbuf
        mov     r2, r4
        sub     r2, r1          ; length
        trap    #8
        li      r2, #len
        movi    r1, #0
        sw      r1, 0(r2)
        pop     lr
        ret

prompt:
        li      r1, #prompt_text
        trap    #4
        ret

; Back over the prompt and the line typed so far.
erase_line:
        li      r2, #len
        lw      r6, 0(r2)
        addi    r6, #2
.b:
        movi    r1, #8
        trap    #1
        addi    r6, #-1
        bne     .b
        ret

; Show the line typed so far (after a message came in).
show_line:
        push    lr
        li      r2, #len
        lw      r6, 0(r2)
        li      r5, #line
.c:
        cmpi    r6, #0
        beq     .done
        lb      r1, 0(r5)
        trap    #1
        addi    r5, #1
        addi    r6, #-1
        bra     .c
.done:
        pop     lr
        ret

title:       .asciz "Logophor 16 chat over Telegraph's network\n"
ask_name:    .asciz "Your name: "
hint:        .asciz "\nType a line and Enter; lines from the others appear as they come.\n"
prompt_text: .asciz "> "
        .align  2
len:    .word   0
name:   .space  NAME_MAX+2
line:   .space  LINE_MAX+2
        .align  2
txbuf:  .byte   $ff, $ff, $ff, $ff, $ff, $ff   ; to everyone
        .byte   $02, $4c, $31, $36, $00, $00   ; from (the hub knows)
        .byte   $4c, $31                       ; type "L1"
        .space  NAME_MAX+LINE_MAX+4
        .align  2
rxbuf:  .space  1540
