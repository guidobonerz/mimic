; Foundry, the Cogitatron 16's ROM (256 KiB at FFFB:0000-FFFE:FFFF).
;
; Step 5 build: sets the palette, copies the font to chip RAM, draws a
; mode-0 test picture and a mode-4 text screen, builds a Copper list and
; the Blitter scene's graphics, and shows in turn every 3 seconds: the
; test picture, the text screen, the Copper scene (a colour gradient line
; by line behind the text, the screen split at line 300 into the test
; picture) and the Blitter scene (a scrolling background, moving balls
; with shadows). Later steps turn this into the real Foundry (self test,
; boot loader, monitor; SPEC.md 11).
;
; Built by the machine crate's build script with asm-ada; `font.inc` is
; generated there (8 x 16 font).

; ---- I/O registers (segment $FFFF, SPEC.md 6, 8.2 and 10.5) ----
IO_SEG   = $ffff
DMACON   = $0002
COP1SEG  = $0020
COP1OFS  = $0022
BLTCON0  = $0100
BLTCON1  = $0102
BLTASEG  = $0104
BLTAOFS  = $0106
BLTBSEG  = $0108
BLTBOFS  = $010a
BLTDSEG  = $010c
BLTDOFS  = $010e
BLTAPITCH = $0110
BLTBPITCH = $0112
BLTDPITCH = $0114
BLTW     = $0116
BLTH     = $0118
BLTKEY   = $011a
BLTCOLOR = $011c
BLTTSEG  = $011e
BLTTOFS  = $0120
BLTLINE  = $0122
BLTSTART = $0124
BLTSTAT  = $0126
USE_A    = $10                  ; BLTCON0
USE_B    = $20
USE_D    = $40
OP_FILL  = $100
OP_LINE  = $200
OP_SHADE = $300
SIZE_8   = 1                    ; BLTCON1
KEY_A    = $10
VMODE    = $0800
FBSEG    = $0802
FBOFS    = $0804
PITCH    = $0806
BORDER   = $080a
TXTBANK  = $080e
FONTSEG  = $0810
FONTOFS  = $0812
CURSOR   = $0814
CURSHAPE = $0816
VSTAT    = $081c
FBNOW    = $081e
PALETTE  = $1000
DBGOUT   = $43f0

; ---- chip RAM layout ----
FONT_SEG = $0001                ; font at 0001:0000 (4 KiB)
TEXT_SEG = $0001
TEXT_OFS = $1000                ; text screen at 0001:1000, 100 x 37 cells
TEXT_PITCH = 200
PIC_SEG  = $0002                ; mode-0 picture at 0002:0000 (76 800 bytes)
COPPER_SEG = $0004              ; Copper list at 0004:0000
GFX_SEG  = $0005                ; balls 0005:0000 and :0400, shade table,
SHADE_OFS = $1000               ; the bobs and variables
BOBS_OFS = $2000
VARS_OFS = $2100
BG_SEG   = $0006                ; background 640 x 240 from 0006:0000
BUF0_SEG = $0009                ; the two 320 x 240 buffers
BUF1_SEG = $000b
NBOBS    = 8
XMAX     = 320-32-6             ; the shadow stays on the screen
YMAX     = 240-32-6
STACK_SEG = $003f

ATTR     = $1e                  ; yellow on blue
FIELDS   = 180                  ; 3 seconds

; Write a constant to an I/O register (S3 = IO_SEG). Uses R1, R2.
.macro reg off, value
        li      r2, #\off
        li      r1, #\value
        swx     r1, (r2)
.endm

; Set S3. Uses R1.
.macro seg3 value
        li      r1, #\value
        mts     s3, r1
.endm

        .section rom
        .org    $fffb0000
reset:
        bra     init

        .org    $fffb0040
vectors:                        ; 32 vectors (SPEC.md 3.11): all to `fault`
        .far    fault, fault, fault, fault, fault, fault, fault, fault
        .far    fault, fault, fault, fault, fault, fault, fault, fault
        .far    fault, fault, fault, fault, fault, fault, fault, fault
        .far    fault, fault, fault, fault, fault, fault, fault, fault

