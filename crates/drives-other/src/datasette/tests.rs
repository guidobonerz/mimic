use super::*;
use emu_core::media::MediaBackend;
use std::sync::{Arc, Mutex};

struct Shared(Arc<Mutex<Vec<u8>>>);

impl MediaBackend for Shared {
    fn len(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let d = self.0.lock().unwrap();
        buf.copy_from_slice(&d[offset as usize..offset as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let mut d = self.0.lock().unwrap();
        d[offset as usize..offset as usize + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

const PAL: u32 = 985_248;

fn tape(pulses: &[u32]) -> Arc<Mutex<Vec<u8>>> {
    let mut t = Tap::blank(TapMachine::C64, Video::Pal, 0);
    t.pulses = pulses.to_vec();
    Arc::new(Mutex::new(t.to_bytes().unwrap()))
}

fn insert(d: &mut Datasette, image: &Arc<Mutex<Vec<u8>>>) {
    d.insert(Media {
        name: "t.tap".into(),
        kind: MediaKind::Tape,
        backend: Box::new(Shared(image.clone())),
    })
    .unwrap();
}

/// Cycles between falling edges of READ over `n` cycles.
fn falling_edges(d: &mut Datasette, n: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut last = d.read();
    let mut since = 0;
    for _ in 0..n {
        d.tick();
        since += 1;
        let now = d.read();
        if last && !now {
            out.push(since);
            since = 0;
        }
        last = now;
    }
    out
}

#[test]
fn plays_pulses_while_the_motor_runs() {
    let mut d = Datasette::new(TapMachine::C64, PAL);
    assert!(!d.sense());
    insert(&mut d, &tape(&[400, 400, 600, 800, 400]));
    assert!(d.sense(), "PLAY pressed on insert");
    // Motor off: nothing moves.
    for _ in 0..5000 {
        d.tick();
    }
    assert_eq!(d.counter().0, 0);
    d.set_motor(true);
    let edges = falling_edges(&mut d, 5000);
    // The first edge comes at the end of the first pulse.
    assert_eq!(edges, [400, 400, 600, 800, 400]);
    assert_eq!(d.counter(), (5, 5));
}

#[test]
fn a_faster_host_sees_longer_pulses() {
    let mut d = Datasette::new(TapMachine::C64, 2 * PAL);
    insert(&mut d, &tape(&[400, 400, 400]));
    d.set_motor(true);
    assert_eq!(falling_edges(&mut d, 5000), [800, 800, 800]);
}

#[test]
fn keys_and_spooling() {
    let mut d = Datasette::new(TapMachine::C64, PAL);
    assert_eq!(d.control("play"), Err(MediaError::NoMedia));
    insert(&mut d, &tape(&[400; 10]));
    d.set_motor(true);
    falling_edges(&mut d, 2000);
    let pos = d.counter().0;
    d.control("stop").unwrap();
    assert!(!d.sense());
    falling_edges(&mut d, 2000);
    assert_eq!(d.counter().0, pos, "stopped");
    d.control("forward").unwrap();
    assert_eq!(d.counter().0, 10);
    d.control("rewind").unwrap();
    assert_eq!(d.counter().0, 0);
    assert!(matches!(d.control("eject"), Err(MediaError::Control(_))));
}

#[test]
fn records_write_pulses_and_writes_them_back() {
    let image = Arc::new(Mutex::new(blank_tape(TapMachine::C64, Video::Pal, 1)));
    let mut d = Datasette::new(TapMachine::C64, PAL);
    insert(&mut d, &image);
    d.control("record").unwrap();
    d.set_motor(true);
    // A square wave: rising edges 384, 384, 528 and 688 cycles apart.
    d.set_write(false);
    for len in [384u32, 384, 528, 688] {
        d.set_write(true);
        for _ in 0..len / 2 {
            d.tick();
        }
        d.set_write(false);
        for _ in 0..len - len / 2 {
            d.tick();
        }
    }
    d.set_write(true);
    d.set_motor(false);
    let back = Tap::parse(&image.lock().unwrap()).unwrap();
    assert_eq!(back.pulses, [384, 384, 528, 688]);
    // Played back as recorded.
    d.control("rewind").unwrap();
    d.control("play").unwrap();
    d.set_motor(true);
    assert_eq!(falling_edges(&mut d, 3000), [384, 384, 528, 688]);
    // A broken-off tab records nothing.
    d.write_protect(true);
    d.control("rewind").unwrap();
    d.control("record").unwrap();
    for _ in 0..3 {
        d.set_write(true);
        d.set_write(false);
    }
    d.control("stop").unwrap();
    assert_eq!(Tap::parse(&image.lock().unwrap()).unwrap().pulses.len(), 4);
}

#[test]
fn half_waves_toggle_read() {
    let mut t = Tap::blank(TapMachine::C16, Video::Pal, 0);
    t.pulses = vec![296, 496, 304];
    let image = Arc::new(Mutex::new(t.to_bytes().unwrap()));
    let mut d = Datasette::new(TapMachine::C16, 886_724);
    insert(&mut d, &image);
    d.set_motor(true);
    let mut changes = Vec::new();
    let mut last = d.read();
    for c in 1..=1200 {
        d.tick();
        if d.read() != last {
            changes.push(c);
            last = d.read();
        }
    }
    assert_eq!(changes, [296, 792, 1096]);
}

#[test]
fn savestate_round_trip() {
    let image = tape(&[400; 20]);
    let mut a = Datasette::new(TapMachine::C64, PAL);
    insert(&mut a, &image);
    a.set_motor(true);
    falling_edges(&mut a, 2345);
    let mut w = StateWriter::new();
    a.save(&mut w);
    let data = w.into_bytes();
    let mut b = Datasette::new(TapMachine::C64, PAL);
    insert(&mut b, &image);
    b.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(falling_edges(&mut a, 3000), falling_edges(&mut b, 3000));
    assert_eq!(a.counter(), b.counter());
    assert!(b.load(&mut StateReader::new(&data[..5])).is_err());
}

#[test]
fn a_motor_stop_keeps_the_recording_going() {
    let image = Arc::new(Mutex::new(blank_tape(TapMachine::C16, Video::Pal, 1)));
    let mut d = Datasette::new(TapMachine::C16, 886_724);
    insert(&mut d, &image);
    d.control("record").unwrap();
    d.set_motor(true);
    let mut level = true;
    let mut edge = |d: &mut Datasette, n: u32| {
        level = !level;
        d.set_write(level);
        for _ in 0..n {
            d.tick();
        }
    };
    edge(&mut d, 200);
    edge(&mut d, 300);
    // The motor stops between two blocks and starts again.
    d.set_motor(false);
    d.set_motor(true);
    edge(&mut d, 400);
    edge(&mut d, 0);
    d.set_motor(false);
    let back = Tap::parse(&image.lock().unwrap()).unwrap();
    assert_eq!(back.pulses, [200, 304, 400], "300 rounds to 304 in a TAP");
}

#[test]
fn status_shows_the_moving_tape() {
    let mut d = Datasette::new(TapMachine::C64, PAL);
    assert_eq!(d.status(), Default::default());
    insert(&mut d, &tape(&[100, 100]));
    assert!(!d.status().active, "PLAY down, motor off");
    d.set_motor(true);
    assert!(d.status().active);
    d.control("stop").unwrap();
    assert!(!d.status().active);
    d.control("record").unwrap();
    assert_eq!(
        d.status(),
        DriveStatus {
            active: true,
            error: false,
            track: None
        }
    );
}
