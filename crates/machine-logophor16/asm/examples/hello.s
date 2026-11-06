; A first program from the SD card: Foundry's boot loader loads it as
; BOOT/BOILER.BIN at 0040:0000. It greets on the console (TRAP #4) and
; then types what is typed (TRAP #2, TRAP #1).
;
;   cargo run --release -p asm-ada -- build crates/machine-logophor16/asm/examples/hello.s -o hello.bin
;   cargo run --release -p asm-ada -- sd card.img --boot hello.bin
;   cargo run --release -p emu-desktop -- --profile logophor16 --disk card.img

        .section text
        .org    $00400000
start:
        li      r1, #greeting
        trap    #4              ; puts
        li      r1, #$ffff      ; and on the debug console
        mts     s3, r1
        li      r2, #$43f0
        li      r1, #greeting
.dbg:
        lb      r3, 0(r1)
        cmpi    r3, #0
        beq     .typing
        sbx     r3, (r2)
        addi    r1, #1
        bra     .dbg
.typing:
        trap    #2              ; wait for a key
        trap    #1              ; and show it
        bra     .typing

greeting:
        .ascii  "Logophor 16 - booted from the SD card.\n"
        .asciz  "Type something:\n"