init:
        li      r1, #$fffb
        mts     s1, r1          ; S1: ROM data (strings, tables, font)
        li      r1, #STACK_SEG
        mts     s2, r1
        li      r12, #$fffe
        seg3    IO_SEG
        li      r1, #hello
        bl      debug_print
        bl      set_palette
        bl      copy_font
        bl      draw_picture
        bl      draw_text
        bl      build_copper
        seg3    IO_SEG
        reg     DMACON, $c021   ; DMA on: master, framebuffer, Blitter
        bl      make_balls
        bl      make_shade
        bl      init_bobs
        seg3    IO_SEG
        bl      draw_background
        reg     COP1SEG, COPPER_SEG
        reg     COP1OFS, 0
        reg     FONTSEG, FONT_SEG
        reg     FONTOFS, 0
        reg     TXTBANK, 0
        reg     BORDER, 0
show:
        reg     FBSEG, PIC_SEG
        reg     FBOFS, 0
        reg     PITCH, 320
        movi    r5, #1
        bl      wait_fields     ; the pointer is taken over at the VBlank
        reg     VMODE, $8000    ; display on, mode 0
        reg     BORDER, 8
        li      r5, #FIELDS
        bl      wait_fields
        reg     FBSEG, TEXT_SEG
        reg     FBOFS, TEXT_OFS
        reg     PITCH, TEXT_PITCH
        movi    r5, #1
        bl      wait_fields
        reg     VMODE, $8004    ; display on, mode 4
        reg     BORDER, 1
        li      r5, #FIELDS
        bl      wait_fields
        reg     DMACON, $8010   ; Copper on: it starts with the next field
        li      r5, #FIELDS
        bl      wait_fields
        reg     DMACON, $0010   ; Copper off
        reg     PALETTE+2, $0015 ; text colour 1 back to blue
        bl      blitter_scene
        bra     show

; An unexpected exception: stop here.
fault:
        bra     fault

; Wait for R5 VBlanks (VSTAT bit 2, cleared by reading). S3 = IO_SEG.
; Uses R1-R3.
wait_fields:
        li      r2, #VSTAT
        lwx     r1, (r2)        ; forget an earlier VBlank
.next:
        lwx     r1, (r2)
        movi    r3, #4
        tst     r1, r3
        beq     .next
        addi    r5, #-1
        bne     .next
        ret

; Print the zero-terminated string at R1 (ROM) on the debug console.
; S3 = IO_SEG. Uses R1, R2, R4.
debug_print:
        li      r2, #DBGOUT
.c:
        lb      r4, 0(r1)
        cmpi    r4, #0
        beq     .done
        sbx     r4, (r2)
        addi    r1, #1
        bra     .c
.done:
        ret

; Palette: 0-15 text colours, 16-79 grey, 80-143 red, 144-207 green
; (64 steps each), 208-255 blue (48 steps). S3 = IO_SEG. Uses R1-R7.
set_palette:
        li      r2, #PALETTE
        li      r1, #colours
        movi    r3, #16
.text:
        lw      r4, 0(r1)
        swx     r4, (r2)+
        addi    r1, #2
        addi    r3, #-1
        bne     .text
        movi    r5, #0          ; step 0-63
.grey:
        mov     r4, r5
        movi    r6, #1
        shr     r4, r6          ; 5-bit level
        mov     r7, r4
        movi    r6, #11
        shl     r7, r6          ; red
        or      r7, r4          ; blue
        mov     r4, r5
        movi    r6, #5
        shl     r4, r6          ; green: 6 bits
        or      r7, r4
        swx     r7, (r2)+
        addi    r5, #1
        cmpi    r5, #64
        bne     .grey
        movi    r5, #0
.red:
        mov     r7, r5
        movi    r6, #1
        shr     r7, r6
        movi    r6, #11
        shl     r7, r6
        swx     r7, (r2)+
        addi    r5, #1
        cmpi    r5, #64
        bne     .red
        movi    r5, #0
.green:
        mov     r7, r5
        movi    r6, #5
        shl     r7, r6
        swx     r7, (r2)+
        addi    r5, #1
        cmpi    r5, #64
        bne     .green
        movi    r5, #0
.blue:
        mov     r7, r5          ; level = step x 2 / 3 (0-31)
        add     r7, r7
        movi    r6, #3
        divu    r7, r6
        swx     r7, (r2)+
        addi    r5, #1
        cmpi    r5, #48
        bne     .blue
        ret

