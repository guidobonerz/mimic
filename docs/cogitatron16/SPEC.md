# Cogitatron 16 — Specification

The Cogitatron 16 is a fictional steampunk home computer. This document is
the single source of truth for its hardware and the conventions of its
software. The emulator lives in mimic as machine `cogitatron16`
(Phase 31); where this file and code disagree, this file wins and the
code is fixed (or this file is changed first, deliberately).

Status: Phase 1 (specification), 2026-11-05. Decisions taken on the
owner's answers are marked **[D]**; decisions this document takes where the
brief left room are marked **[S]** and listed in section 14.

## Contents

1. Overview
2. Clocks and video timing
3. CPU "Ada"
4. Memory map
5. Chip bus and DMA slots
6. Babbage: DMA control, Copper, Blitter
7. Babbage: Stereoscope (3D unit)
8. Lumière: video and sprites
9. Calliope: sound
10. Telegraph: network, timers, interrupts, input, SD card, debug port
11. ROM "Foundry", boot, software conventions
12. Host tool chain
13. Emulation in mimic
14. Decision log

---

## 1. Overview

| Part | Name | Summary |
| --- | --- | --- |
| CPU | **Ada** | 16-bit RISC, load/store, 13 registers, 12 MHz, 32-bit addresses through segments |
| DMA, Blitter, Copper, 3D | **Babbage** | chip-bus arbiter, Copper, Blitter, 3D unit **Stereoscope** |
| Video | **Lumière** | linear chunky framebuffer, 256-colour palette (RGB 5:6:5), 256 sprites |
| Sound | **Calliope** | 8 DMA sample channels, stereo, 48 kHz |
| I/O and network | **Telegraph** | Ethernet-like network, timers, interrupt controller, keyboard, mouse, joysticks, SD card |
| ROM | **Foundry** | one 256 KiB chip |
| Operating system | **Boiler** | pre-emptive multitasking |
| Desktop | **Panorama** | NeXTSTEP-like; the machine boots straight into it |

- RAM: 32 MiB = 4 MiB chip RAM (DMA-capable) + 28 MiB fast RAM (CPU only).
- Byte-addressed, **little-endian** **[D]**: the low byte of a word is at
  the even address. Words are 16 bits on even addresses.
- A 32-bit pointer in memory is two words, **segment first** (lower
  address), then offset **[S]**. DMA registers come in pairs `xxxSEG`,
  `xxxOFS` with `SEG` at the lower address.
- Two buses: the CPU bus (fast RAM, ROM, I/O; no wait states) and the
  chip bus (chip RAM; 32 bits wide, 32 MHz, 128 MB/s), joined by Babbage.

## 2. Clocks and video timing

**[D]** 60 Hz with SVGA-like timing. **[S]** The horizontal total is 1060
pixels instead of VESA's 1056 so that every clock has a whole number of
cycles per line:

| Clock | Frequency | Per line | Per frame |
| --- | --- | --- | --- |
| Pixel clock | 40 MHz | 1060 | 665 680 |
| Chip bus slot | 32 MHz | 848 | 532 544 |
| CPU cycle (Ada) | 12 MHz | 318 | 199 704 |
| Virtual master | 480 MHz | 12 720 | 7 988 160 |

The three clocks stand 10 : 8 : 3 (pixels : slots : CPU cycles). The
emulator counts master ticks (`Machine::now`): pixel = 12, slot = 15, CPU
cycle = 40 master ticks.

| | Visible | Front porch | Sync | Back porch | Total |
| --- | --- | --- | --- | --- | --- |
| Horizontal (pixels) | 800 | 40 | 128 | 92 | 1060 |
| Vertical (lines) | 600 | 1 | 4 | 23 | 628 |

Line rate 37.736 kHz, frame rate 60.09 Hz, 16.642 ms. Beam position:
`x` 0–1059 (0 = first visible pixel), `y` 0–627 (0 = first visible line).
Horizontal blank is `x` 800–1059, vertical blank lines 600–627. The
**VBlank interrupt** fires at `x` 0 of line 600; display pointers are taken
over at that moment.

## 3. CPU "Ada"

### 3.1 Registers

| Register | Use |
| --- | --- |
| R0 | always 0; writes are discarded (flags are still set) |
| R1–R9 | general |
| R10 | general; return segment of a far call (CALLF) |
| R11 | link register (BL, CALLF write the return offset here) |
| R12 | stack pointer by convention (stack grows down, in segment S2) |
| S0–S3 | segment registers, 16 bits: S0 code, S1 data, S2 stack, S3 extra |
| PC | 16 bits, offset in segment S0 |
| SR | status register (below) |

**SR** **[S]**:

| Bit | Name | Meaning |
| --- | --- | --- |
| 0 | C | carry; after a subtraction 1 = no borrow |
| 1 | V | signed overflow |
| 2 | Z | zero |
| 3 | N | negative (bit 15 of the result) |
| 8–10 | IPL | interrupt mask: levels ≤ IPL are blocked (7 blocks all but NMI) |
| 13 | S | supervisor mode |
| others | – | read 0, writes ignored |

**System control registers** (supervisor; MFSC/MTSC) **[S]**:

| # | Name | Meaning |
| --- | --- | --- |
| 0 | SPC | shadow PC (return offset of an exception) |
| 1 | SS0 | shadow S0 |
| 2 | SSR | shadow SR |
| 3 | CAUSE | vector number of the last exception; for TRAP bits 8–15 = n |
| 4 | CYCLE | free-running CPU cycle counter, low 16 bits (read only) |
| 5 | CYCLEH | high 16 bits, latched when CYCLE is read |

