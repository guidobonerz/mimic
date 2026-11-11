# Display and CRT emulation

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

### What machines output (`FrameRef`)

Every machine delivers, per field:

- the **visible area including overscan/border** as a monitor shows it, centred with equal borders left and right (VIC-II: VICE's normal border, 32 pixels each side; PAL 384×272), as indexed pixels + palette (8-bit machines) or RGB (Amiga);
- **signal metadata**: pixel clock, active/total pixels per line, lines per field, field parity (interlace), video standard (PAL/NTSC), output type (composite, S-video/luma-chroma, RGB, monochrome);
- optionally **per-line luma/chroma values** instead of palette indices, so the composite path can reproduce colors palettes cannot express.

Cropping, aspect ratio and pixel-aspect correction (e.g. C64 PAL 0.9365, CPC mode 2 half-width pixels) are applied downstream from this metadata, never inside the machine. A progressive signal above 24 kHz line rate (576p/480p, the MEGA65) shows each line once: its pixel aspect is 2 × the square-pixel clock / the pixel clock (`video_crt::cpu::pixel_aspect`, 2026-11-11).

### Pipeline in `video-crt`

```
raw field ─► [signal stage] ─► [deinterlace/field stage] ─► [CRT stage] ─► host output
             composite encode/decode   weave / bob /           scanlines, mask,
             (optional, per standard)  phosphor-blend          bloom, curvature, persistence
```

1. **Signal stage** (optional, per monitor profile): models the connection the user picked.
   - Composite NTSC: chroma/luma crosstalk and **artifact colors** — required for Atari 8-bit games that rely on artifacting, and for C64/VIC-20 NTSC looks.
   - Composite PAL: **delay-line averaging** of chroma between lines — produces the blended colors C64 PAL demos rely on (e.g. "PAL color mixing").
   - S-video / luma-chroma (C64, C128 40-col, Atari 800XL): reduced crosstalk.
   - RGB (Amiga, CPC, C128 VDC): clean, no signal stage.
   - Monochrome (CPC GT65 green, PET built-in white/green): luma only, phosphor tint.
2. **Field stage:** interlace handling for Amiga (weave, bob, or phosphor-persistence blend) and 50/60 Hz field pacing. **Vector stage** (`video_crt::phosphor::VectorPhosphor`, 2026-11-11): a vector display (the Vectrex, `Machine::vectors`) through a profile with `phosphor_ms` > 0 is drawn from its beam segments, each with its afterglow from the moment the beam drew it (`Segment::at`): within the frame a segment gives `1 − e^(−(F−t)/τ)` of its energy, the rest glows into the next frames; energy is conserved, so a still picture keeps the machine's brightness and τ → 0 gives the machine's own picture. Same rasteriser and curve as the machine (`emu_core::vector::Raster`). Desktop (emulation thread, `Shared::vectors`), web, headless (`--crt`, every frame) and the preview stream (`filter=crt`, one stage per stream; single snapshots stay stateless) use it.
- **Overlays** (`video_crt::overlay`, 2026-11-11): a picture in front of the screen (the Vectrex's sheets), the user's PNG from `~/.mimic/overlays/<machine>/<cartridge stem>.png` (`media_backends::overlays`; the built-in game `minestorm.png`), laid over the picture after the vector stage and before the CRT stage: alpha = coverage, colour = what the filter lets through, plus a faint room light (`AMBIENT` 0.06) on the sheet. Desktop follows the cartridge (`--overlay FILE|none`), headless only with `--overlay FILE|auto`, web with an uploaded PNG named like the cartridge or the "Overlay" choice (command `overlay`). Never in the debugger's or the raw preview's picture (golden rule 9). The Vectrex's frames are `OutputType::Rgb` since (as `Monochrome` the signal stage kept only luma and lost the 3D Imager's and an overlay's colours).
3. **CRT stage:** scanlines (beam width tied to brightness), shadow mask / aperture grille / slot mask, bloom/halation, barrel curvature, vignette, **phosphor decay** across frames, configurable gamma.

### Monitor profiles

TOML presets, each choosing signal type + CRT parameters: Commodore 1702, 1084S (RGB and composite inputs), Amstrad CTM644 (color) and GT65 (green), Atari via TV RF/composite, PET built-in, the Vectrex's tube (`vectrex`: glow, slight curvature, `phosphor_ms = 6`, a look, not measured), plus "generic PAL TV", "generic NTSC TV" and "raw (sharp pixels)". Users can override any parameter; per-machine default profiles live in machine config.

### Implementation notes

- GPU path: WGSL shaders via `wgpu` in desktop and web frontends; multi-pass, render targets sized from signal metadata (not the window).
- CPU path: a simplified, deterministic implementation for headless use and the preview server (snapshots with `filter=crt`), so CI and remote viewers can see CRT output without a GPU.
- Effects must work at integer and non-integer scale; scanline count always follows the emulated line count, not the window height.
- Phosphor persistence keeps its own frame history in `video-crt`, never in machine state (savestates stay CRT-independent).
- Light pens and anything beam-position-dependent use raster timing from the core, not the CRT stage's geometry.
- Performance budget: full CRT pipeline at 50/60 Hz on integrated GPUs at 1080p; provide a "lite" preset for weak hardware and WASM.

### Preview and debugger integration

- Preview parameters: `filter=raw|crt`, `profile=<name>`; default for the stream is `raw`, default for the desktop window is the machine's monitor profile.
- Debug overlays (raster grid, DMA map, beam position) are always drawn on the raw frame, optionally side by side with the CRT view.