; Copy the 4 KiB font to FONT_SEG:0000. Uses R1-R4.
copy_font:
        seg3    FONT_SEG
        li      r1, #font
        movi    r2, #0
        li      r3, #4096
.b:
        lb      r4, 0(r1)
        sbx     r4, (r2)+
        addi    r1, #1
        addi    r3, #-1
        bne     .b
        ret

; The mode-0 test picture (320 x 240) at PIC_SEG:0000: 16 colour bars,
; then grey, red, green and blue ramps, a white frame. Uses R1-R7.
draw_picture:
        seg3    PIC_SEG
        movi    r2, #0          ; S3:R2 runs on across segments
        movi    r6, #0          ; y
.row:
        movi    r7, #0          ; x
.pixel:
        cmpi    r6, #60
        bcs     .ramps
        mov     r1, r7          ; bars: x / 20
        movi    r3, #20
        divu    r1, r3
        bra     .frame
.ramps:
        mov     r1, r7          ; ramp step: x / 5
        movi    r3, #5
        divu    r1, r3
        cmpi    r6, #120
        bcs     .not_grey
        addi    r1, #16
        bra     .frame
.not_grey:
        cmpi    r6, #160
        bcs     .not_red
        addi    r1, #80
        bra     .frame
.not_red:
        cmpi    r6, #200
        bcs     .blue
        li      r3, #144
        add     r1, r3
        bra     .frame
.blue:
        mov     r1, r7          ; 48 steps: x x 3 / 20
        movi    r3, #3
        mul     r1, r3
        movi    r3, #20
        divu    r1, r3
        li      r3, #208
        add     r1, r3
.frame:
        cmpi    r7, #0
        beq     .white
        li      r3, #319
        cmp     r7, r3
        beq     .white
        cmpi    r6, #0
        beq     .white
        cmpi    r6, #239
        bne     .put
.white:
        movi    r1, #15
.put:
        sbx     r1, (r2)+
        addi    r7, #1
        li      r3, #320
        cmp     r7, r3
        bne     .pixel
        addi    r6, #1
        cmpi    r6, #240
        bne     .row
        ret

; The text screen. Uses R1-R9.
draw_text:
        push    lr
        seg3    TEXT_SEG
        li      r2, #TEXT_OFS   ; clear: spaces in ATTR
        li      r3, #3700
        movi    r4, #' '
        movi    r5, #ATTR
.clear:
        sbx     r4, (r2)+
        sbx     r5, (r2)+
        addi    r3, #-1
        bne     .clear
        li      r1, #title
        li      r2, #1*100+2
        movi    r3, #$1f
        bl      print
        li      r1, #line2
        li      r2, #2*100+2
        movi    r3, #ATTR
        bl      print
        li      r1, #line3
        li      r2, #4*100+2
        movi    r3, #$17
        bl      print
        li      r1, #charset_label
        li      r2, #6*100+2
        movi    r3, #ATTR
        bl      print
        ; all 256 characters: 32 a row, every second column, rows 7-14
        movi    r5, #0          ; character
.ch:
        mov     r2, r5
        movi    r3, #5
        shr     r2, r3          ; row in the table
        addi    r2, #7
        movi    r3, #100
        mul     r2, r3
        mov     r4, r5
        movi    r3, #31
        and     r4, r3
        add     r4, r4          ; column x 2
        addi    r4, #4
        add     r2, r4          ; cell
        add     r2, r2
        li      r3, #TEXT_OFS
        add     r2, r3
        sbx     r5, (r2)+
        movi    r3, #$1f
        sbx     r3, (r2)
        addi    r5, #1
        li      r3, #256
        cmp     r5, r3
        bne     .ch
        li      r1, #colour_label
        li      r2, #16*100+2
        movi    r3, #ATTR
        bl      print
        ; 8 background x 16 foreground colours: " f " in each
        movi    r6, #0          ; background
.bg:
        movi    r7, #0          ; foreground
