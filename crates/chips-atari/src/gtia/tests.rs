use super::*;

const LINE: u16 = 50;
/// A colour clock inside the output.
const CC: u16 = 100;

fn gtia() -> Gtia {
    let mut g = Gtia::new(VideoStandard::Pal);
    g.write(0x12, 0x46); // COLPM0 red
    g.write(0x16, 0xc6); // COLPF0 green
    g.write(0x18, 0x94); // COLPF2 blue
    g.write(0x17, 0x0e); // COLPF1 white luma
    g.write(0x1a, 0x00); // COLBK
    g.write(0x00, CC as u8); // HPOSP0
    g.write(0x0d, 0x80); // GRAFP0: leftmost pixel
                         // Position writes reach the player logic a few colour clocks later.
    render_line(&mut g);
    g.write(0x1e, 0); // HITCLR
    g
}

fn pixel(g: &Gtia, cc: u16) -> [u8; 2] {
    let x = usize::from(LINE - FIRST_LINE) * WIDTH as usize + usize::from(cc - FIRST_CC) * 2;
    [g.pixels()[x], g.pixels()[x + 1]]
}

fn render(g: &mut Gtia, cc: u16, pf: Playfield) {
    let cycle = cc / 2;
    let mut both = [Playfield::Background; 2];
    both[usize::from(cc & 1)] = pf;
    g.render_cycle(LINE, cycle, both);
}

#[test]
fn priority_players_over_or_under_playfield() {
    let mut g = gtia();
    g.write(0x1b, 0x01); // players over playfield
    render(&mut g, CC, Playfield::Pf(0));
    assert_eq!(pixel(&g, CC), [0x46, 0x46]);
    g.write(0x1b, 0x04); // playfield over players
    render(&mut g, CC, Playfield::Pf(0));
    assert_eq!(pixel(&g, CC), [0xc6, 0xc6]);
    // Next to the player only the playfield / background.
    render(&mut g, CC + 1, Playfield::Background);
    assert_eq!(pixel(&g, CC + 1), [0x00, 0x00]);
}

#[test]
fn collisions_latch_until_hitclr() {
    let mut g = gtia();
    render(&mut g, CC, Playfield::Pf(0));
    assert_eq!(g.peek(0x04), 0x01, "P0PF: playfield 0");
    g.write(0x01, CC as u8); // player 1 on top of player 0
    g.write(0x0e, 0x80);
    render_line(&mut g);
    assert_eq!(g.peek(0x0c), 0x02, "P0PL: player 1");
    assert_eq!(g.peek(0x0d), 0x01, "P1PL: player 0");
    g.write(0x1e, 0);
    assert_eq!(g.peek(0x04), 0);
    assert_eq!(g.peek(0x0c), 0);
}

#[test]
fn hires_pixels_take_pf1_luma() {
    let mut g = gtia();
    g.write(0x0d, 0); // no player
    render(&mut g, CC, Playfield::Hires(0b10));
    assert_eq!(pixel(&g, CC), [0x9e, 0x94]);
}

/// Render the whole line over background.
fn render_line(g: &mut Gtia) {
    for cycle in 0..114 {
        g.render_cycle(LINE, cycle, [Playfield::Background; 2]);
    }
}

#[test]
fn player_sizes() {
    let mut g = gtia();
    g.write(0x08, 0x03); // quadruple width: one bit covers 4 colour clocks
    render_line(&mut g);
    for cc in CC..CC + 4 {
        assert_eq!(pixel(&g, cc), [0x46, 0x46], "cc {cc}");
    }
    assert_eq!(pixel(&g, CC + 4), [0, 0]);
}

#[test]
fn player_retriggers_while_shifting() {
    let mut g = gtia();
    g.write(0x0d, 0x81); // pixels 0 and 7
    render_line(&mut g);
    // Started again 2 clocks later: the first image goes on under the new
    // one.
    for cycle in 0..114 {
        if cycle == CC / 2 - 2 {
            g.write(0x00, (CC + 2) as u8);
        }
        g.render_cycle(LINE, cycle, [Playfield::Background; 2]);
    }
    for cc in CC..CC + 12 {
        let want = if [CC, CC + 2, CC + 7, CC + 9].contains(&cc) {
            0x46
        } else {
            0
        };
        assert_eq!(pixel(&g, cc), [want, want], "cc {cc}");
    }
}

#[test]
fn gtia_mode_9_luminances() {
    let mut g = gtia();
    g.write(0x0d, 0);
    g.write(0x1a, 0x80); // COLBK hue 8
    g.write(0x1b, 0x40);
    g.render_cycle(
        LINE,
        CC / 2,
        [Playfield::Hires(0b10), Playfield::Hires(0b11)],
    );
    assert_eq!(pixel(&g, CC), [0x8b, 0x8b]);
}

#[test]
fn inputs_and_pal_register() {
    let mut g = gtia();
    assert_eq!(g.peek(0x10), 1, "TRIG0 up");
    g.set_trigger(0, true);
    assert_eq!(g.peek(0x10), 0);
    g.set_trigger(0, false);
    g.write(0x1d, 0x04); // latch triggers
    g.set_trigger(0, true);
    g.set_trigger(0, false);
    assert_eq!(g.peek(0x10), 0, "latched");
    g.write(0x1d, 0x00);
    assert_eq!(g.peek(0x10), 1);
    assert_eq!(g.peek(0x14), 0x01, "PAL");
    assert_eq!(Gtia::new(VideoStandard::Ntsc).peek(0x14), 0x0f);
    g.console_keys = 0x01;
    assert_eq!(g.peek(0x1f) & 0x07, 0x06, "START down");
}

#[test]
fn savestate_round_trip() {
    let mut g = gtia();
    render(&mut g, CC, Playfield::Pf(0));
    let mut w = StateWriter::new();
    g.save(&mut w);
    let bytes = w.into_bytes();
    let mut h = Gtia::new(VideoStandard::Pal);
    h.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(g, h);
}