**Reset**: S0 = $FFFB, PC = $0000, SR = S set, IPL = 7, other flags 0;
R1–R12, S1–S3 and the shadow registers are 0.

### 3.2 Addresses

Linear address = segment × 65 536 + offset (16:16, no overlap). Offsets
wrap within their segment; only the post-increment of the X instructions
carries into S3 (below).

Segment for data accesses: base register R12 → S2; any other base → S1;
the X instructions → S3. Instruction fetch: S0:PC.

Word accesses ignore bit 0 of the address **[S]** (no alignment trap).
Reads from unpopulated space return $FFFF ($FF for bytes); writes there
are ignored; no bus error.

### 3.3 Instruction formats (16 bits; bit 15 left)

```
R  [15:12 Op][11:8 Rd][7:4 Rs][3:0 Funct]     Rd = Rd op Rs
I  [15:12 Op][11:8 Rd][7:0 Imm8]
M  [15:12 Op][11:8 Rd][7:4 Rb][3:0 Off4]
B  [15:12 Op][11:8 Cond][7:0 Off8]
J  [15:12 Op][11:0 Off12]
```

Branch targets are relative to the address of the next instruction
(PC + 2), in words: target = PC + 2 + 2 × sign-extended offset.

Register and number fields an instruction does not use must be 0;
otherwise the word is an illegal instruction **[S]** (every word has one
meaning, and the disassembler shows exactly what was assembled).

### 3.4 Opcode map

| Op | Fmt | Instruction(s) | Cycles |
| --- | --- | --- | --- |
| 0 | R | ALU (3.5) | 1 (MUL 2) |
| 1 | I | `ADDI Rd, #imm8` — Rd += sign-extended imm8; flags as ADD | 1 |
| 2 | I | `MOVI Rd, #imm8` — Rd = imm8 (high byte 0); no flags | 1 |
| 3 | I | `LUI Rd, #imm8` — high byte of Rd = imm8, low byte kept; no flags | 1 |
| 4 | I | `CMPI Rd, #imm8` — flags of Rd − imm8, imm8 **zero-extended** **[S]** | 1 |
| 5 | M | `LW Rd, off(Rb)` — off = 2 × Off4 (0–30) | 2 |
| 6 | M | `SW Rd, off(Rb)` | 2 |
| 7 | M | `LB Rd, off(Rb)` — off = Off4 (0–15); zero-extended **[S]** | 2 |
| 8 | M | `SB Rd, off(Rb)` — low byte of Rd | 2 |
| 9 | R | X loads/stores through S3 (3.6) | 2 |
| A | B | `Bcc off8` — ±256 bytes (3.7) | 1, taken 2 |
| B | J | `BL off12` — R11 = PC + 2; ±4 KiB | 2 |
| C | R | jumps and segments (3.8) | 2 (MTS/MFS 1) |
| D | R | MULH, DIV, REM, NEG, extend, swap (3.9) | 2; DIV/REM 18 |
| E | R | system (3.10) | 1–2 |
| F | – | reserved prefix: illegal instruction | – |

Loads and stores to chip RAM additionally wait for a free chip-bus slot
(section 5); I/O register accesses take one extra cycle **[S]**.
Loads/stores never change flags.

### 3.5 Op 0: ALU (`Rd = Rd op Rs`)

| Funct | Mnemonic | Operation | Flags |
| --- | --- | --- | --- |
| 0 | ADD | Rd + Rs | N Z C V |
| 1 | SUB | Rd − Rs | N Z C V |
| 2 | ADC | Rd + Rs + C | N Z C V |
| 3 | SBC | Rd − Rs − (1 − C) | N Z C V |
| 4 | AND | Rd & Rs | N Z |
| 5 | OR | Rd \| Rs | N Z |
| 6 | XOR | Rd ^ Rs | N Z |
| 7 | NOT | ~Rs | N Z |
| 8 | SHL | Rd << (Rs & 15) | N Z C (last bit out; unchanged for 0) |
| 9 | SHR | logical Rd >> (Rs & 15) | N Z C |
| A | ASR | arithmetic Rd >> (Rs & 15) | N Z C |
| B | ROL | rotate Rd left by (Rs & 15) | N Z C (bit 0 of the result for counts > 0) |
| C | MOV | Rs | N Z |
| D | CMP | flags of Rd − Rs, Rd unchanged | N Z C V |
| E | TST | flags of Rd & Rs, Rd unchanged | N Z |
| F | MUL | low 16 bits of Rd × Rs | N Z |

Flags not listed are unchanged. `0x0000` is `ADD R0, R0`: it sets Z and
clears N, C, V; the real no-op is `NOP` (Op E).

### 3.6 Op 9: X loads and stores (segment S3, base Rs, no offset)

| Funct | Mnemonic | Operation |
| --- | --- | --- |
| 0 | `LWX Rd, (Rs)` | load word |
| 1 | `SWX Rd, (Rs)` | store word |
| 2 | `LBX Rd, (Rs)` | load byte, zero-extended |
| 3 | `SBX Rd, (Rs)` | store byte |
| 4–7 | `LWX Rd, (Rs)+` … `SBX Rd, (Rs)+` | as 0–3, then Rs += 2 (word) or 1 (byte) |
| 8–F | – | illegal instruction |

**[S]** When a post-increment wraps Rs from $FFFF to $0000 (or $FFFE →
$0000), S3 is incremented: block copies stream across segments.
Post-increment with Rd = Rs: the loaded value wins.

### 3.7 Op A: conditional branches