.fg:
        mov     r2, r6
        addi    r2, #17
        movi    r3, #100
        mul     r2, r3
        mov     r4, r7
        movi    r3, #3
        mul     r4, r3
        addi    r4, #4
        add     r2, r4
        add     r2, r2
        li      r3, #TEXT_OFS
        add     r2, r3          ; byte offset of the cell
        mov     r8, r6
        movi    r3, #4
        shl     r8, r3
        or      r8, r7          ; attribute
        movi    r4, #' '
        sbx     r4, (r2)+
        sbx     r8, (r2)+
        li      r1, #hex
        add     r1, r7
        lb      r4, 0(r1)
        sbx     r4, (r2)+
        sbx     r8, (r2)+
        movi    r4, #' '
        sbx     r4, (r2)+
        sbx     r8, (r2)+
        addi    r7, #1
        cmpi    r7, #16
        bne     .fg
        addi    r6, #1
        cmpi    r6, #8
        bne     .bg
        li      r1, #blink_text
        li      r2, #26*100+2
        movi    r3, #$80|ATTR   ; blinking
        bl      print
        li      r1, #cursor_text
        li      r2, #28*100+2
        movi    r3, #ATTR
        bl      print
        li      r1, #footer
        li      r2, #35*100+2
        movi    r3, #$17
        bl      print
        seg3    IO_SEG
        reg     CURSOR, 28*100+2+cursor_len
        reg     CURSHAPE, $fe   ; lines 14-15
        pop     lr
        ret

; The Copper list at COPPER_SEG:0000 (SPEC.md 6.2): at the field start
; the text screen (pointer taken over at once with FBNOW), then on every
; line 0-599 a WAIT for its start and a MOVE of palette entry 1 (the text
; background and the border) along a gradient; in the horizontal blank of
; line 299 the split: mode 0 with the test picture from line 300 on; in
; that of line 599 the gradient's first colour for the next field.
; Uses R1-R8.
build_copper:
        seg3    COPPER_SEG
        movi    r2, #0
        li      r1, #cop_head
        movi    r3, #12
.head:
        lw      r4, 0(r1)
        swx     r4, (r2)+
        addi    r1, #2
        addi    r3, #-1
        bne     .head
        movi    r5, #0          ; line
.line:
        li      r4, #$2000      ; WAIT line, 0
        or      r4, r5
        swx     r4, (r2)+
        movi    r4, #0
        swx     r4, (r2)+
        li      r4, #(PALETTE+2)>>1 ; MOVE palette 1
        swx     r4, (r2)+
        mov     r6, r5          ; red: line x 13 / 256
        movi    r7, #13
        mul     r6, r7
        movi    r7, #8
        shr     r6, r7
        movi    r7, #11
        shl     r6, r7
        mov     r8, r5          ; green: line x 20 / 256
        movi    r7, #20
        mul     r8, r7
        movi    r7, #8
        shr     r8, r7
        movi    r7, #5
        shl     r8, r7
        or      r6, r8
        mov     r8, r5          ; blue: 24 - line x 10 / 256
        movi    r7, #10
        mul     r8, r7
        movi    r7, #8
        shr     r8, r7
        movi    r7, #24
        sub     r7, r8
        or      r6, r7
        swx     r6, (r2)+
        li      r4, #299
        cmp     r5, r4
        bne     .next
        li      r1, #cop_split
        movi    r3, #12
.split:
        lw      r4, 0(r1)
        swx     r4, (r2)+
        addi    r1, #2
        addi    r3, #-1
        bne     .split
.next:
        addi    r5, #1
        li      r4, #600
        cmp     r5, r4
        bne     .line
        li      r1, #cop_tail
        movi    r3, #4
.tail:
        lw      r4, 0(r1)
        swx     r4, (r2)+
        addi    r1, #2
        addi    r3, #-1
        bne     .tail
        li      r4, #$c000      ; STOP
        swx     r4, (r2)+
        movi    r4, #0
        swx     r4, (r2)+
        ret

; ---- the Blitter scene (step 5) ----
; A background twice as wide as the screen scrolls by one pixel a field,
; copied by the Blitter into the back buffer; 8 balls move over it, each
; with a shadow darkened through the shade table, then the ball with its
; colour key; the buffers swap at the VBlank (double buffering).

; Write register R to I/O register OFF (S3 = IO_SEG). Uses R9.
.macro wr off, r
        li      r9, #\off
        swx     \r, (r9)
