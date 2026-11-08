# Memory freezer, cheats and remote device control

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

Two related features: a **software memory freezer** in the emulator core, and emulation of **freezer cartridges** (Action Replay & co.) as real hardware.

### Software memory freezer (`emu-core::freeze`)

```rust
pub struct Freeze {
    pub space: MemSpaceId,      // CPU view, chip RAM, drive RAM, VDC RAM …
    pub addr: u32,
    pub size: Size,             // Byte | Word | Long
    pub value: u32,
    pub mode: FreezeMode,       // BlockWrites | ReapplyPerFrame | ReapplyOnRead
    pub enabled: bool,
    pub label: Option<String>,  // "Lives", "Energy"
}
```

- **BlockWrites** (default): guest writes to the address are suppressed at bus level; reads return the frozen value. Implemented with the same per-space bitmap mechanism as watchpoints, so zero cost when no freeze is active.
- **ReapplyPerFrame**: value written at the start of each frame (closest to classic trainers / POKE cheats; tolerates code that checks its own writes).
- **ReapplyOnRead**: value forced only when read (for I/O-mapped or bank-switched addresses).
- Freezes respect banking: a freeze is bound to a memory space + physical address, not just a CPU address. Freezes on I/O registers are allowed but flagged in the UI.
- Freezes are **part of savestates and input replays** so runs stay reproducible.

### Cheat search (memory finder)

Classic narrowing search over a memory space:

1. `search_start(space, size)` snapshots all candidate addresses.
2. Repeat `search_next(op)` with `op` = equal/not-equal to a value, increased, decreased, unchanged, changed, increased-by-N.
3. Remaining candidates can be watched live (values update with each frame) and turned into freezes in one step.

Search runs on `peek` snapshots taken at frame boundaries — it never halts or slows emulation beyond the copy.

### Cheat files

- Per-game cheat sets in TOML (`cheats/<machine>/<game-hash>.toml`), keyed by media/image hash so they apply automatically.
- Import of common formats where documented: POKE lists, VICE-style monitor commands, Amiga Action Replay codes, Atari/CPC POKE lists.
- Cheats can include **code patches** (byte sequences at addresses) in addition to value freezes.

### Freezer cartridges (hardware)

Emulated as `Cartridge` port devices with a **freeze button** (NMI + bank switch), so original cartridges and their menus work:

- C64/C128: Action Replay, Retro Replay, Final Cartridge III, Super Snapshot.
- Amiga: Action Replay (A500 side-port).
- Freeze button is triggerable from UI, hotkey and the debug server (`press_cartridge_button`).

### Debugger and preview integration

- Commands: `freeze_add`, `freeze_remove`, `freeze_list`, `freeze_toggle`, `search_start`, `search_next`, `search_results`, `cheats_load`, `cheats_save`.
- Event `freeze_blocked_write` (optional, rate-limited) tells the debugger which code tried to change a frozen address — useful for finding the routine to patch.
- Memory views highlight frozen addresses.

### Remote control of devices

The debug server exposes devices too, so tools, tests and the web preview can drive a machine:

- `list_ports`, `connect_device`, `disconnect_device`
- `list_media_slots`, `insert_media`, `eject_media`, `set_drive_mode`, `flush_media`
- `input` (inject `InputEvent`s, timestamped), `record_input`, `replay_input`
- `controls`, `set_control` (console panel: the machine's buttons and switches; protocol v8)

The web frontend uses these over WebSocket to forward browser keyboard/gamepad/mouse input, making the live preview interactive.