| Cond | Mnemonic | Taken when |
| --- | --- | --- |
| 0 | BEQ | Z |
| 1 | BNE | !Z |
| 2 | BCS / BHS | C |
| 3 | BCC / BLO | !C |
| 4 | BMI | N |
| 5 | BPL | !N |
| 6 | BVS | V |
| 7 | BVC | !V |
| 8 | BHI | C & !Z |
| 9 | BLS | !C \| Z |
| A | BGE | N = V |
| B | BLT | N ≠ V |
| C | BGT | !Z & N = V |
| D | BLE | Z \| N ≠ V |
| E | BRA | always |
| F | – | never (2-byte no-op) |

### 3.8 Op C: jumps and segments

| Funct | Mnemonic | Operation |
| --- | --- | --- |
| 0 | `JR Rs` | PC = Rs |
| 1 | `JALR Rd, Rs` | Rd = PC + 2; PC = Rs |
| 2 | `MTS Sn, Rs` | Sn = Rs, n = Rd & 3; n = 0 is illegal (use JF) |
| 3 | `MFS Rd, Sn` | Rd = Sn, n = Rs & 3 |
| 4 | `JF Rd:Rs` | far jump: S0 = Rd, PC = Rs |
| 5 | `CALLF Rd:Rs` | far call: R10 = S0, R11 = PC + 2, S0 = Rd, PC = Rs |
| 6–F | – | illegal instruction |

Assembler aliases: `RET` = `JR R11`, `RETF` = `JF R10:R11`.

### 3.9 Op D: multiply high, divide, misc (`Rd = f(Rd, Rs)`)

| Funct | Mnemonic | Operation | Flags |
| --- | --- | --- | --- |
| 0 | MULHU | high 16 bits of unsigned Rd × Rs | N Z |
| 1 | MULHS | high 16 bits of signed Rd × Rs | N Z |
| 2 | DIVU | Rd / Rs unsigned | N Z |
| 3 | DIVS | Rd / Rs signed, towards zero; $8000 / −1 = $8000 with V | N Z V |
| 4 | REMU | Rd mod Rs unsigned | N Z |
| 5 | REMS | signed remainder, sign of the dividend | N Z |
| 6 | NEG | 0 − Rs | as SUB |
| 7 | SXB | Rs's low byte sign-extended | N Z |
| 8 | ZXB | Rs's low byte zero-extended | N Z |
| 9 | SWAB | Rs with its bytes swapped | N Z |
| A–F | – | illegal instruction | |

Division (2–5) by zero raises the divide-by-zero exception; Rd is
unchanged. DIV/REM take 18 cycles **[S]**, the others 2.

### 3.10 Op E: system

| Funct | Mnemonic | Operation | Priv. |
| --- | --- | --- | --- |
| 0 | `TRAP #n` | n = Rd:Rs (8 bits); exception 4 with CAUSE = n << 8 \| 4 | |
| 1 | `RTI` | PC = SPC, S0 = SS0, SR = SSR (2 cycles) | yes |
| 2 | `EI` | IPL = 0 | yes |
| 3 | `DI` | IPL = 7 | yes |
| 4 | `HALT` | stop until an interrupt is taken (with IPL), then continue after HALT | yes |
| 5 | `MTSR Rs` | SR = Rs | yes |
| 6 | `MFSR Rd` | Rd = SR | |
| 7 | `MFSC Rd, #c` | Rd = system register c (c = Rs field) | yes |
| 8 | `MTSC #c, Rs` | system register c = Rs (c = Rd field) | yes |
| 9 | `NOP` | nothing | |
| A | `BRK` | debugger breakpoint; without a debugger exception 5 | |
| B–F | – | illegal instruction | |

Privileged instructions in user mode (S = 0) raise exception 2.

### 3.11 Exceptions and interrupts

Taking exception or interrupt *v*: SPC = address of the next instruction
(for exceptions caused by an instruction: that instruction's address for
illegal/privilege/divide, the next one for TRAP and BRK), SS0 = S0,
SSR = SR; S = 1; for interrupts IPL = level; CAUSE = v (TRAP: n << 8 | 4);
S0:PC = vector v. Entry takes 2 cycles. There is one set of shadow
registers: a handler that lowers IPL must first save SPC, SS0, SSR (MFSC).

**Vector table** **[S]**: 32 entries of 4 bytes (segment, offset) at
`FFFB:0040` in the ROM.

| v | Cause |
| --- | --- |
| 0 | reserved (reset always starts at FFFB:0000) |
| 1 | illegal instruction |
| 2 | privilege violation |
| 3 | divide by zero |
| 4 | TRAP #n |
| 5 | BRK without a debugger |
| 6–8 | reserved |
| 9–15 | interrupt level 1–7 |
| 16–31 | reserved |

Interrupts: 7 levels; level *L* is taken between instructions when
L > IPL. Level 7 is the NMI: taken on its rising edge even with IPL = 7.
Source-to-level assignment: section 10.4.

### 3.12 Timing

Execution is instruction-exact from Phase 2 and line-exact from Phase 4:
each instruction takes the cycles in 3.4 plus wait states for chip-bus
accesses. The three-stage pipeline (fetch, decode, execute) is not
visible to software: no delay slots, no hazards; a taken branch costs one
extra cycle (the refill).

## 4. Memory map

| Segments | Size | Content |
| --- | --- | --- |
| 0000–003F | 4 MiB | chip RAM |
| 0040–01FF | 28 MiB | fast RAM |
| 0200–7FFF | – | fast-RAM expansion, not fitted |
| 8000–FEFF | ~2 GiB | expansion bus, nothing fitted |
| FF00–FFFA | – | reserved |
| FFFB–FFFE | 256 KiB | ROM "Foundry"; reset starts at FFFB:0000 |
| FFFF | 64 KiB | I/O registers |