.endm

; Wait for the Blitter (BLTSTAT bit 0). S3 = IO_SEG. Uses R7, R9, R10.
blit_wait:
        li      r9, #BLTSTAT
.w:
        lwx     r10, (r9)
        movi    r7, #1
        tst     r10, r7
        bne     .w
        ret

; Linear address of (R1, R2) in a picture of pitch R6 from segment R8:
; R3 segment, R4 offset. Uses R7.
addr:
        mov     r4, r2
        mul     r4, r6          ; y x pitch, low
        mov     r3, r2
        mulhu   r3, r6          ; high
        add     r4, r1          ; + x
        movi    r7, #0
        adc     r3, r7
        add     r3, r8          ; + base segment
        ret

; Fill a rectangle of the background: D = R3:R4, width R6, height R2,
; colour R1. Uses R7, R9, R10.
fill:
        push    lr
        bl      blit_wait
        li      r7, #OP_FILL
        wr      BLTCON0, r7
        movi    r7, #SIZE_8
        wr      BLTCON1, r7
        wr      BLTDSEG, r3
        wr      BLTDOFS, r4
        li      r7, #640
        wr      BLTDPITCH, r7
        wr      BLTW, r6
        wr      BLTH, r2
        wr      BLTCOLOR, r1
        wr      BLTSTART, r7
        pop     lr
        ret

; A line in the background from D = R3:R4, R6 across, R2 down, BLTLINE
; bits R8, colour R1. Uses R7, R9, R10.
line:
        push    lr
        bl      blit_wait
        li      r7, #OP_LINE
        wr      BLTCON0, r7
        movi    r7, #SIZE_8
        wr      BLTCON1, r7
        wr      BLTDSEG, r3
        wr      BLTDOFS, r4
        li      r7, #640
        wr      BLTDPITCH, r7
        wr      BLTW, r6
        wr      BLTH, r2
        wr      BLTLINE, r8
        wr      BLTCOLOR, r1
        wr      BLTSTART, r7
        pop     lr
        ret

; The background (640 x 240 at BG_SEG), all by the Blitter: 40 bands of
; sky, 20 hills, the ground, 6 rays, then the left half copied to the
; right. S1 = ROM, S3 = IO_SEG. Uses R1-R10.
draw_background:
        push    lr
        movi    r5, #0          ; band
.sky:
        mov     r2, r5
        add     r2, r2
        add     r2, r2          ; y = band x 4
        movi    r1, #0
        li      r6, #640
        li      r8, #BG_SEG
        bl      addr
        li      r1, #208
        add     r1, r5          ; blue ramp, dark to light
        li      r6, #320
        movi    r2, #4
        bl      fill
        addi    r5, #1
        cmpi    r5, #40
        bne     .sky
        movi    r5, #0          ; hill
.hill:
        li      r1, #hills
        add     r1, r5
        lb      r2, 0(r1)       ; height
        push    r2
        li      r1, #240
        sub     r1, r2
        mov     r2, r1          ; y = 240 - height
        mov     r1, r5
        movi    r7, #4
        shl     r1, r7          ; x = hill x 16
        li      r6, #640
        li      r8, #BG_SEG
        bl      addr
        pop     r2
        mov     r1, r5
        movi    r7, #3
        and     r1, r7
        shl     r1, r7          ; (hill & 3) x 8
        li      r7, #160
        add     r1, r7          ; green
        movi    r6, #16
        bl      fill
        addi    r5, #1
        cmpi    r5, #20
        bne     .hill
        movi    r1, #0          ; the ground
        movi    r2, #228
        li      r6, #640
        li      r8, #BG_SEG
        bl      addr
        movi    r1, #6
        li      r6, #320
        movi    r2, #12
        bl      fill
        movi    r5, #0          ; ray
