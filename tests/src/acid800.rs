//! Runner for Avery Lee's Acid800 Atari 8-bit test suite (standalone
//! tests).
//!
//! Each standalone test is an XEX with a label file (`.lab`, MADS label
//! table). A test ends in one of the library routines `_testPassed`,
//! `_testFailed` or `_testSkipped`; the runner loads the XEX through the
//! machine's program slot, sets execution breakpoints on those three
//! labels and stops at the first one hit. After a failure it lets the test
//! print its message and reads it from the OS text screen (SAVMSC at
//! `$58`). Everything goes through the generic `Machine`/`DebugHooks`
//! interface.

use emu_core::media::{Media, MediaBackend, MediaError, MediaKind};
use emu_core::registry::{MachineConfig, Registry};
use emu_core::{BreakpointKind, DebugHooks, DeviceId, Machine, MemSpaceId, Size, StopReason};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Skipped,
    Timeout,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub verdict: Verdict,
    /// The test's message lines from the screen (failures, skips).
    pub message: String,
    pub frames: u32,
}

/// Addresses of `_testPassed`, `_testFailed`, `_testSkipped` in a MADS
/// label table (`bank<TAB>addr<TAB>name` lines).
pub fn result_labels(labels: &str) -> Option<[u32; 3]> {
    let find = |name: &str| {
        labels.lines().find_map(|l| {
            let mut f = l.split('\t');
            let (_, addr, n) = (f.next()?, f.next()?, f.next()?);
            (n.trim() == name)
                .then(|| u32::from_str_radix(addr.trim(), 16).ok())
                .flatten()
        })
    };
    Some([
        find("_testPassed")?,
        find("_testFailed")?,
        find("_testSkipped")?,
    ])
}

struct Bytes(Vec<u8>);

impl MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(self.0.get(o..o + buf.len()).ok_or(MediaError::OutOfRange)?);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Err(MediaError::WriteProtected)
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn cpu_space(m: &dyn Machine) -> MemSpaceId {
    m.memory_spaces()
        .into_iter()
        .find(|s| s.name == "cpu")
        .map(|s| s.id)
        .expect("cpu memory space")
}

fn byte(m: &mut dyn Machine, space: MemSpaceId, addr: u32) -> u8 {
    let cpu = m.debug_target(DeviceId("cpu")).expect("cpu");
    cpu.peek(space, addr, Size::Byte).unwrap_or(0) as u8
}

/// The OS text screen as lines of ASCII (internal screen codes mapped
/// back; inverse video ignored).
pub fn screen_text(m: &mut dyn Machine) -> Vec<String> {
    let space = cpu_space(m);
    let base = u32::from(byte(m, space, 0x58)) | (u32::from(byte(m, space, 0x59)) << 8);
    (0..24)
        .map(|row| {
            let line: String = (0..40)
                .map(|col| {
                    let c = byte(m, space, base + row * 40 + col) & 0x7f;
                    match c {
                        0x00..=0x3f => (c + 0x20) as char,
                        0x60..=0x7f => c as char,
                        _ => ' ',
                    }
                })
                .collect();
            line.trim_end().to_string()
        })
        .collect()
}

/// Run one standalone test.
pub fn run(
    registry: &Registry,
    config: &MachineConfig,
    xex: &[u8],
    labels: &str,
    disk: Option<&[u8]>,
    max_frames: u32,
) -> Result<Outcome, String> {
    let [passed, failed, skipped] =
        result_labels(labels).ok_or("label file without _testPassed/_testFailed/_testSkipped")?;
    let mut m = registry
        .create("atari8", config)
        .map_err(|e| e.to_string())?;
    if let Some(disk) = disk {
        if let Some(d1) = m
            .media_slots()
            .into_iter()
            .find(|s| s.kind == MediaKind::Disk)
        {
            m.media_slot(d1.id)
                .ok_or("no disk slot")?
                .insert(Media {
                    name: "acid800.atr".into(),
                    kind: MediaKind::Disk,
                    backend: Box::new(Bytes(disk.to_vec())),
                })
                .map_err(|e| e.to_string())?;
        }
    }
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == MediaKind::Program)
        .ok_or("no program slot")?
        .id;
    m.media_slot(slot)
        .ok_or("no program slot")?
        .insert(Media {
            name: "test.xex".into(),
            kind: MediaKind::Program,
            backend: Box::new(Bytes(xex.to_vec())),
        })
        .map_err(|e| e.to_string())?;
    let space = cpu_space(m.as_ref());
    let mut dbg = DebugHooks::new();
    let ids = [passed, failed, skipped]
        .map(|addr| dbg.add_breakpoint(BreakpointKind::Exec { space, addr }));
    let mut frames = 0;
    let verdict = loop {
        if frames >= max_frames {
            break Verdict::Timeout;
        }
        match m.run_frame(&mut dbg) {
            StopReason::FrameDone => frames += 1,
            StopReason::Breakpoint(id) if id == ids[0] => break Verdict::Pass,
            StopReason::Breakpoint(id) if id == ids[1] => break Verdict::Fail,
            StopReason::Breakpoint(id) if id == ids[2] => break Verdict::Skipped,
            other => {
                return Ok(Outcome {
                    verdict: Verdict::Timeout,
                    message: format!("stopped: {other:?}"),
                    frames,
                })
            }
        }
    };
    let mut message = String::new();
    if matches!(verdict, Verdict::Fail | Verdict::Skipped) {
        // Let the library restore the screen and print its message.
        let mut quiet = DebugHooks::new();
        for _ in 0..30 {
            m.run_frame(&mut quiet);
        }
        let lines = screen_text(m.as_mut());
        let start = lines
            .iter()
            .rposition(|l| l.contains("FAIL") || l.contains("Skipped"))
            .unwrap_or(0);
        message = lines[start..]
            .iter()
            .filter(|l| !l.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" / ");
    }
    Ok(Outcome {
        verdict,
        message,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_result_labels() {
        let lab = "mads 2.1.0\nLabel table:\n00\t1A26\t_exitTest\n00\t1DF8\t_testPassed\n\
                   00\t1E0D\t_testFailed\n00\t1E3A\t_testSkipped\n";
        assert_eq!(result_labels(lab), Some([0x1df8, 0x1e0d, 0x1e3a]));
        assert_eq!(result_labels("00\t1DF8\t_testPassed\n"), None);
    }
}
