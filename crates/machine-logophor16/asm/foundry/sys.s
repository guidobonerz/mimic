; Foundry's system part (step 9): constants, system variables, macros.
; Included at the top of foundry.s.

; ---- Telegraph (SPEC.md 10) ----
TSTAT    = $4220
KBDDATA  = $4240
KBDSTAT  = $4242
SDSTAT   = $4260
SDCMD    = $4262
SDBLKH   = $4264
SDBLKL   = $4266
SDCOUNT  = $4268
SDSEG    = $426a
SDOFS    = $426c
AUDMASTER = $3100
NETCON   = $4000                ; network (SPEC.md 10.1)
TXRSEG   = $4008
TXROFS   = $400a
RXRSEG   = $400c
RXROFS   = $400e
TXTAIL   = $4010
TXHEAD   = $4012
RXHEAD   = $4014
RXTAIL   = $4016
NETSTAT  = $4018

; ---- memory ----
SYS_SEG  = $003e                ; system variables (below the stack)
SDBUF_SEG = $001a               ; SD buffer: a cluster (up to 64 KiB)
LOGO_SEG = $001b                ; start picture, 320 x 240 (76 800 bytes)
FATBUF_SEG = $001d              ; one FAT sector
NETBUF_SEG = $001e              ; network rings and buffers (net.s)
PROG_SEG = $0040                ; programs load and start at 0040:0000
ROM_SEG  = $fffb
DATA_SEG = $fffc                ; ROM data: zeppelin and music
LOGO_ROM = $fffd                ; ROM data: the start picture

LOGO_FIELDS = 120               ; the start picture shows 2 s
CON_ATTR = $0a                  ; monitor text: light green on black

; System variables at SYS_SEG:0000.
SV_SHIFT = $00                  ; shift keys held (bit 0 left, 1 right)
SV_CARD  = $02                  ; a card was in the slot at boot
SV_ERR   = $04                  ; why the boot failed (1-4), 0 = no try
SV_CUR   = $06                  ; console cursor (cell)
SV_MSEG  = $08                  ; the monitor's current segment
SV_CONON = $0a                  ; the console is set up
SV_CS1   = $0c                  ; S1 of a TRAP's caller
SV_REGS  = $10                  ; R1-R12, S0-S3, PC, SR, CAUSE (19 words)
SV_PC    = $30
SV_SR    = $32
SV_CAUSE = $34
SV_PART  = $40                  ; FAT32: high, low words
SV_FAT   = $44
SV_DATA  = $48
SV_SPCSH = $4c                  ; log2 of the sectors a cluster
SV_SPC   = $4e
SV_CLUS  = $50
SV_SIZE  = $54
SV_DSEG  = $58                  ; where the next loaded word goes
SV_DOFS  = $5a
SV_NAME  = $5c                  ; the 11-byte name sought (ROM offset)
SV_TXI   = $60                  ; network: next transmit descriptor
SV_RXI   = $62                  ; next receive descriptor
SV_LINE  = $100                 ; the monitor's input line (100 bytes)

; R = system variable (S3 = SYS_SEG afterwards). Uses R9.
.macro ldv r, var
        li      r9, #SYS_SEG
        mts     s3, r9
        li      r9, #\var
        lwx     \r, (r9)
.endm

; System variable = R (S3 = SYS_SEG afterwards). Uses R9.
.macro stv r, var
        li      r9, #SYS_SEG
        mts     s3, r9
        li      r9, #\var
        swx     \r, (r9)
.endm

; R = I/O register (S3 = IO_SEG afterwards). Uses R9.
.macro iord r, off
        li      r9, #IO_SEG
        mts     s3, r9
        li      r9, #\off
        lwx     \r, (r9)
.endm

; I/O register = R (S3 = IO_SEG afterwards). Uses R9.
.macro iowr off, r
        li      r9, #IO_SEG
        mts     s3, r9
        li      r9, #\off
        swx     \r, (r9)
.endm

; S1 = value. Uses R9.
.macro seg1 value
        li      r9, #\value
        mts     s1, r9
.endm

; Calls and jumps beyond BL's 4 KiB and a branch's 256 bytes (same
; segment). They use R10.
.macro call l
        li      r10, #\l
        jalr    r11, r10
.endm

.macro jmp l
        li      r10, #\l
        jr      r10
.endm

.macro jne l
        beq     .jne\@
        li      r10, #\l
        jr      r10
.jne\@:
.endm

.macro jeq l
        bne     .jeq\@
        li      r10, #\l
        jr      r10
.jeq\@:
.endm
