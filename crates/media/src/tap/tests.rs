use super::*;

fn header(version: u8, machine: u8, data: &[u8]) -> Vec<u8> {
    let mut v = b"C64-TAPE-RAW".to_vec();
    v.extend([version, machine, 0, 0]);
    v.extend((data.len() as u32).to_le_bytes());
    v.extend_from_slice(data);
    v
}

#[test]
fn versions_and_long_pulses() {
    let v0 = Tap::parse(&header(0, 0, &[0x30, 0, 0x56])).unwrap();
    assert_eq!(v0.pulses, [0x180, 2048, 0x2b0]);
    let v1 = Tap::parse(&header(1, 1, &[0x30, 0, 0x10, 0x27, 0x00, 0x42])).unwrap();
    assert_eq!(v1.pulses, [0x180, 10_000, 0x210]);
    assert_eq!(v1.machine, TapMachine::Vic20);
    assert_eq!(v1.clock_hz(), 1_108_405);
    assert!(!v1.half_waves);
    let v2 = Tap::parse(&header(2, 2, &[0x20])).unwrap();
    assert!(v2.half_waves);
    // Written back: version 1, the long pulse as an exact length.
    let bytes = v1.to_bytes().unwrap();
    assert_eq!(bytes.len(), v1.capacity);
    assert_eq!(Tap::parse(&bytes).unwrap(), v1);
    assert!(Tap::parse(b"NOT-A-TAPE-AT-ALL...").is_err());
    let mut short = header(1, 0, &[0x30; 4]);
    short.truncate(22);
    assert!(Tap::parse(&short).is_err());
}

#[test]
fn blank_tapes_fill_up() {
    let mut t = Tap::blank(TapMachine::C64, Video::Pal, 30);
    assert_eq!(t.to_bytes().unwrap().len(), 30);
    t.pulses = vec![0x180; 10];
    let bytes = t.to_bytes().unwrap();
    assert_eq!(Tap::parse(&bytes).unwrap().pulses.len(), 10);
    t.pulses.push(0x180);
    assert!(t.to_bytes().is_err(), "full");
}

/// Read back the bytes of a CBM recording (C64 PAL cycles).
fn decode(pulses: &[u32]) -> Vec<u8> {
    let kind = |p: u32| match p {
        0..=0x1bf => b'S',
        0x1c0..=0x25f => b'M',
        _ => b'L',
    };
    let k: Vec<u8> = pulses.iter().map(|&p| kind(p)).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 20 <= k.len() {
        if &k[i..i + 2] == b"LM" {
            let mut b = 0u8;
            let mut parity = 1;
            for bit in 0..9 {
                let v = match &k[i + 2 + bit * 2..i + 4 + bit * 2] {
                    b"SM" => 0,
                    b"MS" => 1,
                    _ => panic!("bad bit at {i}"),
                };
                if bit < 8 {
                    b |= v << bit;
                    parity ^= v;
                } else {
                    assert_eq!(v, parity, "parity");
                }
            }
            out.push(b);
            i += 20;
        } else {
            i += 1;
        }
    }
    out
}

#[test]
fn cbm_recording_holds_header_and_data_twice() {
    let prg = [0x01, 0x08, 0xaa, 0xbb, 0xcc];
    let tap = encode_cbm(&prg, b"HELLO", true, TapMachine::C64, Video::Pal).unwrap();
    let bytes = decode(&tap.pulses);
    // Header: 9 sync + 192 + checksum, twice; data: 9 + 3 + 1, twice.
    assert_eq!(bytes.len(), 2 * (9 + 192 + 1) + 2 * (9 + 3 + 1));
    assert_eq!(
        &bytes[..10],
        &[0x89, 0x88, 0x87, 0x86, 0x85, 0x84, 0x83, 0x82, 0x81, 1]
    );
    assert_eq!(&bytes[10..14], &[0x01, 0x08, 0x04, 0x08], "start, end + 1");
    assert_eq!(&bytes[14..19], b"HELLO");
    let data = &bytes[2 * 202..];
    assert_eq!(&data[9..13], &[0xaa, 0xbb, 0xcc, 0xaa ^ 0xbb ^ 0xcc]);
    assert_eq!(data[13], 0x09, "second copy");
    // A leader of short pulses comes first.
    assert!(tap.pulses[..0x6a00].iter().all(|&p| p == 0x180));
    // On a VIC-20 tape the same timing takes more cycles.
    let vic = encode_cbm(&prg, b"HELLO", true, TapMachine::Vic20, Video::Pal).unwrap();
    assert_eq!(vic.pulses[0], 432);
}

#[test]
fn ted_recording_is_half_waves() {
    let prg = [0x01, 0x10, 0x42];
    let tap = encode_ted(&prg, b"T", true, Video::Pal).unwrap();
    assert!(tap.half_waves);
    assert_eq!(tap.machine, TapMachine::C16);
    assert_eq!(tap.clock_hz(), 886_724);
    // A lead-in half wave, then waves of two equal halves; the leader is
    // short waves.
    assert_eq!(tap.pulses[0], 496);
    assert!(tap.pulses[1..].chunks(2).all(|w| w[0] == w[1]));
    assert!(tap.pulses[1..1 + 2 * 0x3ffe].iter().all(|&p| p == 216));
    let bytes = tap.to_bytes().unwrap();
    assert_eq!(&bytes[..12], b"C16-TAPE-RAW");
    assert_eq!(bytes[12], 2);
    assert_eq!(Tap::parse(&bytes).unwrap().pulses.len(), tap.pulses.len());
}