### 4.1 I/O segment FFFF

| Offset | Unit |
| --- | --- |
| 0000–07FF | Babbage: DMA control, Copper, Blitter, Stereoscope |
| 0800–0FFF | Lumière: video mode, framebuffer, sprites control |
| 1000–11FF | palette: 256 × RGB 5:6:5 |
| 2000–27FF | sprite table: 256 × 8 bytes (inside the chip) |
| 3000–31FF | Calliope |
| 4000–41FF | Telegraph: network |
| 4200–43FF | Telegraph: timers, interrupts, input, SD card, debug port |

Registers are 16 bits wide on even offsets; byte writes to a register
write its low byte with the high byte 0 **[S]**. Unused offsets read 0.
All DMA pointers are 32 bits (`SEG`/`OFS` pair). A DMA pointer outside
chip RAM (segment > $003F) sets the unit's bit in `DMAERR` and raises the
DMA-error interrupt; the unit stops.

"Set/clear" registers (marked S/C) work like the Amiga's: writing with bit
15 set sets the bits given in 0–14, with bit 15 clear clears them.

## 5. Chip bus and DMA slots

One slot = one 32-bit chip-RAM access (4 bytes, on a 4-byte boundary;
16-bit and byte accesses also take a whole slot). 848 slots a line.

**Fixed DMA** for line y + 1 starts at slot 640 of line y (horizontal
blank, `x` = 800) and takes consecutive slots in this order until all are
served, running on into line y + 1 if needed:

| # | Unit | Slots per line (max) |
| --- | --- | --- |
| 1 | Lumière framebuffer | mode 0: 80 on even lines; 1: 160 on even lines; 2: 100; 3: 200; 4: 50 on a row's first line + 25 (font) |
| 2 | Lumière sprites | 8 per 32-pixel 8-bit row, …; up to 256 (32 sprites) |
| 3 | Calliope | 1 per active channel: up to 8 |
| 4 | Telegraph | up to 8 (network descriptors/data, SD card) |

In mode 3 the framebuffer's 200 slots fit the 208 slots of the horizontal
blank exactly.

**Free slots** (all others) go, slot by slot, to the highest-priority unit
with an access pending: 5. Copper, 6. Blitter and Stereoscope (alternating
when both wait), 7. CPU. With `BUSCON` bit 0 set the CPU comes before the
Blitter and Stereoscope.

A CPU cycle spans 8/3 slots (cycle c: slots ⌊8c/3⌋ to ⌊8(c+1)/3⌋ − 1).
A CPU chip-RAM access waits until it gets a slot; the wait is counted in
CPU cycles: an instruction that is fetched from chip RAM or loads or
stores there starts only in a cycle with a free slot left after the
fixed DMA and the Copper **[S]** (one free slot covers all its accesses,
an emulator simplification).

All DMA is off after reset (DMACON = 0): the ROM turns the framebuffer
DMA on.

## 6. Babbage: DMA control, Copper, Blitter

### 6.1 Control registers (FFFF:0000)

| Offset | Name | R/W | Meaning |
| --- | --- | --- | --- |
| 0000 | BABID | R | chip id $BA01 |
| 0002 | DMACON | S/C | bit 0 framebuffer, 1 sprites, 2 Calliope, 3 Telegraph, 4 Copper, 5 Blitter, 6 Stereoscope, 14 master enable |
| 0004 | DMAERR | R/W1C | bits as DMACON: unit stopped by a pointer outside chip RAM |
| 0006 | BUSCON | R/W | bit 0: CPU before Blitter/Stereoscope |

### 6.2 Copper (FFFF:0020)

| Offset | Name | Meaning |
| --- | --- | --- |
| 0020/0022 | COP1SEG/COP1OFS | list 1 start |
| 0024/0026 | COP2SEG/COP2OFS | list 2 start |
| 0028 | COPJMP1 | write: restart at list 1 now |
| 002A | COPJMP2 | write: restart at list 2 now |
| 002C | COPCON | bit 0: Copper may write Babbage registers (0000–07FF) |
| 002E | COPIRQ | value of the last `IRQ` instruction (read) |

The Copper restarts at list 1 when a field begins (line 0, x 0) **[S]**
(at the VBlank every WAIT for a visible line would hold at once; a list
sets up line 0 with a WAIT in the blank of line 599). It executes one
instruction (two words, one slot) per free slot it gets; a WAIT compares
in every free slot without taking it, and once it holds the next
instruction runs in that same slot. Instruction: word 0 bits 15–13 are
the type:

| Type | Name | Word 0 | Word 1 |
| --- | --- | --- | --- |
| 000 | MOVE | bits 12–0: register word index (FFFF:0000 + 2 × index, up to FFFF:3FFE) | value |
| 001 | WAIT | bits 9–0: y | bits 10–0: x; bit 15: also wait for the Blitter to finish |
| 010 | SKIP | bits 9–0: y | bits 10–0: x; skip the next instruction if the beam is at or past (y, x) |
| 011 | JUMP | bit 0: list (0 = list 1, 1 = list 2) | – |
| 100 | BLIT | – | – : start the Blitter job prepared in its registers |
| 101 | IRQ | – | value for COPIRQ; raises the Copper interrupt |
| 110 | STOP | – | – : wait for the next field |
| 111 | – | reserved (acts as STOP) | |

WAIT holds until the beam is at or past (y, x) (y first, then x) and,
with bit 15, the Blitter is idle. A MOVE into Babbage without COPCON
bit 0 stops the Copper and sets DMAERR bit 4 (DMA-error interrupt).

### 6.3 Blitter (FFFF:0100)