.ray:
        movi    r1, #40
        movi    r2, #24
        li      r6, #640
        li      r8, #BG_SEG
        bl      addr
        li      r1, #rays
        mov     r7, r5
        add     r7, r7
        mov     r10, r7
        add     r7, r7
        add     r7, r10         ; ray x 6
        add     r1, r7
        lw      r6, 0(r1)
        lw      r2, 2(r1)
        lw      r8, 4(r1)
        movi    r1, #14
        bl      line
        addi    r5, #1
        cmpi    r5, #6
        bne     .ray
        bl      blit_wait       ; the left half to the right
        li      r7, #$c|USE_A|USE_D
        wr      BLTCON0, r7
        movi    r7, #SIZE_8
        wr      BLTCON1, r7
        li      r7, #BG_SEG
        wr      BLTASEG, r7
        wr      BLTDSEG, r7
        movi    r7, #0
        wr      BLTAOFS, r7
        li      r7, #320
        wr      BLTDOFS, r7
        wr      BLTW, r7
        li      r7, #640
        wr      BLTAPITCH, r7
        wr      BLTDPITCH, r7
        li      r7, #240
        wr      BLTH, r7
        wr      BLTSTART, r7
        bl      blit_wait
        pop     lr
        ret

; Two 32 x 32 balls at GFX_SEG:0000 (red) and :0400 (steel), lit from the
; upper left, 0 outside. Uses R1-R10.
make_balls:
        push    lr
        seg3    GFX_SEG
        movi    r2, #0
        movi    r10, #80
        bl      ball
        movi    r10, #16
        bl      ball
        pop     lr
        ret

; One ball at S3:R2 on, ramp from R10. Uses R1-R7.
ball:
        movi    r6, #0          ; y
.y:
        movi    r7, #0          ; x
.x:
        mov     r1, r7          ; (2x - 31)^2 + (2y - 31)^2
        add     r1, r1
        addi    r1, #-31
        mul     r1, r1
        mov     r3, r6
        add     r3, r3
        addi    r3, #-31
        mul     r3, r3
        add     r1, r3
        li      r3, #900
        cmp     r1, r3
        bcs     .out
        mov     r4, r7          ; light: (2x - 19)^2 + (2y - 19)^2
        add     r4, r4
        addi    r4, #-19
        mul     r4, r4
        mov     r3, r6
        add     r3, r3
        addi    r3, #-19
        mul     r3, r3
        add     r4, r3
        movi    r3, #40
        divu    r4, r3
        cmpi    r4, #56
        bcc     .lit
        movi    r4, #56
.lit:
        movi    r3, #63
        sub     r3, r4
        add     r3, r10
        bra     .put
.out:
        movi    r3, #0
.put:
        sbx     r3, (r2)+
        addi    r7, #1
        cmpi    r7, #32
        bne     .x
        addi    r6, #1
        cmpi    r6, #32
        bne     .y
        ret

