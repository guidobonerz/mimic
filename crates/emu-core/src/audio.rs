//! Audio output of a machine: signed 16-bit samples at a fixed rate,
//! produced as part of the emulation (deterministic, savestate-covered).
//! Frontends resample or buffer as their device needs.

/// Format of the samples [`crate::Machine::audio`] appends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioFormat {
    pub sample_rate: u32,
    /// Interleaved channels (1 = mono, 2 = stereo).
    pub channels: u8,
}

/// The default output rate of all machines.
pub const SAMPLE_RATE: u32 = 48_000;

/// Fractional resampler from a chip clock to [`SAMPLE_RATE`] by averaging
/// all input samples of an output period (box filter). Integer arithmetic,
/// so the output is identical on every platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decimator {
    in_rate: u32,
    out_rate: u32,
    phase: u32,
    sum: i64,
    count: u32,
}

impl Decimator {
    /// `None` if a rate is 0 or the output rate exceeds the input rate.
    pub fn new(in_rate: u32, out_rate: u32) -> Option<Self> {
        (in_rate > 0 && out_rate > 0 && out_rate <= in_rate).then_some(Self {
            in_rate,
            out_rate,
            phase: 0,
            sum: 0,
            count: 0,
        })
    }

    /// Feed one input sample; returns an output sample when one is due.
    #[inline]
    pub fn push(&mut self, sample: i32) -> Option<i16> {
        self.sum += i64::from(sample);
        self.count += 1;
        self.phase += self.out_rate;
        if self.phase < self.in_rate {
            return None;
        }
        self.phase -= self.in_rate;
        let avg = self.sum / i64::from(self.count);
        self.sum = 0;
        self.count = 0;
        Some(avg.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16)
    }

    pub fn save(&self, w: &mut crate::StateWriter) {
        w.u32(self.phase);
        w.u64(self.sum as u64);
        w.u32(self.count);
    }

    pub fn load(&mut self, r: &mut crate::StateReader<'_>) -> Result<(), crate::StateError> {
        let phase = r.u32()?;
        let sum = r.u64()? as i64;
        let count = r.u32()?;
        if phase >= self.in_rate || count > self.in_rate {
            return Err(crate::StateError::Invalid("audio resampler"));
        }
        (self.phase, self.sum, self.count) = (phase, sum, count);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_and_average() {
        let mut d = Decimator::new(985_248, SAMPLE_RATE).unwrap();
        let out: Vec<i16> = (0..985_248).filter_map(|i| d.push(i % 2 * 1000)).collect();
        assert_eq!(out.len(), 48_000);
        assert!(
            out.iter().all(|&s| (400..=600).contains(&s)),
            "average of 0/1000"
        );
        assert!(Decimator::new(100, 200).is_none());
    }

    #[test]
    fn clamps() {
        let mut d = Decimator::new(2, 1).unwrap();
        d.push(100_000);
        assert_eq!(d.push(100_000), Some(i16::MAX));
    }
}