| Offset | Name | Meaning |
| --- | --- | --- |
| 0100 | BLTCON0 | bits 3–0 ROP; 4 use A; 5 use B; 6 use D; 8–9 op: 0 copy/ROP, 1 fill, 2 line, 3 shade |
| 0102 | BLTCON1 | bits 1–0 pixel size (0 = 4, 1 = 8, 2 = 16 bits); 2 flip X; 3 flip Y; 4 colour key on A; 5 nibble phase of A, 6 of B, 7 of D (4-bit: start at the high nibble = 0) |
| 0104/0106 | BLTASEG/OFS | source A |
| 0108/010A | BLTBSEG/OFS | source B |
| 010C/010E | BLTDSEG/OFS | destination D |
| 0110 | BLTAPITCH | bytes from one row to the next (signed) |
| 0112 | BLTBPITCH | |
| 0114 | BLTDPITCH | |
| 0116 | BLTW | width in pixels (1–4096) / line: dx |
| 0118 | BLTH | height in rows (1–4096) / line: dy |
| 011A | BLTKEY | colour key (A pixels equal to it are skipped) |
| 011C | BLTCOLOR | fill and line colour |
| 011E/0120 | BLTTSEG/OFS | 256-byte shade table |
| 0122 | BLTLINE | line: bit 0 x steps −1, bit 1 y steps −1, bit 2 y is the major axis |
| 0124 | BLTSTART | write: start |
| 0126 | BLTSTAT | bit 0 busy, bit 1 done (cleared on read) |

ROP: for each bit, D = ROP[ (A << 1) | B ] (bit index into the 4-bit
value); $C = A (copy), $A = B, $8 = A AND B, $E = A OR B, $6 = A XOR B.
Op fill: D = BLTCOLOR over the rectangle. Op line: Bresenham from D, BLTW
× BLTH as dx × dy, BLTLINE directions, colour BLTCOLOR. Op shade:
D = TABLE[B] where A (if used) is not BLTKEY, else D = B — darkening and
lightening for Panorama's shadows. With the colour key, A pixels equal to
BLTKEY leave D unchanged. Flips mirror the source walk. Each 32-bit
access is one slot. Completion raises the Blitter interrupt.

**[S]** Details: a channel that is not used reads 0; a job works a row
at a time, a row costing, for each channel it uses, the 32-bit words its
bytes touch (a line pixel costs two slots); BLTSTART, or the Copper's
BLIT, with a used pointer outside chip RAM starts nothing, sets DMAERR
bit 5 and raises the DMA-error interrupt (the Copper stops too); the
registers must not change while BLTSTAT bit 0 says busy.

## 7. Babbage: Stereoscope (FFFF:0200)

| Offset | Name | Meaning |
| --- | --- | --- |
| 0200/0202 | S3DLSEG/OFS | display list |
| 0204/0206 | S3DTSEG/OFS | target framebuffer |
| 0208 | S3DPITCH | target pitch |
| 020A | S3DMODE | target format: 0 = mode 0 (8 bit), 1 = mode 1 (16 bit), 3 = mode 3 (8 bit) |
| 020C/020E | S3DZSEG/OFS | 16-bit Z buffer |
| 0210 | S3DSTART | write: run the display list |
| 0212 | S3DSTAT | bit 0 busy, bit 1 done, bit 2 error (bad command) |

Display list: 32-bit words (low word first). Command word: bits 31–24
opcode, 23–0 argument; parameters follow.

| Op | Name | Parameters |
| --- | --- | --- |
| 00 | END | raises the Stereoscope interrupt |
| 01 | MATRIX | 16 × 16.16 fixed point (row-major 4 × 4) |
| 02 | VERTICES | pointer, count (≤ 256): vertex list (below) |
| 03 | TRIANGLES | pointer to index triples (16 bit), count |
| 04 | LINES | pointer to index pairs, count |
| 05 | POINTS | pointer to indices, count |
| 06 | TEXTURE | pointer, width and height (log2, ≤ 8), format (4/8/16 bit) |
| 07 | STATE | bit 0 Gouraud, 1 texture, 2 Z test, 3 Z write, 4 backface culling, 5 lighting, 6 dither (8 bit), 7 blend, 8 fog |
| 08 | LIGHT | direction (3 × 16.16), ambient and diffuse (0–255) |
| 09 | TABLES | blend table pointer, fog table pointer (256 × 256 bytes each, chip RAM) |
| 0A | CLEAR | colour, Z value, rectangle |
| 0B | VIEWPORT | x, y, w, h, near clip (16.16), perspective scale |
| 0C | JUMP | pointer (continue the list there) |

Vertex: x, y, z (3 × 16.16), normal (3 × s16, 1.15), u, v (2 × s16,
texels × 16), colour (16 bit) = 24 bytes. Rasteriser: triangles, lines,
points; flat or Gouraud; perspective-correct textures every 16 pixels;
16-bit Z buffer; blend and fog through the tables; ordered dithering in
8-bit modes. Throughput figures: about 300 000 vertices/s and 5 million
pixels/s, modelled as slot costs in Phase 7.

## 8. Lumière: video and sprites (FFFF:0800)

### 8.1 Modes

| Mode | Picture | Pixel | Bytes/frame | Shown as |
| --- | --- | --- | --- | --- |
| 0 | 320 × 240 | 8 bit indexed | 76 800 | pixels doubled to 640 × 480, centred (border 80 left/right, 60 top/bottom) |
| 1 | 320 × 240 | 16 bit RGB 5:6:5 | 153 600 | likewise |
| 2 | 800 × 600 | 4 bit indexed (high nibble = left pixel) | 240 000 | full screen |
| 3 | 800 × 600 | 8 bit indexed | 480 000 | full screen |
| 4 | 100 × 37 characters | text, 8 × 16 font | 7 400 | 800 × 592, border 4 lines top and bottom |

