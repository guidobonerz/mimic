# Phase 42: Ultimate 64 (dropped)

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 42 | Dropped 2026-11-23 (user decision): the Ultimate 64 / Commodore 64 Ultimate as a machine is off the list again | – | – |

Added, built and dropped on 2026-11-23, all on the user's word. Built that day and removed again ("entferne den u64 und c64u teil wieder"): a machine `ultimate64` in `machine-c64` with the models `u64` and `c64u`, the turbo CPU with its registers, a rebuilt menu and a firmware updater with an emulated flash. The commits a998f96 and 6fb60ad hold that state.

Why it was dropped: the user's aim was to try firmware and cores before flashing a real board. A menu and an updater rebuilt here cannot do that; it needs the real firmware running on an emulated board processor (Nios II on the Ultimate 64, RISC-V on the Elite II board) with the board's flash, USB, overlay and C64 interface. Another project, UE2-C64U-Emulator (GPL-3), does this for the Elite II board and the Commodore 64 Ultimate.

What stays: the C64's own options (`uci`, `reu`, `userport=wic64`, `sd2iec` …) were there before and are untouched. Golden rule 7 keeps its wording of the same day (sources may be read for facts, code is never taken over).

Not to be begun again without the user asking for it.
