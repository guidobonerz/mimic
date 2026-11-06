# Logophor 16 — Specification

The Logophor 16 is a fictional steampunk home computer. This document is
the single source of truth for its hardware and the conventions of its
software. The emulator lives in mimic as machine `logophor16`
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
6. Babbage: DMA control, Jacquard, Lithograph
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
| CPU | **Ada** | 16-bit RISC, load/store, 13 registers, 36 MHz, 32-bit addresses through segments |
| DMA, Lithograph, Jacquard, 3D | **Babbage** | chip-bus arbiter, Jacquard, Lithograph, 3D unit **Stereoscope** |
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
| Chip bus slot | 96 MHz | 2544 | 1 597 632 |
| CPU cycle (Ada) | 36 MHz | 954 | 599 112 |
| Virtual master | 1 440 MHz | 38 160 | 23 964 480 |

The three clocks stand 10 : 24 : 9 (pixels : slots : CPU cycles). The
emulator counts master ticks (`Machine::now`): pixel = 36, slot = 15, CPU
cycle = 40 master ticks. **[S]** The CPU and the chip bus ran at 12 and
32 MHz up to 2026-11-06 and were tripled on the owner's request (as fast
as the emulator still keeps real time; `chips_logophor::CLOCK`); the
picture, Calliope's 48 kHz and the timers' 1 MHz kept their rates.

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
| 6 | VBR | vector base: the segment whose offset $0040 holds the vector table (reset $FFFB, the ROM's; step 11) |

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
SSR = SR; S = 1; IPL = 7 (changed in step 9, see decision 44: before,
an interrupt set IPL to its level and an exception kept it, so a second
interrupt could overwrite the one set of shadow registers before the
handler had saved them); CAUSE = v (TRAP: n << 8 | 4); S0:PC = vector v.
Entry takes 2 cycles. There is one set of shadow registers: a handler that
lowers IPL (to let higher levels in) must first save SPC, SS0, SSR (MFSC);
RTI restores the IPL from SSR. An NMI can still interrupt a handler that
has not saved them.

**Vector table** **[S]**: 32 entries of 4 bytes (segment, offset) at
`VBR:0040`; VBR is $FFFB after reset (the ROM's table at `FFFB:0040`),
so a system can move the table to RAM (step 11, owner's choice).

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
| 0000–07FF | Babbage: DMA control, Jacquard, Lithograph, Stereoscope |
| 0800–0FFF | Lumière: video mode, framebuffer, sprites control |
| 1000–11FF | palette: 256 × RGB 5:6:5 |
| 2000–27FF | sprite table: 256 × 8 bytes (inside the chip) |
| 3000–31FF | Calliope |
| 3200–32FF | Phonograph: compressed audio decoder |
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
16-bit and byte accesses also take a whole slot). 2 544 slots a line.

**Fixed DMA** for line y + 1 starts at slot 1 920 of line y (horizontal
blank, `x` = 800) and takes consecutive slots in this order until all are
served, running on into line y + 1 if needed:

| # | Unit | Slots per line (max) |
| --- | --- | --- |
| 1 | Lumière framebuffer | mode 0: 80 on even lines; 1: 160 on even lines; 2: 100; 3: 200; 4: 50 on a row's first line + 25 (font) |
| 2 | Lumière sprites | 8 per 32-pixel 8-bit row, …; up to 256 (32 sprites) |
| 3 | Calliope | 1 per active channel: up to 8 |
| 4 | Telegraph | up to 8 (network descriptors/data, SD card) |

The horizontal blank holds 624 slots, so the whole fixed DMA (at most
472 slots) fits into it.

**XGA fields** **[S]** (mode 5, owner's request 2026-11-06): a field that
begins with mode 5 on runs with XGA timing: 1 992 slots (747 CPU cycles)
a line, three slots two pixels (1 328 pixel clocks, about 64 MHz), 806
lines of which 768 show, about 59.8 fields a second; the CPU clock stays.
The fixed DMA starts at slot 1 536 (x = 1 024); the framebuffer's 256
slots fit into the horizontal blank. The mode is looked at only when a field begins: a
switch shows from the next field (the field it is made in keeps its
timing).

**Free slots** (all others) go, slot by slot, to the highest-priority unit
with an access pending: 5. Jacquard, 6. Lithograph and Stereoscope (alternating
when both wait), 7. CPU. With `BUSCON` bit 0 set the CPU comes before the
Lithograph and Stereoscope.

A CPU cycle spans 8/3 slots (cycle c: slots ⌊8c/3⌋ to ⌊8(c+1)/3⌋ − 1).
A CPU chip-RAM access waits until it gets a slot; the wait is counted in
CPU cycles: an instruction that is fetched from chip RAM or loads or
stores there starts only in a cycle with a free slot left after the
fixed DMA and the Jacquard **[S]** (one free slot covers all its accesses,
an emulator simplification).

All DMA is off after reset (DMACON = 0): the ROM turns the framebuffer
DMA on.

## 6. Babbage: DMA control, Jacquard, Lithograph

### 6.1 Control registers (FFFF:0000)

| Offset | Name | R/W | Meaning |
| --- | --- | --- | --- |
| 0000 | BABID | R | chip id $BA01 |
| 0002 | DMACON | S/C | bit 0 framebuffer, 1 sprites, 2 Calliope, 3 Telegraph, 4 Jacquard, 5 Lithograph, 6 Stereoscope, 7 Phonograph, 14 master enable |
| 0004 | DMAERR | R/W1C | bits as DMACON: unit stopped by a pointer outside chip RAM |
| 0006 | BUSCON | R/W | bit 0: CPU before Lithograph/Stereoscope |

### 6.2 Jacquard (FFFF:0020)

| Offset | Name | Meaning |
| --- | --- | --- |
| 0020/0022 | JAQ1SEG/JAQ1OFS | list 1 start |
| 0024/0026 | JAQ2SEG/JAQ2OFS | list 2 start |
| 0028 | JAQJMP1 | write: restart at list 1 now |
| 002A | JAQJMP2 | write: restart at list 2 now |
| 002C | JAQCON | bit 0: Jacquard may write Babbage registers (0000–07FF) |
| 002E | JAQIRQ | value of the last `IRQ` instruction (read) |

The Jacquard restarts at list 1 when a field begins (line 0, x 0) **[S]**
(at the VBlank every WAIT for a visible line would hold at once; a list
sets up line 0 with a WAIT in the blank of line 599). It executes one
instruction (two words, one slot) per free slot it gets; a WAIT compares
in every free slot without taking it, and once it holds the next
instruction runs in that same slot. Instruction: word 0 bits 15–13 are
the type:

| Type | Name | Word 0 | Word 1 |
| --- | --- | --- | --- |
| 000 | MOVE | bits 12–0: register word index (FFFF:0000 + 2 × index, up to FFFF:3FFE) | value |
| 001 | WAIT | bits 9–0: y | bits 10–0: x; bit 15: also wait for the Lithograph to finish |
| 010 | SKIP | bits 9–0: y | bits 10–0: x; skip the next instruction if the beam is at or past (y, x) |
| 011 | JUMP | bit 0: list (0 = list 1, 1 = list 2) | – |
| 100 | LITH | – | – : start the Lithograph job prepared in its registers |
| 101 | IRQ | – | value for JAQIRQ; raises the Jacquard interrupt |
| 110 | STOP | – | – : wait for the next field |
| 111 | – | reserved (acts as STOP) | |

WAIT holds until the beam is at or past (y, x) (y first, then x) and,
with bit 15, the Lithograph is idle. A MOVE into Babbage without JAQCON
bit 0 stops the Jacquard and sets DMAERR bit 4 (DMA-error interrupt).

### 6.3 Lithograph (FFFF:0100)

| Offset | Name | Meaning |
| --- | --- | --- |
| 0100 | LTHCON0 | bits 3–0 ROP; 4 use A; 5 use B; 6 use D; 8–9 op: 0 copy/ROP, 1 fill, 2 line, 3 shade |
| 0102 | LTHCON1 | bits 1–0 pixel size (0 = 4, 1 = 8, 2 = 16 bits); 2 flip X; 3 flip Y; 4 colour key on A; 5 nibble phase of A, 6 of B, 7 of D (4-bit: start at the high nibble = 0) |
| 0104/0106 | LTHASEG/OFS | source A |
| 0108/010A | LTHBSEG/OFS | source B |
| 010C/010E | LTHDSEG/OFS | destination D |
| 0110 | LTHAPITCH | bytes from one row to the next (signed) |
| 0112 | LTHBPITCH | |
| 0114 | LTHDPITCH | |
| 0116 | LTHW | width in pixels (1–4096) / line: dx |
| 0118 | LTHH | height in rows (1–4096) / line: dy |
| 011A | LTHKEY | colour key (A pixels equal to it are skipped) |
| 011C | LTHCOLOR | fill and line colour |
| 011E/0120 | LTHTSEG/OFS | 256-byte shade table |
| 0122 | LTHLINE | line: bit 0 x steps −1, bit 1 y steps −1, bit 2 y is the major axis |
| 0124 | LTHSTART | write: start |
| 0126 | LTHSTAT | bit 0 busy, bit 1 done (cleared on read) |

ROP: for each bit, D = ROP[ (A << 1) | B ] (bit index into the 4-bit
value); $C = A (copy), $A = B, $8 = A AND B, $E = A OR B, $6 = A XOR B.
Op fill: D = LTHCOLOR over the rectangle. Op line: Bresenham from D, LTHW
× LTHH as dx × dy, LTHLINE directions, colour LTHCOLOR. Op shade:
D = TABLE[B] where A (if used) is not LTHKEY, else D = B — darkening and
lightening for Panorama's shadows. With the colour key, A pixels equal to
LTHKEY leave D unchanged. Flips mirror the source walk. Each 32-bit
access is one slot. Completion raises the Lithograph interrupt.

**[S]** Details: a channel that is not used reads 0; a job works a row
at a time, a row costing, for each channel it uses, the 32-bit words its
bytes touch (a line pixel costs two slots); LTHSTART, or the Jacquard's
LITH, with a used pointer outside chip RAM starts nothing, sets DMAERR
bit 5 and raises the DMA-error interrupt (the Jacquard stops too); the
registers must not change while LTHSTAT bit 0 says busy.

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
opcode, 23–0 argument; parameter words follow. Pointers are linear chip
RAM addresses (segment << 16 | offset). Fixed point is 16.16, signed.

| Op | Name | Argument | Parameter words |
| --- | --- | --- | --- |
| 00 | END | – | – (the list is over) |
| 01 | MATRIX | – | 16: row-major 4 × 4, v' = M · (x, y, z, 1) |
| 02 | VERTICES | count (≤ 256) | pointer: transforms and lights the vertices (below) |
| 03 | TRIANGLES | count | pointer to index triples (16 bit) |
| 04 | LINES | count | pointer to index pairs |
| 05 | POINTS | count | pointer to indices |
| 06 | TEXTURE | bits 0–3 log2 width, 4–7 log2 height (≤ 8), 8–9 format (0 = 4 bit, high nibble first; 1 = 8 bit; 2 = 16 bit RGB 5:6:5) | pointer |
| 07 | STATE | bit 0 Gouraud, 1 texture, 2 Z test, 3 Z write, 4 backface culling, 5 lighting, 6 dither (8 bit), 7 blend, 8 fog | – |
| 08 | LIGHT | bits 0–7 ambient, 8–15 diffuse | 3: direction the light travels (camera space) |
| 09 | TABLES | – | 2: blend table, shade table (256 × 256 bytes each) |
| 0A | CLEAR | colour (8 or 16 bit) | 1: Z value (low 16 bits); clears the viewport, the Z buffer too when STATE has Z test or Z write |
| 0B | VIEWPORT | – | 4: x \| y << 16, w \| h << 16, near plane (16.16), perspective scale (16.16) |
| 0C | JUMP | – | pointer: the list continues there |
| 0D | FOG | – | 2: near and far distance (16.16) |

Vertex (24 bytes): x, y, z (3 × 16.16), normal (3 × s16, 1.15), u, v
(2 × s16, texels × 16), colour (16 bit). Indices past the last vertex
skip their primitive.

Behaviour **[S]** (step 7): every list starts from a reset state
(identity matrix, STATE 0, no texture, no tables, viewport the whole
target with the near plane at 1.0 and the scale half its width, light
straight ahead, ambient 0, diffuse 255). The camera looks along +z with
y up: a point lands at x = vx + w/2 + scale · x/z, y = vy + h/2 −
scale · y/z. A triangle faces the camera when it runs clockwise on the
screen; culling drops the others, without culling both sides are drawn.
Triangles are cut at the near plane, lines and points behind it are
dropped. Pixel centres at +0.5, a pixel on a shared edge belongs to one
triangle only (top-left rule). Light per vertex: I = ambient + diffuse ·
max(0, −n·l) with the transformed normal normalised, at most 255;
flat shading takes the first vertex's I, Gouraud interpolates it; with
lighting off I = 255. Textures repeat; u, v are exact every 16 pixels of
a row and linear in between. Z buffer: 16 bits per target pixel, a row
as many entries as the target row has pixels, value z · 256 (smaller is
nearer); the test passes when the new value is smaller. Fog scales I by
1 − (z − near) / (far − near), clamped. 8-bit targets: with lighting or
fog the index becomes shade[I · 256 + index] (I plus a 4 × 4 ordered
dither of −8…+7 when STATE bit 6), with blend blend[source · 256 +
destination]. 16-bit targets: R, G, B scaled by I / 255; blend is the
average of source and destination.

S3DSTAT: START while busy is ignored. The list runs when it is started
and the unit then stays busy for the slots its work costs: 100 a vertex,
6 a pixel reaching the Z test, 2 a cleared pixel with the Z buffer (1
without), 2 a command. It takes free chip-bus slots as the Lithograph does
(taking turns with it, DMACON bit 6); then done is set and the
Stereoscope interrupt raised. A bad list (unknown command, a pointer
beyond chip RAM, more than 2^20 commands) sets error, stops the list
there, raises DMAERR bit 6 and the DMA error interrupt. Done and error
clear when S3DSTAT is read. The unit computes in double precision; the
fixed-point formats are its interface. Throughput at these costs: about
300 000 vertices/s and 4 million pixels/s when every free slot is the
Stereoscope's.

## 8. Lumière: video and sprites (FFFF:0800)

### 8.1 Modes

| Mode | Picture | Pixel | Bytes/frame | Shown as |
| --- | --- | --- | --- | --- |
| 0 | 320 × 240 | 8 bit indexed | 76 800 | pixels doubled to 640 × 480, centred (border 80 left/right, 60 top/bottom) |
| 1 | 320 × 240 | 16 bit RGB 5:6:5 | 153 600 | likewise |
| 2 | 800 × 600 | 4 bit indexed (high nibble = left pixel) | 240 000 | full screen |
| 3 | 800 × 600 | 8 bit indexed | 480 000 | full screen |
| 4 | 100 × 37 characters | text, 8 × 16 font | 7 400 | 800 × 592, border 4 lines top and bottom |
| 5 | 1024 × 768 | 8 bit indexed | 786 432 | full screen, XGA timing (section 5) |

After reset every Lumière register and the palette are 0 (display off,
border palette entry 0) **[S]**. Blinking text and the cursor alternate
every 32 fields (about 0.53 s) **[S]**.

Text mode: each cell is two bytes, character then attribute (bits 0–3
foreground, 4–6 background, 7 blink), colours 0–15 of the palette's text
bank (`TXTBANK`). Font: 256 × 16 bytes in chip RAM (`FONTSEG/OFS`).

Mode 1 **[S]** (step 7): the border colour (`BORDER`) is RGB 5:6:5 itself,
FGINDEX has no effect, and a sprite with the "behind" bit shows only
where the background pixel is 0 (black); sprites keep their palette
colours.

The monitor **[S]** (owner's request 2026-11-06): it has two tubes, an
RGB tube with visible raster lines and a green phosphor tube that glows
and fades slowly; VMODE bit 13 selects the green one (Panorama sets it in
its oscilloscope theme). The picture's bytes are the same either way; in
mimic the frame is marked monochrome with bit 13, and the monitor
profiles `logophor`/`logophor-scope` (video-crt) follow the switch.

### 8.2 Registers

| Offset | Name | Meaning |
| --- | --- | --- |
| 0800 | VMODE | bits 2–0 mode; bit 13 the monitor's green phosphor tube; bit 15 display on (off: border colour only) |
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

**[S]** Details: the choice for a line is made at the start of the
horizontal blank before it (chip-bus slot 1 920), as part of the fixed DMA
(each chosen sprite's row costs the 32-bit words it touches); a change of
the table shows from the next line on. Sprites show only inside the
mode's picture area. A sprite pixel touches the "foreground" (SPRCOLFG)
over a background index ≥ FGINDEX, or ≠ 0 while FGINDEX is 0. Two sprite
pixels on top of each other set both groups' bits (group 0 takes no part).
In the text mode a background pixel's index is TXTBANK | its colour.

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

Behaviour **[S]** (step 8):

- Writing AUDCTL bit 0 from 0 to 1 starts the channel from AUDSEG/AUDOFS
  with AUDLEN (latched then); writing it 0 stops it. Period, volume, pan,
  the loop and envelope registers act at once. After AUDLEN samples the
  channel plays AUDLLEN samples from AUDLOOP, again and again; with
  AUDLLEN = 0 it stops (AUDCTL bit 0 and AUDSTAT bit 0 clear). With bit 6
  each such end raises the channel's interrupt (AUDIRQ bit n, AUDSTAT bit
  1; Telegraph source 5). The last sample sounds for its full period.
- DMA: at slot 1 920 every channel that plays samples, has 4 bytes free in
  its FIFO and data left takes one slot of the fixed DMA (after Lumière)
  and reads the aligned 32-bit word with its next bytes (only those from
  the pointer on). Without DMACON bit 2 no channel fetches. A channel
  whose FIFO is empty keeps its last sample and goes on when data comes
  (it plays slower than AUDPER asks).
- Timing: a channel takes a new sample every AUDPER ticks of 4 MHz (9 CPU
  cycles each); the channels are advanced and mixed once per output
  sample, every 750 CPU cycles (48 kHz).
- Envelope (AUDCTL bit 4): a level 0–255 stepped at the end of every
  line; attack rises by its rate to 255, decay falls to the sustain
  level, release (gate closed) to 0. A rate counts in 1/256 of a level a
  line (rate 1: 1.7 s for the whole range, 255: 6.8 ms); rate 0 is at
  once. The gate's rising edge starts the attack from 0. Without bit 4 the
  level is 255.
- Noise (bit 5): a 17-bit LFSR (taps 17 and 14) stepped at the channel's
  rate; its low byte is the sample. No DMA.
- Pan 0–16 keeps the left side at full and raises the right from 0; 16–31
  keeps the right at full and lowers the left to 0. Mix: Σ sample ×
  AUDVOL/127 × level/255 × pan gain, × AUDMASTER/127, clipped to 16 bits.
- A channel modulates with its new samples; channel 7 cannot modulate.

### 9.1 Phonograph: compressed audio (FFFF:3200)

Added 2026-11-06 on the owner's request (for the web radio): a decoder
for MP3 (MPEG-1/2 Layer III) and AAC (AAC-LC in ADTS frames), as a
chip, so the CPU only moves the stream's bytes **[S]**.

| Offset | Name | Meaning |
| --- | --- | --- |
| 3200 | PHID | read: $F0E1 |
| 3202 | PHCON | bit 0 on, bit 1 format (0 MP3, 1 AAC/ADTS), bit 2 interrupt when the ring is half empty |
| 3204/3206 | PHSEG/PHOFS | the ring buffer of compressed bytes in chip RAM |
| 3208 | PHSIZE | its size in bytes: a power of two, 256–32 768 |
| 320A | PHWR | the CPU's index: bytes written up to here (0 – size−1) |
| 320C | PHRD | read: the chip's index: bytes taken up to here |
| 320E | PHSTAT | bit 0 decoding, 1 starved (no whole frame in the ring: silence), 2 bad data skipped since the last read, 3 interrupt pending (bits 2 and 3 cleared on read) |
| 3210 | PHRATE | read: the stream's sample rate / 10 (4 410 for 44.1 kHz) |
| 3212 | PHCHAN | read: channels (1 or 2) |
| 3214 | PHKBPS | read: the last frame's bit rate in kbit/s (MP3; AAC: from the frame's size) |
| 3216 | PHVOL | volume 0–127 |
| 3218/321A | PHLEVL/PHLEVR | read: peak level (0–32 767) of each side since the last read |
| 321C/321E | PHFRMH/PHFRML | read: frames decoded since PHCON bit 0 was set (32 bits) |

Behaviour **[S]**:

- With PHCON bit 0 (and DMACON bit 7) the chip takes the bytes from PHRD
  up to PHWR (the ring wraps at PHSIZE; PHRD = PHWR: empty, so at most
  size − 1 bytes are in it). It looks for a frame header (MP3: 11 sync
  bits and a valid header; AAC: the ADTS sync word with layer 0) and
  decodes a frame only when all its bytes are in the ring; bytes before a
  header are skipped (PHSTAT bit 2). Setting bit 0 starts afresh (PHRD =
  PHWR is not changed; the frame count is reset).
- Output: the decoded samples are resampled (linear) to 48 kHz stereo
  (mono on both sides) and added to Calliope's mix before clipping, ×
  PHVOL / 127 × AUDMASTER / 127. The chip decodes the next frame when
  less than one frame of output is left; with no whole frame waiting it
  plays silence and sets PHSTAT bit 1 until one comes.
- Interrupt: with PHCON bit 2, each time the free space in the ring
  reaches half of PHSIZE, PHSTAT bit 3 and Telegraph source 12.
- DMA: the bytes a frame takes are charged to the following lines' fixed
  DMA, a slot for 4 bytes, after Calliope (128 kbit/s: about one slot
  every 9.4 lines).
- Savestate: registers, indices and counters; the decoder's inner state
  and the decoded samples not yet played are not kept: after loading,
  decoding starts again at the next frame (an MP3 frame that needs bits
  of the frames before it may sound wrong once).

Details **[S]** (fixed while building it):

- Frames taken: MP3 Layer III only, bit-rate index 1–14 (free format is
  refused), sample-rate index 0–2, MPEG-1, 2 and 2.5; ADTS with profile
  LC, sampling index 0–12, channel configuration 1 or 2 and one raw data
  block (a frame's CRC is skipped, not checked). Anything else is bad
  data: one byte is skipped and the search goes on (PHSTAT bit 2); so is
  a frame longer than PHSIZE − 1.
- The chip works once a line, at slot 1 920: when less than a frame of
  output is left it takes at most one whole frame (and the bad bytes
  before it). Every byte taken is owed to the fixed DMA, one slot a line
  while bytes are owed (a 417-byte frame: 105 slots).
- A frame the decoder refuses plays as silence of its length (1 152,
  576 or 1 024 samples) and sets bit 2, so PHRD, DMA and interrupts never
  depend on the decoder; PHFRM counts every frame taken.
- PHSTAT bit 0 is PHCON bit 0 with DMACON bit 7; bit 1 is set when an
  output sample finds nothing decoded and cleared when a frame is.
  PHLEVL/PHLEVR are the peaks before PHVOL. PHKBPS for AAC: the ADTS
  frame's length × 8 × rate / 1 024 / 1 000.
- PHSIZE is clamped to 256–32 768 and rounded down to a power of two
  (reset 256); PHWR and PHRD are kept modulo the size. PHVOL resets to 0.
  PHCON bit 0 from 0 to 1 clears the frame count, rate, channels, bit
  rate, the starved bit, the decoded samples and the decoder; from 1 to
  0 the decoded samples are dropped. A ring outside chip RAM clears
  PHCON bit 0 and sets DMAERR bit 7 (DMA-error interrupt).
- The half-empty interrupt fires when the frames taken in a line move
  the free space (size − 1 − used) from below PHSIZE / 2 to it or more.
  Resampling is linear with the exact step rate / 48 000.
- After loading a savestate the samples that were waiting play as
  silence; everything but the sound and the peaks stays as without the
  load.

Emulator: the decoders are Symphonia's (MPL-2.0, `symphonia-core`,
`symphonia-bundle-mp3`, `symphonia-codec-aac` 0.5.5); on an FPGA a
decoder core or a VS1053-like chip behind the same registers.

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

Behaviour **[S]** (step 10; owner's choices: a hub program, the chat as a
program on the card):

- The hardware works by the descriptors' OWN bits. Writing TXTAIL sends
  the descriptors from TXHEAD up to TXTAIL that the hardware owns
  (stopping at one the CPU owns): a frame of 14–1536 bytes gets its FCS
  (IEEE 802.3 CRC-32, low byte first) and goes on the wire; the
  descriptor's flags become 0 (sent) or bit 0 (a bad length or buffer),
  TXHEAD moves on, NETIRQ bit 1. Needs NETCON bit 1.
- A received frame (NETCON bit 0) passes the filter when it is for this
  MAC, a broadcast or multicast (bit 0 of its first byte), or with NETCON
  bit 2 always. It goes into the descriptor at RXHEAD if the hardware
  owns it (the CPU gives it a buffer and its size in the length field):
  length = the frame's bytes without the FCS, flags 0, bit 0 when cut to
  the buffer, bit 1 for a wrong FCS (the frame is still delivered);
  RXHEAD moves on, NETIRQ bit 0. No descriptor to take it: the frame is
  dropped, NETIRQ bit 2. RXTAIL is the CPU's bookkeeping only.
- A frame moves at once; its DMA slots (a slot for 4 bytes) are charged
  to the following lines' fixed DMA, 8 a line together with the SD card
  (the card first). DMACON bit 3.
- NETIRQ bits stay until written with 1; the network interrupt
  (Telegraph source 7) is raised when a bit NETIRQEN enables is set.
  NETSTAT bit 0 is the tunnel's state; a change sets NETIRQ bit 3.
- The MAC after power-on comes from the machine option `mac` (default
  02:4C:31:36:00:01, locally administered); software may change it.
- Tunnel: `--option net=HOST:PORT` names a hub (`ada-asm hub`, default
  port 6516). Each frame is one UDP datagram `LGN1`, 0, frame, FCS; a
  hello (`LGN1`, 1) goes out every 2 seconds so the hub knows the
  machine. The hub sends each frame on to every other machine it heard
  from in the last 30 seconds (an Ethernet hub; no echo to the sender).
  The machine looks at the socket every 16 lines through mimic's host
  network services, which a recording journals (deterministic replay);
  without them (the web frontend, `--net off`) the link stays down.
- **[S]** `--option net=host` (decision 83): no hub; the hub's gateway
  (section 11.3: 10.16.0.1 with ARP, ping, DHCP, DNS, UDP and TCP to the
  outside) runs inside the machine and reaches the Internet through the
  same host network services, so a single machine needs no `ada-asm hub`.
  The frames go to the gateway every 16 lines, its answers come in as
  received frames; the link is up while the host services exist. The
  host services connect by name and have no lookup of their own, so DNS
  gives each outside name a stand-in address from 198.18.0.0/15 (RFC
  2544's benchmarking net, never routed) and a connection to it goes out
  by the name; an address written as a number goes out as it is. Every
  name gets an address (a name that does not exist fails when connected
  to), and a machine's FIN does not half-close the host connection. Only
  the machine itself is on this net. The built-in profile `logophor16`
  uses `net=host`. Neither variant is machine state: a savestate load
  keeps the tunnel or the gateway as they are.

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

The SD card is a block device with DMA (no SPI). In the emulator the
card is an image file (whole 512-byte blocks) in a media slot.

Behaviour **[S]** (step 9):

- Timers count at 1 MHz (every 36th CPU cycle). A running timer at 0
  underflows on its next count: TSTAT bit n, the timer interrupt with
  TnCTL bit 3, then TnLATCH again (bit 1) or it stops (bit 0 clears);
  so it underflows every TnLATCH + 1 counts. With bit 2 timer n counts
  timer n − 1's underflows (timer 0 ignores the bit). TnCNT is readable
  and writable.
- The clock's second turns every 36 000 000 CPU cycles. A write to RTCHI
  waits for the write to RTCLO, which sets the clock and starts the
  second anew; reading RTCHI latches RTCLO for the next read. The alarm
  interrupt comes when the low 16 bits of the new second equal RTCALARM.
  The clock starts from the machine's `rtc` option (default 2026-01-01
  00:00); the emulator never reads the host's time inside the machine.
- Keyboard: every key and release enters the 16-entry FIFO (a full FIFO
  drops the event and sets KBDSTAT bit 1, cleared on read) and raises
  the keyboard interrupt; reading KBDDATA takes the oldest. The usages
  are USB HID's Keyboard/Keypad page, including the modifiers ($E0–$E7).
- Ports: PORTCFG shows what the machine's options plugged in (`port0`,
  `port1`: mouse, joystick, none); it cannot be written. Mouse motion
  adds to MOUSEnX/Y, PORTn bits 4–6 hold its buttons; a joystick sets
  PORTn bits 0–6. A change raises the ports interrupt.
- SD: the transfer runs through Telegraph's fixed-DMA slots, 8 a line
  (32 bytes), so a block takes 16 lines (about 0.42 ms, 1.2 MB/s; this
  replaces the 20 µs first written here). SDCMD while busy is ignored. A
  command ends with done (SDSTAT bit 3, cleared on read) and the SD
  interrupt; with error (bit 2) when there is no card, a block lies
  beyond the card, the buffer beyond chip RAM, or a write meets a
  write-protected card. Needs DMACON bit 3.

### 10.4 Interrupt controller

| Offset | Name | Meaning |
| --- | --- | --- |
| 4280 | INTREQ | S/C: pending sources |
| 4282 | INTENA | S/C: enabled sources |

| Bit | Source | Level |
| --- | --- | --- |
| 0 | VBlank | 4 |
| 1 | Jacquard (IRQ instruction) | 4 |
| 2 | DMA error | 4 |
| 3 | Lithograph done | 3 |
| 4 | Stereoscope done | 3 |
| 5 | Calliope (any channel, see AUDIRQ) | 6 |
| 6 | Timers (see TSTAT) | 5 |
| 7 | Network (see NETIRQ) | 2 |
| 8 | Keyboard | 1 |
| 9 | Mouse/joystick ports | 1 |
| 10 | SD card | 1 |
| 11 | RTC alarm | 1 |
| 12 | Phonograph (see PHSTAT) | 6 |
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

System font **[S]** (step 7, owner's choice): Martin Sedlák's Sweet16mono
(8 × 16, Boost Software License 1.0, <https://github.com/kmar/Sweet16Font>),
its first 256 characters, so the character set is ISO 8859-1; $00–$20
and $80–$A0 are blank. Foundry copies it to chip RAM for text mode.

Boot **[S]** (step 9; owner's choices: a FAT32 card, the demonstration
without one, a monitor with the basic functions):

1. A quick chip-RAM test (two patterns at four places in every segment;
   an error goes to the debug console), the system variables at
   003E:0000 cleared, the font copied.
2. The start picture: the owner's emblem (`asm/foundry/logo.png`, made a
   320 × 180 mode-0 picture with 256 colours at build time) for 2
   seconds. A key pressed then enters the emergency monitor.
3. The boot loader: an SD card with a FAT32 volume (MBR partition 1 of
   type $0B or $0C, or a volume without a partition table; 512-byte
   sectors, 8.3 names) and the file `BOOT/BOILER.BIN`: it is loaded at
   0040:0000 and started there with S0 = S1 = 0040, S2:R12 = 003F:FFFE,
   IPL 7, as the program slot does. Boiler (step 11) loads Panorama.
   The host tool `ada-asm sd` makes such cards.
4. Without one: the demonstration of steps 3–8 with its music; the text
   screen says why nothing started. A card inserted or removed makes
   Foundry start over (checked in the VBlank interrupt).

Emergency monitor **[S]** (text mode, light green on black): M addr
[lines] memory dump, E addr bytes… enter, F addr count byte fill, D addr
[count] disassemble, G addr call (the program returns with RETF), R the
registers saved at entry, S R|W block count seg SD blocks to/from chip
RAM seg:0000, B boot, X restart, ? help; numbers hex, an address
SSSS:OOOO or OOOO in the last segment. An unexpected exception or
interrupt (every vector but 4 and 12) enters it with the registers it
found and the line "Exception $vv at ssss:pppp".

ROM services **[S]**: TRAP #n with arguments in R1–R4 and the result in
R1; R5–R12 and S0–S3 are kept. A TRAP runs with IPL 7.

| n | Service |
| --- | --- |
| 0 | enter the monitor (does not return) |
| 1 | put character R1 on the console (text mode, set up at first use; 10 new line, 8 back) |
| 2 | wait for a key: R1 = its character (ISO 8859-1, US layout) |
| 3 | R1 = a waiting key's character, or 0 |
| 4 | put the zero-terminated string at S1:R1 on the console |
| 5 | read R3 SD blocks from block R1:R2 (high, low) into chip RAM R4:0000; R1 = 0 or an error |
| 6 | write R3 SD blocks likewise |
| 7 | network on: the rings and buffers at 001E:0000, transmit and receive on; R1 = NETSTAT |
| 8 | send the frame at S1:R1, R2 bytes (14–1536), and wait until it is out; R1 = 0 or 1 (error) |
| 9 | a received frame to S1:R1 (room for 1536 bytes): R1 = its length or 0, R2 = its descriptor flags |

Others return R1 = $FFFF. Open for step 11 (Boiler): the vector table is
fixed in the ROM, so a loaded program cannot yet install its own
interrupt handlers (it polls); file services through TRAP come with
Boiler too.

Panorama themes **[D]** (owner's request 2026-11-06, built in step 11):
besides the brass default, an **oscilloscope theme**, switchable at run
time: everything in shades of green only on black (like a green phosphor
tube), drawn with lines (outlines, no filled bevels or textures); window
frames, buttons, icons and text are line drawings in a few green levels,
the brightest for focus and highlights.

Calling convention **[S]**: arguments in R1–R4, results in R1 (R2 for
32-bit), R1–R4 and R10/R11 caller-saved, R5–R9 callee-saved, R12 stack
pointer in S2 (stack grows down, word-aligned), far pointers as
(segment, offset) pairs. ROM services through `TRAP #n` (above).

ROM layout **[S]** (step 9): FFFB code, tables, texts and the font; FFFC
the 3D scene's and the music's data; FFFD the start picture.

### 11.1 Boiler (step 11b)

Boiler is written in Brass (`crates/machine-logophor16/system/`: package
`boiler` the kernel, `files` the file system; the program's `main` is the
system's first task) and booted by Foundry as `BOOT/BOILER.BIN`.
Decisions **[S]**:

- Importing `boiler` starts it (its package init): the vector table is
  copied to 003C:0040 and VBR points there; Telegraph's timer 0 runs at
  100 Hz (latch 9 999) on level 5 (vector 13); the running program becomes
  task 1 (priority 10, its stack 003F); an idle task (priority 0) halts.
- Tasks: every task has a 64 KiB stack segment from $0100 on (240 at
  most); its registers are saved in its Task on every timer interrupt;
  the scheduler runs the ready task of the highest priority, in turn
  among equals, and the idle task when none is ready. Yield requests the
  timer interrupt by software (INTREQ), so a task switch is always the
  same path. Tasks run in supervisor mode with IPL 0; Lock/Unlock turn
  interrupts off and on (nesting).
- Brass's `go` starts a task with the caller's priority; channels are
  ring buffers of the element size (an unbuffered channel holds one
  value and the sender waits until it is taken); `select` takes the
  first ready case in order (Go chooses at random); a blocked task waits
  until any channel or mutex changes, then checks again.
- `Sleep(n)` in hundredths of a second; `Mutex` lets one task in (the
  owner may lock again); `Key()` reads the keyboard (the scheduler takes
  Telegraph's FIFO every tick; shift keys followed, US layout).
- Files: FAT32 (read and write, 8.3 names, directories), blocks through
  Foundry's TRAP #5/#6 into chip RAM (003B sectors, 003A FAT sectors),
  one task at a time (a mutex). Mount, Open, Create, Read, Write, Close,
  ReadDir, Mkdir, Remove.
- Programs are linked with Boiler into one binary for now; loading
  programs from the card as separate executables comes after step 11c
  (Panorama's applications are linked in as well).

### 11.2 Panorama (step 11c)

Panorama is Brass too (`system/`: package `gfx` draws, `panorama` is the
desktop, `info`, `archive` and `engine` are its applications; `main`
registers them and runs Panorama as task 1). Decisions **[S]**:

- Screen: mode 5 (1024 x 768, 8-bit pixels; mode 3's 800 x 600 until
  the owner asked for 1024 x 768 on 2026-11-06), two buffers at
  0020:0000 and 002C:0000, shown in turn at the VBlank. Panorama redraws only
  what changed: every change is a rectangle added to what each buffer
  lacks, and a buffer is composed through a clip rectangle (all of
  `gfx`'s drawing keeps to it).
- Chip RAM: windows' contents are surfaces (8-bit, own pitch) taken
  first-fit from 0002:0000-001D:FFFF; 0038 holds the glyphs (the system
  font as 8 x 16 bytes of 0/FF, drawn with the Lithograph's colour key),
  colour rows, the darken and lighten tables (for lit buttons, the
  Lithograph's shade mode), the pointer's image and the shadow tables
  (soft: the desk's grey a fine step darker; edge: other colours a step
  darker).
- Palette: 0-9 greys, 10-15 brass, 16-231 a 6 x 6 x 6 colour cube,
  232-250 the skin's 19 leather and brass tones (greys without a skin),
  251-252 British racing green (#004225) and a darker one for the title
  bars, 253-255 the desk's shadow greys (78, 71, 64 between Grey3 and
  Grey2).
  The oscilloscope theme replaces it with greens of the same brightness
  and draws outlines instead of filled surfaces, over a graticule.
- The skin (owner's artwork 2026-11-06, `system/panorama/tile.png`: an
  aged leather panel in a brass frame with a screw in each corner):
  `tile.rs` makes it at build time into `PANORAMA/TILE.RAW` on the
  system card (19 palette words with their darker and lighter
  neighbours for the shade tables, the panel at 48 x 48 for the dock's
  and the minimised windows' tiles, the panel at 160 x 160 for the
  windows). A window in the brass theme is drawn from the 160 x 160
  picture: its corners (12 x 12, the screws), rail pieces of 24 pixels
  from each side's middle repeated along the sides, and the leather
  from its middle (96 x 96) repeated, mirrored in turn so the seams
  meet; the content sits 12 pixels inside the frame on the left, right
  and bottom, the title bar on the top rail. Without the file the
  desktop stays plain; the oscilloscope theme keeps its outlines.
- Application tiles (owner's artwork 2026-11-06,
  `system/panorama/icons/`: Info, Archive, Engine Room, Radio,
  Pathfinder, Steamworks): 48 x 48 pictures after the skin in
  `TILE.RAW`, each under its application's name (16 bytes); the skin's
  19 tones are chosen over
  the panel and these pictures together. In the brass theme the dock
  shows an application's picture in place of its drawn tile and symbol;
  the oscilloscope theme and applications without one keep the drawn
  symbols.
- Wallpaper (owner's artwork 2026-11-06, `system/panorama/wallpaper.png`,
  aged leather): the top left quarter of the picture at 1024 x 768
  (512 x 384, 192 KiB of chip RAM) follows the skin in `TILE.RAW`;
  Panorama draws it four times, mirrored into the other quarters (the
  Lithograph's flips), as the desk of the brass theme. Without it the
  desk is plain grey; the oscilloscope theme keeps its graticule.
- Title bars in British racing green (owner's request 2026-11-06; the
  darker green for windows not in front) between the frame's corners,
  the title in brass. Window buttons from the owner's artwork
  (`system/panorama/buttons.png`, 18 x 18 with alpha, after the
  wallpaper in `TILE.RAW`; drawn with the colour key 0): minimise on
  the left, maximise and close (a skull) on the right.
- Maximise (owner's choice 2026-11-06: a real change of size): the
  button grows the window's frame up to the dock and the screen's
  bottom, a second press puts it back. A window's frame has its own size
  (CW x CH) apart from its content (W x H): content smaller than the
  frame sits in its middle on Paper. A window an application marks
  Resizable gets a Resize event with the new size; the application calls
  `SetSize` (a new surface, the old one freed; false when chip RAM is
  short) and draws again. Engine Room and Teleprinter (the text screen
  `term`, up to 120 x 46 characters) and Archive (more columns and rows)
  follow it; the games, Info and Radio keep their content in the middle.
- **[S]** Only Panorama's task changes the window list: a window an
  application opens or closes (`Done`) is noted and taken in (or out,
  its surfaces freed) between two passes of Panorama's loop, so drawing
  and hit tests never meet a window half gone (an application closing
  its window while Panorama drew it ended in the monitor).
- The pointer is sprite 0 (16 x 16, 8-bit), moved every 20 ms from
  Boiler's mouse (port 0, counters read each tick, kept inside the
  screen). Boiler keeps a button pressed since the last look as pressed
  once, so a busy desktop loses no click.
- Layout (NeXTSTEP's): the dock on the right with 48 x 48 tiles, one
  per application (running applications carry a small steam gauge whose
  needle rises with their windows; the machine's emblem tile that opened
  Info again is gone, owner's request 2026-11-06); the
  application menu at the top left (Info, Archive, Engine Room, Theme,
  Hide windows) with the Theme submenu, which is torn off by dragging its
  title and then stays open; windows with a dark title bar (minimise on
  the left, close on the right), rounded corners and a shadow of two
  thin lines below and right (owner's request 2026-11-06: the old
  shadow of three wide steps was too coarse): on the desk the inner line
  two of the desk's fine greys darker (three for the front window), the
  outer one, over other colours the inner line one step darker;
  menus and tiles alike; minimised windows as tiles
  along the bottom. Buttons and items lighten under the pointer.
- Applications are Boiler tasks. A window's events (Key, Click with the
  position in its content, Release, Close, Theme) come through its
  channel of 16; Panorama never waits for an application (a full channel
  drops the event). An application draws into its window's surface and
  says which part changed.
- Info shows the uptime, tasks and free chip RAM; Archive browses the
  card in a column view; Engine Room is the command shell (help, ls, cd,
  cat, mkdir, rm, echo, cls, ps, mem, theme, exit), as many as wanted.
- Under Panorama, `print` and `println` reach the debug port only
  (Foundry's console would switch the picture to text).
- The built-in profile `logophor16` boots into Panorama: the machine
  option `card=system` puts in a system card made at build time from
  `system/` (BOOT/BOILER.BIN on 64 MiB FAT32, kept in memory; writes are
  lost at the end). A card of one's own (`--disk`) takes its place.
- Backlog (owner's request 2026-11-06): Notepad, Calendar and a task
  list (the web radio was built on 2026-11-06, section 11.5).

### 11.3 The network (step 11d)

Owner's choices 2026-11-06: Teleprinter reaches other Logophors and the
Internet (the hub is a gateway); the stack as the brief lists it, in
full. Decisions **[S]**:

- The net: IPv4 10.16.0.0/24. The hub (`ada-asm hub`) is also a host on
  it, the **gateway**: 10.16.0.1, MAC 02:4C:31:36:FF:01. It sees every
  frame (it is the hub) and answers those for its MAC, broadcasts
  included: ARP for 10.16.0.1 (and for every address outside the net:
  the gateway's MAC), ICMP echo to 10.16.0.1.
- DHCP (server on the gateway, UDP 67/68, RFC 2131): addresses from
  10.16.0.10 up in the order MACs first ask, kept for the hub's run;
  options subnet mask 255.255.255.0, router and DNS server 10.16.0.1,
  domain `logophor`, lease 1 hour; the client's host name (option 12)
  is remembered for DNS.
- DNS (server on the gateway, UDP 53, RFC 1035, A records only): the
  machines' host names (`name` or `name.logophor`), `hub` and `gateway`
  for 10.16.0.1; other names through the host's resolver (IPv4
  addresses); unknown: NXDOMAIN.
- Outside the net (the Internet through the host's sockets; NAT): UDP
  datagrams go out from a host socket per machine address and port, the
  answers come back from the outside address. TCP is terminated in the
  hub: a SYN to an outside address makes the hub connect there (no
  connection: RST); then it carries the bytes both ways as a TCP peer of
  its own (MSS 1460, the machine's window respected, retransmission after
  1 s, FIN both ways). Pings beyond 10.16.0.1 get no answer (the host
  gives no raw sockets).
- `ada-asm hub --no-gateway` keeps the plain relay of step 10.
- The machine's stack is a Boiler task (package `net`): Ethernet through
  Telegraph's rings, ARP with a cache, IPv4 (no fragments; MTU 1500),
  ICMP (echo answered, `Ping`), UDP, TCP (RFC 793 states, the three-way
  handshake, a sliding window, retransmission with a timeout, FIN and
  RST), a DHCP client at start (the host name `logophor-XXXX` from the
  MAC's last two bytes), a DNS client.
- Teleprinter is the network terminal: it connects by name and port
  (TCP), shows what comes (refusing Telnet options) and sends what is
  typed; it can also wait for a call on a port (Logophor to Logophor).
  Engine Room gains `ifconfig`, `ping`, `host`.

### 11.4 Games (step 11e)

Owner's choices 2026-11-06: his own game Pathfinder as the public game,
Steamworks as well, and Comet Sweep hidden. Decisions **[S]**:

- **Pathfinder** (`system/pathfinder`) is the owner's tip-over puzzle
  for GUI64 on the C64 (`gui64_apps/Pathfinder`), ported to Brass with
  its rules, its 10 levels and its texts: towers of 1-4 boxes on 6 x 6
  cells, a reachable tower is tipped over and lies on as many free cells
  as it was high, the level is solved when the red box can be reached.
  Cursor keys or W/A/S/D, Return or Space, U undo, R restart; the mouse
  chooses and tips; GUI64's game menu becomes the keys N and P (next and
  previous level).
- **Steamworks** (`system/steamworks`): pipe pieces on 8 x 6 cells are
  turned by clicks until the steam runs from the boiler on the left to the
  engine on the right with no pipe open; levels are mazes made from the
  level's number and turned at random (always solvable).
- Panorama keeps a hidden game. Its way in is not documented.
- For games, Boiler gives the keys held now (`Held`, by USB HID usage),
  the cursor keys as characters ($81-$84) and the mouse buttons pressed
  since the last look; a window can be locked while a frame is drawn in
  steps (`BeginPaint`/`EndPaint`), so Panorama never shows half a frame.

### 11.5 Radio (2026-11-06, owner's request)

Radio (`system/radio`) plays Internet radio through the hub's gateway and
Phonograph (section 9.1). Decisions **[S]**:

- Stations: `/RADIO/STATIONS.TXT` on the card, a line
  `Name|http://host[:port]/path`, up to 12; without the file a built-in
  list (owner's choice: Deutschlandfunk, DLF Kultur, DLF Nova, WDR 2,
  1LIVE, WDR 3, WDR 5, Radio Bob, Radio Bob Alternative; all redirect to
  their stream hosts, checked 2026-11-06). Only `http://`:
  the Logophor has no TLS, so a station that moves to `https://` is
  reported as such; `http://` redirects are followed (three at most).
- HTTP/1.0 with `Icy-MetaData: 1`: the answer's status (`HTTP/1.x` or
  `ICY`), `icy-metaint`, `icy-name` and `content-type` (`audio/aac…`:
  AAC, otherwise MP3) are read; metadata blocks are taken out of the
  stream and their `StreamTitle` shown; the audio bytes go into a 16 KiB
  ring in chip RAM for Phonograph.
- The window: the stations (a click plays one, a second click stops),
  the station's name, the title, rate, channels, bit rate and format, a
  level meter for each side; + and − set the volume, S stops.

## 12. Host tool chain

All in Rust in mimic's workspace **[D]**:

- `asm-ada`: assembler with macros and a linker (objects, sections,
  far labels: `seg(label)`, `off(label)`), output raw images, ROM images
  and listings. Syntax: one instruction per line, registers `r0`–`r12`
  (`sp` = r12, `lr` = r11), `#imm`, `off(rb)`, labels with `:`,
  directives `.org .seg .word .byte .ascii .align .include .macro`.
- Emulator: mimic's frontends and debugger (breakpoints, stepping, memory;
  Jacquard list, sprites and framebuffer through mimic's graphics views).
- Later phases: PNG → chunky converter with a 5:6:5 palette, SD image
  tool, C back end (`char` 8, `int` 16, `long` 32, pointers 32 bits).

## 13. Emulation in mimic

- Machine id `logophor16`; crates `cpu-ada` (CPU), `chips-logophor`
  (Babbage, Lumière, Calliope, Telegraph), `machine-logophor16`,
  `asm-ada` (assembler, also used by tests and to build the ROM).
- Frame: 800 × 600 RGB (the full visible area; modes 0, 1 and 4 with
  their borders), signal metadata 40 MHz pixel clock, 1060 × 628, VGA RGB
  output.
- **Shipped as a standard profile** (owner's request): the Foundry ROM is
  assembled from `rom/` sources at build time and embedded, so the
  machine needs no user files; mimic carries a built-in profile
  `logophor16` that the desktop and web profile pickers show even
  without a `profiles.yaml` (a user profile of the same name overrides
  it). SD card images go into a media slot.
- A program slot (`--prg`) loads a raw binary at 0040:0000 and starts it
  there in supervisor mode (S1 = S0, stack S2:R12 = 003F:FFFE), as the
  host tool `ada-asm run` does; the debug console's output stays in the
  machine. The machine option `roms` is accepted and ignored (the ROM is
  built in).
- Savestates, debugger targets (`cpu`; Jacquard and Lithograph as devices),
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
13. Fixed DMA starts at the horizontal blank, in the brief's priority order; free slots by priority; BUSCON for CPU before Lithograph.
14. Calliope gets one slot per channel and line (32 bytes/line, not 16) and a 4 MHz period clock.
15. Jacquard: 8 instruction types incl. STOP; MOVE reaches FFFF:0000–3FFE.
16. Interrupt levels: Calliope 6, timers 5, VBlank/Jacquard/DMA error 4, Lithograph/3D 3, network 2, input/SD/RTC 1, NMI 7.
17. Keyboard reports USB HID usages; SD card is a DMA block device.
18. Sprite width stays 8/16/32 (value 3 reserved); coordinates offset by 64; in modes 0/1 in 320 × 240 pixels.
19. Modes 0/1 are shown pixel-doubled, centred in 800 × 600.
20. Debug port at FFFF:43F0 (console output, test exit code).
21. Byte writes to registers write the low byte, high byte 0.
22. Calling convention R1–R4 arguments, R5–R9 callee-saved.
23. Unused instruction fields must be 0 (else illegal), step 2.
24. Lumière's registers and palette are 0 after reset; blink and cursor phase 32 fields (step 3).
25. The Jacquard restarts at the field start, not at the VBlank; a WAIT compares without taking a slot (step 4).
26. FBNOW (FFFF:081E) takes the framebuffer pointer over at the line's end, for split screens; VSTAT bit 2 only at the VBlank (step 4).
27. A CPU instruction touching chip RAM starts only in a cycle with a free slot (step 4).
28. Lithograph: unused channels read 0; row-wise work with slot costs per touched word, a line pixel 2 slots; a bad pointer refuses the start with DMAERR bit 5 (step 5).
29. Sprites: chosen at slot 640 of the line before, only inside the picture area; the foreground for SPRCOLFG is FGINDEX and up (or any non-zero index while FGINDEX is 0) (step 6).
30. Stereoscope list formats as in section 7: counts in the argument, pointers as parameter words, the FOG command 0D added; the second table is a shade table for light and fog (step 7).
31. Every display list starts from a reset state (step 7).
32. Camera along +z with y up, front faces clockwise on the screen, light direction in camera space (step 7).
33. 16-bit targets scale RGB directly by the light, blend is the average (step 7).
34. The list runs at START; the unit is then busy for its slot costs (100 a vertex, 6 a pixel, 2/1 a cleared pixel, 2 a command), taking turns with the Lithograph (step 7).
35. Mode 1 border in RGB 5:6:5, FGINDEX unused there, "behind" sprites show over black (step 7).
36. The machine is called Logophor 16 (owner's decision 2026-11-06; formerly Cogitatron 16).
37. System font Sweet16mono, ISO 8859-1 (owner's choice, step 7).
38. Panorama gets a second theme, "oscilloscope": green shades and lines only (owner's request 2026-11-06, for step 11).
39. Calliope: AUDCTL bit 0 starts (0 → 1) and stops; AUDLEN latched at the start, the other registers act at once (step 8).
40. Calliope DMA: one aligned 32-bit word per line and channel at slot 640 when 4 FIFO bytes are free; an empty FIFO holds the last sample (step 8).
41. Calliope envelope rates in 1/256 level a line, 0 = at once; the gate's rising edge restarts the attack from 0 (step 8).
42. Calliope noise: 17-bit LFSR at the channel rate; pan as balance (centre = both sides full); mix summed and clipped (step 8).
43. Foundry's VBlank interrupt (vector 12) runs the music player and acknowledges the VBlank; other level-4 sources stop there as before (step 8).
44. Taking any exception or interrupt sets IPL to 7; RTI restores it (step 9; fixes the shadow-register race of step 2's rule).
45. Timers, clock, keyboard, ports and SD as in section 10.3's behaviour list; the SD card moves 32 bytes a line, a block in 16 lines (step 9).
46. The clock starts from the machine option `rtc`, never from the host's time (determinism) (step 9).
47. Boot from a FAT32 SD card: `BOOT/BOILER.BIN` to 0040:0000; without one the demonstration; the start picture is the owner's emblem; a key during it enters the monitor (owner's choices, step 9).
48. ROM services TRAP #0–6 as in section 11; the vector table stays in the ROM until step 11 (step 9).
49. ROM data in segments FFFC (3D, music) and FFFD (start picture) (step 9).
50. The network's emulator side is a hub program (`ada-asm hub`, UDP, default port 6516) that relays frames between machines; `--option net=HOST:PORT` (owner's choice, step 10).
51. Network behaviour as in section 10.1: OWN bits drive the rings, frames move at once, their DMA slots are charged after the SD card's, a wrong FCS is delivered with flag bit 1 (step 10).
52. Default MAC 02:4C:31:36:00:01, option `mac` (step 10).
53. ROM services TRAP #7–9 for the network; the chat is a program on the card (`asm/examples/chat.s`) (owner's choice, step 10).
54. System register 6 VBR moves the vector table (owner's choice, step 11a).
55. Step 11 in five parts, each accepted: 11a modes 2/3, VBR, the language Brass and its compiler; 11b Boiler's kernel; 11c Panorama; 11d the network stack and Teleprinter; 11e a game in Brass (owner's choice).
56. The high-level language is Brass, after Go (owner's choice); its specification is `docs/logophor16/BRASS.md`.
57. The hub serves DHCP and DNS for the machines on it (owner's choice, built in 11d).
58. Boiler as section 11.1: tasks on the 100 Hz timer with priorities, Yield through the timer interrupt, stacks as segments from $0100, vector table at 003C:0040 (step 11b).
59. Channels as ring buffers, select takes the first ready case, blocked tasks recheck on any change (step 11b).
60. Boiler's file system FAT32 with 8.3 names through Foundry's block services (step 11b).
61. Panorama on mode 3 (800 x 600, 8 bit), double-buffered, redrawn only where changed (a rectangle per buffer, drawn through a clip rectangle) (step 11c).
62. Panorama's fixed palette (greys, brass, 6 x 6 x 6 cube); the oscilloscope theme swaps in greens of the same brightness and draws outlines (step 11c).
63. Panorama's applications are Boiler tasks with an event channel per window; Panorama never waits for them (step 11c).
64. Under Panorama print/println go to the debug port only (step 11c).
65. The built-in profile boots Panorama from a system card built from `system/` (machine option `card=system`); an own card replaces it (step 11c).
66. Notepad, Calendar, Web radio and a task list in the backlog (owner's request 2026-11-06).
67. Loading separate executables from the card moved after step 11c (step 11c).
68. The net 10.16.0.0/24 with the hub as gateway 10.16.0.1 (MAC 02:4C:31:36:FF:01): DHCP, DNS (machines' names, `hub`, the host's resolver), NAT for UDP, TCP terminated in the hub (owner's choice: the Internet through the hub) (step 11d).
69. Boiler's network stack as the brief lists it, in full (owner's choice), as a task of package `net`; host name `logophor-XXXX` from the MAC (step 11d).
70. Teleprinter: TCP by name and port, Telnet options refused, also waiting for a call; Engine Room gains ifconfig, ping, host (step 11d).
71. Mode 5: 1024 x 768 with 8 bits a pixel and XGA field timing (664 slots and 249 CPU cycles a line, 806 lines), taken at a field's start; modes 0-4 keep SVGA; Panorama moves to mode 5 (owner's choice 2026-11-06: a new mode, not the whole machine).
72. Boiler remembers a mouse button pressed since the last look (no lost clicks) (step 11d).
73. Step 11e's games: the owner's Pathfinder (ported from GUI64) and Steamworks in the dock, and a hidden game (owner's choice).
74. Boiler: `Held` keys, cursor keys as $81-$84, `Mouse` returns the presses since the last call; Panorama: windows locked while a frame is painted (step 11e).
75. Step 11 accepted 2026-11-06; step 12 (the optional FPGA core) moved to mimic's backlog (owner's choice).
76. Phonograph (section 9.1), a decoder chip for MP3 and AAC (owner's choice 2026-11-06), for the radio; Symphonia in the emulator.
77. Radio (section 11.5): stations from the card or a built-in list, HTTP with Icecast metadata, http only (no TLS) (owner's choice 2026-11-06: chip and app together).
78. Babbage's Blitter renamed Lithograph (owner's request 2026-11-06): registers LTH… (were BLT…), the Copper instruction LITH (was BLIT); the CPU's branch BLT keeps its name.
79. Babbage's Copper renamed Jacquard (owner's request 2026-11-06): registers JAQ… (were COP…: JAQ1SEG, JAQJMP1, JAQCON, JAQIRQ …).
80. The monitor's two tubes (VMODE bit 13): RGB with raster lines, green phosphor with glow for the oscilloscope theme; shown by the CRT stage, raw pictures unchanged (owner's choice 2026-11-06).
81. Panorama's skin from the owner's leather-panel artwork: the dock's tiles at 48 x 48, the windows from a 160 x 160 picture cut into corners, rails and the leather, 24 palette entries for its tones (owner's request 2026-11-06).
82. CPU and chip bus three times as fast: Ada at 36 MHz, 2 544 slots a line (1 992 in XGA fields), the fixed DMA from slot 1 920 (1 536); the slot numbers in decisions 29 and 40 are those of the old clock (owner's request 2026-11-06 "as fast as possible", in real time: the emulator runs the Foundry at about 1.4 times real time on the development PC with this factor, too little for four).
83. `net=host`: the hub's gateway inside the machine through mimic's host network services, DNS answering stand-in addresses from 198.18.0.0/15 for outside names (the services connect by name); the built-in profile uses it (owner's choice 2026-11-06, after the radio did not play without a hub).
84. Application tiles from the owner's artwork in the dock (brass theme; Steamworks' too), no emblem tile, shadows as two thin lines over three fine desk greys (palette 253-255, the skin down to 21 tones); the window list changes only in Panorama's task (owner's report 2026-11-06: closing a window crashed the system).
85. Wallpaper (a mirrored quarter of the owner's leather picture), title bars in British racing green, window buttons from the owner's artwork (owner's requests 2026-11-06); palette 251-252 for the greens, the skin down to 19 tones.
86. Maximise: the frame grows to the screen, Resizable applications get a Resize event and SetSize their content, the others keep it in the middle (owner's choice 2026-11-06).