After reset every Lumière register and the palette are 0 (display off,
border palette entry 0) **[S]**. Blinking text and the cursor alternate
every 32 fields (about 0.53 s) **[S]**.

Text mode: each cell is two bytes, character then attribute (bits 0–3
foreground, 4–6 background, 7 blink), colours 0–15 of the palette's text
bank (`TXTBANK`). Font: 256 × 16 bytes in chip RAM (`FONTSEG/OFS`).

### 8.2 Registers

| Offset | Name | Meaning |
| --- | --- | --- |
| 0800 | VMODE | bits 2–0 mode; bit 15 display on (off: border colour only) |
| 0802/0804 | FBSEG/FBOFS | framebuffer start; taken over at the VBlank |
| 0806 | PITCH | bytes per framebuffer row (the framebuffer may be wider than the screen) |
| 0808 | FINEX | 4-bit mode: start at the low nibble (bit 0) |
| 080A | BORDER | border colour (palette index; mode 1: RGB 5:6:5) |
| 080C | FGINDEX | palette indices ≥ FGINDEX are in front of sprites (0 = off) |
| 080E | TXTBANK | text mode: palette bank (bits 7–4) |
| 0810/0812 | FONTSEG/FONTOFS | text font |
| 0814 | CURSOR | text cursor cell (0–3699; $FFFF = off) |
| 0816 | CURSHAPE | cursor lines: bits 3–0 first, 7–4 last |
| 0818 | BEAMY | current line (read) |
| 081A | BEAMX | current pixel (read) |
| 081C | VSTAT | bit 0 in VBlank, bit 1 in HBlank, bit 2 frame pointer taken over at the VBlank (cleared on read) |
| 081E | FBNOW | write: take FBSEG/FBOFS over at the end of this line (split screens); the rows count anew from there **[S]** |
| 0820/0822 | SPRSEG/SPROFS | sprite image area base |
| 0824 | SPRCOLFG | collision with the foreground: bit g = group g (cleared on read) |
| 0840–085E | SPRCOL0–15 | sprite–sprite collisions: in register g, bit h = group g touched group h (cleared on read) |
| 1000–11FE | PALETTE | 256 × RGB 5:6:5 |

### 8.3 Sprites **[D]** fixed image area

Sprite table FFFF:2000 + 8 × n, four words:

| Word | Bits |
| --- | --- |
| 0 | 10–0 X (screen x + 64); 12–11 width (0 = 8, 1 = 16, 2 = 32, 3 reserved); 13 flip X; 14 flip Y; 15 active |
| 1 | 9–0 Y (screen y + 64); 11–10 height (8/16/32/64); 12 behind the background |
| 2 | image offset in the sprite area in 16-byte steps (area: up to 1 MiB) |
| 3 | 0: 8 bit (else 4 bit); 7–4 palette bank (4-bit sprites: colour = bank × 16 + pixel); 11–8 collision group (0 = none) |

Coordinates are in the pixels of the current mode (modes 0/1: 320 × 240).
Image data: chunky rows, top to bottom, `width` bytes (8 bit) or
`width / 2` (4 bit) per row. Pixel value 0 is transparent. At most 32
sprites per line: in the horizontal blank the lowest-numbered active
sprites covering the next line are chosen into a line buffer; a lower
number lies on top. "Behind the background": the sprite shows only where
the framebuffer pixel is 0 (modes 0, 2, 3) or black (mode 1). Palette
indices ≥ FGINDEX lie in front of all sprites.

## 9. Calliope: sound (FFFF:3000)

Channel n (0–7) at FFFF:3000 + $20 × n:

| Offset | Name | Meaning |
| --- | --- | --- |
| +00/+02 | AUDSEG/AUDOFS | sample start |
| +04/+06 | AUDLENH/AUDLENL | length in samples (32 bits) |
| +08 | AUDPER | period: rate = 4 000 000 / AUDPER samples/s (AUDPER ≥ 16) |
| +0A | AUDVOL | volume 0–127 |
| +0C | AUDPAN | pan 0 (left) … 16 (centre) … 31 (right) |
| +0E/+10 | AUDLOOPH/AUDLOOPL | loop start (sample index) |
| +12/+14 | AUDLLENH/AUDLLENL | loop length (0 = play once, then stop) |
| +16 | AUDCTL | bit 0 play; 1 16-bit samples; 3–2 modulate channel n + 1 (0 off, 1 volume, 2 period); 4 envelope; 5 noise instead of samples; 6 interrupt at the end / each loop |
| +18 | AUDENV1 | attack (bits 7–0), decay (15–8): rate steps |
| +1A | AUDENV2 | sustain level (7–0), release (15–8) |
| +1C | AUDGATE | bit 0 gate (envelope) |
| +1E | AUDSTAT | bit 0 playing, bit 1 interrupt pending (cleared on read) |

| Offset | Name | Meaning |
| --- | --- | --- |
| 3100 | AUDMASTER | master volume 0–127 |
| 3102 | AUDIRQ | bit n: channel n interrupt (cleared on write 1) |

Samples are signed. Each active channel fetches one 32-bit slot per line
(4 bytes into an 8-byte FIFO), so up to about 150 000 8-bit or 75 000
16-bit samples/s per channel. Output: stereo, 16 bit, 48 kHz (mixer:
sample × volume × pan law (linear) × master, summed and clipped).
Modulation: channel n's output replaces channel n + 1's volume (bits 15–9
of the sample) or period (the sample as period); a modulating channel is
not heard.