; The shade table at GFX_SEG:SHADE_OFS: the text colours to black, each
; ramp colour 24 steps darker (not below its ramp's start). Uses R1-R5.
make_shade:
        seg3    GFX_SEG
        li      r2, #SHADE_OFS
        movi    r5, #0
.t:
        mov     r1, r5
        cmpi    r5, #16
        bcs     .ramp
        movi    r1, #0
        bra     .put
.ramp:
        movi    r3, #16
        cmpi    r5, #80
        bcc     .sub
        movi    r3, #80
        cmpi    r5, #144
        bcc     .sub
        li      r3, #144
        cmpi    r5, #208
        bcc     .sub
        li      r3, #208
.sub:
        addi    r1, #-24
        cmp     r1, r3
        bge     .put
        mov     r1, r3
.put:
        sbx     r1, (r2)+
        addi    r5, #1
        li      r3, #256
        cmp     r5, r3
        bne     .t
        ret

; The balls' start positions and speeds, and the variables. S1 = ROM.
; Uses R1-R4.
init_bobs:
        seg3    GFX_SEG
        li      r1, #bob_init
        li      r2, #BOBS_OFS
        movi    r3, #4*NBOBS
.c:
        lw      r4, 0(r1)
        swx     r4, (r2)+
        addi    r1, #2
        addi    r3, #-1
        bne     .c
        li      r2, #VARS_OFS
        movi    r4, #0
        swx     r4, (r2)+       ; scroll
        swx     r4, (r2)+       ; buffer parity
        li      r4, #BUF0_SEG
        swx     r4, (r2)+       ; back buffer
        ret

; Bob R5: move it, bouncing off the edges; R1 = x, R2 = y.
; S1 = GFX_SEG. Uses R1-R4, R7, R8.
move_bob:
        mov     r3, r5
        add     r3, r3
        add     r3, r3
        add     r3, r3
        li      r4, #BOBS_OFS
        add     r3, r4
        lw      r1, 0(r3)
        lw      r2, 2(r3)
        lw      r7, 4(r3)
        lw      r8, 6(r3)
        add     r1, r7
        bmi     .xflip
        li      r4, #XMAX
        cmp     r1, r4
        ble     .xok
.xflip:
        neg     r7, r7
        add     r1, r7
        add     r1, r7
.xok:
        add     r2, r8
        bmi     .yflip
        li      r4, #YMAX
        cmp     r2, r4
        ble     .yok
.yflip:
        neg     r8, r8
        add     r2, r8
        add     r2, r8
.yok:
        sw      r1, 0(r3)
        sw      r2, 2(r3)
        sw      r7, 4(r3)
        sw      r8, 6(r3)
        ret

; Bob R5 at (R1, R2) into the back buffer: its shadow 6 pixels down and
; right through the shade table, then the ball. S1 = GFX_SEG, S3 =
; IO_SEG. Uses R1-R10.
draw_bob:
        push    lr
        push    r5
        push    r1
        push    r2
        addi    r1, #6
        addi    r2, #6
        li      r6, #VARS_OFS
        lw      r8, 4(r6)       ; back buffer segment
        li      r6, #320
        bl      addr
        bl      blit_wait
        li      r7, #OP_SHADE|USE_A|USE_B|USE_D
        wr      BLTCON0, r7
        movi    r7, #SIZE_8|KEY_A
        wr      BLTCON1, r7
        movi    r7, #0
        wr      BLTKEY, r7
        lw      r8, 4(r12)      ; the bob's number
        movi    r7, #1
        and     r8, r7
        movi    r7, #10
        shl     r8, r7          ; its ball: x 1024
        li      r7, #GFX_SEG
        wr      BLTASEG, r7
        wr      BLTTSEG, r7
        wr      BLTAOFS, r8
        li      r7, #SHADE_OFS
        wr      BLTTOFS, r7
        movi    r7, #32
        wr      BLTAPITCH, r7
        wr      BLTW, r7
        wr      BLTH, r7
        wr      BLTBSEG, r3
        wr      BLTBOFS, r4
        wr      BLTDSEG, r3
        wr      BLTDOFS, r4
        li      r7, #320
        wr      BLTBPITCH, r7
        wr      BLTDPITCH, r7
        wr      BLTSTART, r7
        pop     r2
        pop     r1
        li      r6, #VARS_OFS
        lw      r8, 4(r6)
        li      r6, #320
        bl      addr
        bl      blit_wait
        li      r7, #$c|USE_A|USE_D
        wr      BLTCON0, r7
        wr      BLTDSEG, r3
        wr      BLTDOFS, r4
        wr      BLTSTART, r7
        pop     r5
        pop     lr
        ret

; One field of the scene into the back buffer, then its pointer for the
; next VBlank. S1 = GFX_SEG, S3 = IO_SEG. Uses R1-R10.
draw_frame:
        push    lr
        li      r1, #VARS_OFS
        lw      r2, 2(r1)
        movi    r3, #1
        xor     r2, r3
        sw      r2, 2(r1)       ; flip the parity
        li      r6, #BUF0_SEG
        cmpi    r2, #0
        beq     .b
        li      r6, #BUF1_SEG
.b:
        sw      r6, 4(r1)       ; back buffer
        lw      r4, 0(r1)       ; scroll
        bl      blit_wait
        li      r7, #$c|USE_A|USE_D
        wr      BLTCON0, r7
        movi    r7, #SIZE_8
        wr      BLTCON1, r7
        li      r7, #BG_SEG
        wr      BLTASEG, r7
        wr      BLTAOFS, r4
        wr      BLTDSEG, r6
        movi    r7, #0
        wr      BLTDOFS, r7
        li      r7, #640
        wr      BLTAPITCH, r7
        li      r7, #320
        wr      BLTDPITCH, r7
        wr      BLTW, r7
        li      r7, #240
        wr      BLTH, r7
        wr      BLTSTART, r7
        li      r1, #VARS_OFS
        addi    r4, #1          ; scroll on, modulo 320
        li      r7, #320
        cmp     r4, r7
        bne     .s
        movi    r4, #0
.s:
        sw      r4, 0(r1)
        movi    r5, #0
.bob:
        bl      move_bob
        bl      draw_bob
        addi    r5, #1
        cmpi    r5, #NBOBS
        bne     .bob
        bl      blit_wait
        li      r1, #VARS_OFS
        lw      r6, 4(r1)
        wr      FBSEG, r6
        movi    r7, #0
        wr      FBOFS, r7
        li      r7, #320
        wr      PITCH, r7
        pop     lr
        ret

; The scene for FIELDS fields. Uses R1-R10.
blitter_scene:
        push    lr
        li      r1, #GFX_SEG
        mts     s1, r1          ; variables
        seg3    IO_SEG
        li      r5, #FIELDS
.f:
        push    r5
        bl      draw_frame
        movi    r5, #1
        bl      wait_fields
        reg     VMODE, $8000
        reg     BORDER, 0
        pop     r5
        addi    r5, #-1
        bne     .f
        li      r1, #$fffb
        mts     s1, r1
        pop     lr
        ret

; Write the zero-terminated string at R1 (ROM) to text cell R2 in
; attribute R3. S3 = TEXT_SEG. Uses R1, R2, R4.
print:
        add     r2, r2
        li      r4, #TEXT_OFS
        add     r2, r4
.c:
        lb      r4, 0(r1)
        cmpi    r4, #0
        beq     .done
        sbx     r4, (r2)+
        sbx     r3, (r2)+
        addi    r1, #1
        bra     .c
.done:
        ret

; ---- data ----
colours:                        ; RGB 5:6:5, the 16 text colours
        .word   $0000, $0015, $0540, $0555, $a800, $a815, $aaa0, $ad55
        .word   $52aa, $52bf, $57ea, $57ff, $faaa, $fabf, $ffea, $ffff
hills:                          ; hill heights
        .byte   40, 52, 66, 74, 70, 58, 46, 40, 48, 62
        .byte   78, 86, 80, 64, 50, 44, 54, 68, 60, 46
        .align  2
rays:                           ; across, down, BLTLINE bits
        .word   150, 20, 0, 150, 60, 0, 120, 100, 0
        .word   60, 120, 4, 20, 130, 4, 90, 40, 0
bob_init:                       ; x, y, dx, dy
        .word   10, 20, 2, 1, 60, 100, -1, 2, 150, 50, 3, -2, 200, 150, -2, -1
        .word   250, 30, 1, 3, 100, 180, 2, -2, 30, 120, -3, 1, 270, 90, -1, -3
cop_head:                       ; 6 MOVEs at the field start
        .word   VMODE>>1, $8004, FBSEG>>1, TEXT_SEG, FBOFS>>1, TEXT_OFS
        .word   PITCH>>1, TEXT_PITCH, FBNOW>>1, 0, BORDER>>1, 1
cop_split:                      ; WAIT 299, 820 (its blank), 5 MOVEs
        .word   $2000|299, 820, VMODE>>1, $8000, FBSEG>>1, PIC_SEG
        .word   FBOFS>>1, 0, PITCH>>1, 320, FBNOW>>1, 0
cop_tail:                       ; line 0's colour, set in line 599's blank
        .word   $2000|599, 820, (PALETTE+2)>>1, $0018
hex:    .ascii  "0123456789ABCDEF"
hello:  .asciz  "Foundry (step 5): test picture, text mode, Copper, Blitter\n"
title:  .asciz  "COGITATRON 16"
line2:  .asciz  "Foundry ROM - step 5: framebuffer, text mode, Copper, Blitter"
line3:  .asciz  "Ada 16-bit RISC at 12 MHz - 4 MiB chip RAM - 28 MiB fast RAM - Lumiere 800 x 600"
charset_label: .asciz "Character set:"
colour_label:  .asciz "Text colours (foreground on background):"
blink_text:    .asciz "This line blinks."
cursor_text:   .asciz "The cursor: "
cursor_len = 12
footer: .asciz  "mimic - Phase 31, step 5"
        .align  2
font:
        .include "font.inc"