## 10. Telegraph (FFFF:4000)

### 10.1 Network (FFFF:4000)

| Offset | Name | Meaning |
| --- | --- | --- |
| 4000 | NETCON | bit 0 receive on, 1 transmit on, 2 promiscuous |
| 4002–4006 | NETMAC0–2 | 48-bit MAC (high word first: NETMAC0 = bytes 0–1) |
| 4008/400A | TXRSEG/TXROFS | transmit ring: 16 descriptors |
| 400C/400E | RXRSEG/RXROFS | receive ring: 16 descriptors |
| 4010 | TXTAIL | index (0–15) the CPU has filled up to; writing starts sending |
| 4012 | TXHEAD | index hardware has sent up to (read) |
| 4014 | RXHEAD | index hardware has filled up to (read) |
| 4016 | RXTAIL | index the CPU has emptied up to |
| 4018 | NETSTAT | bit 0 link up |
| 401A | NETIRQ | bit 0 received, 1 sent, 2 receive ring full, 3 link changed (write 1 clears) |
| 401C | NETIRQEN | enables for NETIRQ |

Descriptor (8 bytes): buffer segment, buffer offset, length (bytes,
≤ 1536), flags: bit 15 OWN (1 = hardware's), bit 0 error, bit 1 CRC
error. Frames are Ethernet-like (destination MAC, source MAC, type,
payload); the hardware appends and checks the CRC-32. Emulator: frames go
through a UDP tunnel between emulator instances (later a TAP device).

### 10.2 Timers and real-time clock (FFFF:4200)

| Offset | Name | Meaning |
| --- | --- | --- |
| 4200 + 8n | TnCNT | counter n (0–3), counts down at 1 MHz |
| 4202 + 8n | TnLATCH | reload value |
| 4204 + 8n | TnCTL | bit 0 run, 1 reload (continuous), 2 count timer n − 1's underflows (cascade), 3 interrupt |
| 4220 | TSTAT | bit n: timer n underflowed (cleared on read) |
| 4224/4226 | RTCHI/RTCLO | seconds since 2000-01-01 00:00 UTC; writing RTCLO sets the clock |
| 4228 | RTCALARM | alarm in seconds (low 16 bits), interrupt when RTCLO equals it |

### 10.3 Input and SD card

| Offset | Name | Meaning |
| --- | --- | --- |
| 4240 | KBDDATA | next key event (FIFO, 16 entries): bits 7–0 USB HID usage, bit 15 released; $0000 = empty |
| 4242 | KBDSTAT | bit 0 event waiting, bit 1 overflow |
| 4248 | PORTCFG | port 0 bits 1–0, port 1 bits 3–2: 0 none, 1 mouse, 2 joystick |
| 424A/424C | MOUSE0X/Y | mouse counters (16 bit, wrap) |
| 424E/4250 | MOUSE1X/Y | |
| 4252 | PORT0 | joystick: bit 0 up, 1 down, 2 left, 3 right, 4–6 buttons 1–3; mouse: bits 4–6 buttons |
| 4254 | PORT1 | |
| 4260 | SDSTAT | bit 0 card present, 1 busy, 2 error, 3 done (cleared on read), 4 write protected |
| 4262 | SDCMD | write: 1 read blocks, 2 write blocks |
| 4264/4266 | SDBLKH/SDBLKL | first block (512 bytes each) |
| 4268 | SDCOUNT | blocks |
| 426A/426C | SDSEG/SDOFS | chip-RAM buffer |
| 426E/4270 | SDSIZEH/SDSIZEL | card size in blocks (read) |

The SD card is a block device with DMA (no SPI); a block takes 20 µs
(through Telegraph's DMA slots). In the emulator the card is an image
file in a media slot.

### 10.4 Interrupt controller

| Offset | Name | Meaning |
| --- | --- | --- |
| 4280 | INTREQ | S/C: pending sources |
| 4282 | INTENA | S/C: enabled sources |

| Bit | Source | Level |
| --- | --- | --- |
| 0 | VBlank | 4 |
| 1 | Copper (IRQ instruction) | 4 |
| 2 | DMA error | 4 |
| 3 | Blitter done | 3 |
| 4 | Stereoscope done | 3 |
| 5 | Calliope (any channel, see AUDIRQ) | 6 |
| 6 | Timers (see TSTAT) | 5 |
| 7 | Network (see NETIRQ) | 2 |
| 8 | Keyboard | 1 |
| 9 | Mouse/joystick ports | 1 |
| 10 | SD card | 1 |
| 11 | RTC alarm | 1 |
| 14 | NMI (host key or debugger) | 7 |

A source's level is pending while its INTREQ and INTENA bits are both
set; the CPU sees the highest pending level.

### 10.5 Debug port (FFFF:43F0)

| Offset | Name | Meaning |
| --- | --- | --- |
| 43F0 | DBGOUT | write: low byte to the host's debug console |
| 43F2 | DBGEXIT | write: end an emulator test run with this exit code |
| 43F4 | DBGID | read: $C016 when running in the emulator |

## 11. ROM "Foundry", boot and software conventions

ROM layout at FFFB:0000:

| Offset | Content |
| --- | --- |
| 0000 | reset entry: `BRA init` |
| 0040–00BF | vector table (32 × segment, offset) |
| 00C0 | `init`, then code and data over FFFB–FFFE |

Boot: self-test and chip set-up, start picture with the brass logo,
boot loader loads Boiler and Panorama from the SD card; holding a key at
power-on enters the emergency monitor. No text screen in the normal boot.

Calling convention **[S]**: arguments in R1–R4, results in R1 (R2 for
32-bit), R1–R4 and R10/R11 caller-saved, R5–R9 callee-saved, R12 stack
pointer in S2 (stack grows down, word-aligned), far pointers as
(segment, offset) pairs. ROM services through `TRAP #n` (numbers in the
ROM's own documentation, Phase 9).

## 12. Host tool chain

All in Rust in mimic's workspace **[D]**:

- `asm-ada`: assembler with macros and a linker (objects, sections,
  far labels: `seg(label)`, `off(label)`), output raw images, ROM images
  and listings. Syntax: one instruction per line, registers `r0`–`r12`
  (`sp` = r12, `lr` = r11), `#imm`, `off(rb)`, labels with `:`,
  directives `.org .seg .word .byte .ascii .align .include .macro`.
- Emulator: mimic's frontends and debugger (breakpoints, stepping, memory;
  Copper list, sprites and framebuffer through mimic's graphics views).
- Later phases: PNG → chunky converter with a 5:6:5 palette, SD image
  tool, C back end (`char` 8, `int` 16, `long` 32, pointers 32 bits).

## 13. Emulation in mimic

- Machine id `cogitatron16`; crates `cpu-ada` (CPU), `chips-cogitatron`
  (Babbage, Lumière, Calliope, Telegraph), `machine-cogitatron16`,
  `asm-ada` (assembler, also used by tests and to build the ROM).
- Frame: 800 × 600 RGB (the full visible area; modes 0, 1 and 4 with
  their borders), signal metadata 40 MHz pixel clock, 1060 × 628, VGA RGB
  output.
- **Shipped as a standard profile** (owner's request): the Foundry ROM is
  assembled from `rom/` sources at build time and embedded, so the
  machine needs no user files; mimic carries a built-in profile
  `cogitatron16` that the desktop and web profile pickers show even
  without a `profiles.yaml` (a user profile of the same name overrides
  it). SD card images go into a media slot.
- A program slot (`--prg`) loads a raw binary at 0040:0000 and starts it
  there in supervisor mode (S1 = S0, stack S2:R12 = 003F:FFFE), as the
  host tool `ada-asm run` does; the debug console's output stays in the
  machine. The machine option `roms` is accepted and ignored (the ROM is
  built in).
- Savestates, debugger targets (`cpu`; Copper and Blitter as devices),
  memory spaces (cpu, chip, fast, rom), chips() register dumps from day
  one, as for every mimic machine.

## 14. Decision log

Owner's answers (2026-11-05): emulator inside mimic (no SDL2; mimic's
frontends); assembler in Rust; 60 Hz SVGA timing; fixed sprite image area;
little-endian; Phase 31 of mimic, begun at once; shipped as a standard
profile.

Decisions of this document:

1. Horizontal total 1060 (not 1056) for whole clock counts per line; 60.09 Hz.
2. Virtual master clock 480 MHz (pixel 12, slot 15, CPU 40 ticks).
3. Pointers in memory: segment first.
4. SR layout (C V Z N in bits 0–3, IPL 8–10, S 13); system registers SPC, SS0, SSR, CAUSE, CYCLE.
5. Word accesses ignore address bit 0; unpopulated reads $FFFF, no bus error.
6. CMPI zero-extends; ADDI sign-extends; LB/LBX zero-extend.
7. C after subtraction = no borrow (ARM style); conditions as in 3.7.
8. Op 9 functs 0–7 (plain and post-increment); post-increment carries into S3.
9. Op C: JR, JALR, MTS (not S0), MFS, JF, CALLF (return segment in R10).
10. Op D: MULHU, MULHS, DIVU, DIVS, REMU, REMS, NEG, SXB, ZXB, SWAB; DIV/REM 18 cycles, divide-by-zero exception.
11. Op E: TRAP, RTI, EI, DI, HALT, MTSR, MFSR, MFSC, MTSC, NOP, BRK.
12. Vector table at FFFB:0040, 32 entries; reset entry branches over it.
13. Fixed DMA starts at the horizontal blank, in the brief's priority order; free slots by priority; BUSCON for CPU before Blitter.
14. Calliope gets one slot per channel and line (32 bytes/line, not 16) and a 4 MHz period clock.
15. Copper: 8 instruction types incl. STOP; MOVE reaches FFFF:0000–3FFE.
16. Interrupt levels: Calliope 6, timers 5, VBlank/Copper/DMA error 4, Blitter/3D 3, network 2, input/SD/RTC 1, NMI 7.
17. Keyboard reports USB HID usages; SD card is a DMA block device.
18. Sprite width stays 8/16/32 (value 3 reserved); coordinates offset by 64; in modes 0/1 in 320 × 240 pixels.
19. Modes 0/1 are shown pixel-doubled, centred in 800 × 600.
20. Debug port at FFFF:43F0 (console output, test exit code).
21. Byte writes to registers write the low byte, high byte 0.
22. Calling convention R1–R4 arguments, R5–R9 callee-saved.
23. Unused instruction fields must be 0 (else illegal), step 2.
24. Lumière's registers and palette are 0 after reset; blink and cursor phase 32 fields (step 3).
25. The Copper restarts at the field start, not at the VBlank; a WAIT compares without taking a slot (step 4).
26. FBNOW (FFFF:081E) takes the framebuffer pointer over at the line's end, for split screens; VSTAT bit 2 only at the VBlank (step 4).
27. A CPU instruction touching chip RAM starts only in a cycle with a free slot (step 4).
28. Blitter: unused channels read 0; row-wise work with slot costs per touched word, a line pixel 2 slots; a bad pointer refuses the start with DMAERR bit 5 (step 5).
