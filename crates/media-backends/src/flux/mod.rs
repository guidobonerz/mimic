//! Flux: the time between magnetic flux transitions as a flux reader
//! (Greaseweazle) samples them, and the way from there to images.
//!
//! - [`Flux`]: intervals in sample ticks plus the index pulses.
//! - [`decode_bits`]: a phase-locked loop turning intervals into bit cells
//!   (a transition is a 1, every cell without one a 0) — the data
//!   separator a floppy controller has in hardware.
//! - [`encode_bits`]: the reverse with a seeded jitter, so tests can make
//!   flux from known images without a drive.
//! - [`gcr`], [`mfm`]: bits to sectors for Commodore GCR, Amiga MFM and
//!   IBM MFM (1581, PC/ST); [`disk`]: disk formats, the track list and the
//!   image assembled from the tracks read.
//!
//! The PLL is this project's own: each interval is quantised to a whole
//! number of cells of the current period, the period follows the error
//! slowly (within ±10 % of nominal) and a part of the phase error carries
//! into the next interval. Sources for the recording methods: see the
//! submodules.

pub mod disk;
pub mod gcr;
pub mod mfm;

/// Flux transitions of one read: `intervals[i]` ticks between transition
/// `i - 1` (or the start of the read) and transition `i`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flux {
    /// Sample ticks per second.
    pub sample_freq: u32,
    pub intervals: Vec<u32>,
    /// Index pulses, ticks from the start of the read.
    pub index: Vec<u64>,
}

impl Flux {
    /// Mean ticks per revolution between index pulses.
    pub fn revolution_ticks(&self) -> Option<f64> {
        let (first, last) = (self.index.first()?, self.index.last()?);
        let n = self.index.len().checked_sub(1).filter(|&n| n > 0)?;
        Some((last - first) as f64 / n as f64)
    }

    /// Drive speed from the index pulses; `None` without two of them.
    pub fn rpm(&self) -> Option<f64> {
        let rev = self.revolution_ticks()?;
        (rev > 0.0).then(|| 60.0 * f64::from(self.sample_freq) / rev)
    }

    /// Total ticks of the read.
    pub fn duration(&self) -> u64 {
        self.intervals.iter().map(|&i| u64::from(i)).sum()
    }
}

/// Bit cells decoded from a [`Flux`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bits {
    pub bits: Vec<bool>,
    /// Bit position of each index pulse.
    pub index: Vec<usize>,
}

impl Bits {
    /// The bits packed MSB first (the last byte padded with zeros).
    pub fn packed(&self) -> Vec<u8> {
        pack(&self.bits)
    }

    /// One revolution: the bits from the first index pulse to the second,
    /// or everything without two pulses.
    pub fn revolution(&self) -> &[bool] {
        match self.index.as_slice() {
            [a, b, ..] if a < b && *b <= self.bits.len() => &self.bits[*a..*b],
            _ => &self.bits,
        }
    }
}

/// Bits packed MSB first.
pub fn pack(bits: &[bool]) -> Vec<u8> {
    bits.chunks(8)
        .map(|c| {
            c.iter()
                .enumerate()
                .fold(0u8, |b, (i, &v)| b | u8::from(v) << (7 - i))
        })
        .collect()
}

/// Bits from bytes, MSB first.
pub fn unpack(bytes: &[u8]) -> Vec<bool> {
    bytes
        .iter()
        .flat_map(|&b| (0..8).rev().map(move |i| b >> i & 1 != 0))
        .collect()
}

/// How much of the phase error the PLL corrects at once.
const PHASE_GAIN: f64 = 0.65;
/// How much of the error goes into the period.
const PERIOD_GAIN: f64 = 0.05;
/// How far the period may drift from nominal.
const PERIOD_RANGE: f64 = 0.10;
/// Longer gaps (no flux, unformatted areas) count as this many cells.
const MAX_CELLS: f64 = 1024.0;

/// Decode flux into bit cells of nominally `cell_ticks` ticks.
pub fn decode_bits(flux: &Flux, cell_ticks: f64) -> Bits {
    let mut out = Bits {
        bits: Vec::with_capacity(flux.intervals.len() * 3),
        index: Vec::with_capacity(flux.index.len()),
    };
    if cell_ticks <= 0.0 {
        return out;
    }
    let (min, max) = (
        cell_ticks * (1.0 - PERIOD_RANGE),
        cell_ticks * (1.0 + PERIOD_RANGE),
    );
    let mut period = cell_ticks;
    let mut carry = 0.0;
    let mut now: u64 = 0;
    let mut next_index = flux.index.iter().peekable();
    for &d in &flux.intervals {
        let start = now;
        now += u64::from(d);
        let start_bit = out.bits.len();
        let x = f64::from(d) + carry;
        let n = (x / period).round().clamp(1.0, MAX_CELLS);
        // Index pulses within this interval land at their cell.
        while let Some(&&at) = next_index.peek() {
            if at >= now {
                break;
            }
            let cells = ((at.saturating_sub(start)) as f64 / period).round() as usize;
            out.index.push(start_bit + cells.min(n as usize - 1));
            next_index.next();
        }
        out.bits.extend(std::iter::repeat_n(false, n as usize - 1));
        out.bits.push(true);
        let err = x - n * period;
        if n < MAX_CELLS {
            period = (period + err * PERIOD_GAIN / n).clamp(min, max);
            carry = err * (1.0 - PHASE_GAIN);
        } else {
            carry = 0.0;
        }
    }
    for _ in next_index {
        out.index.push(out.bits.len());
    }
    out
}

/// A small deterministic random number generator (xorshift64*) for the
/// jitter of generated flux.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in -1.0..1.0.
    pub fn signed_unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

/// Flux of `revs` revolutions of a track's bits (one revolution, read
/// from the index), cells of `cell_ticks`, each transition moved by up to
/// `jitter` cells at random. The read starts `lead_in` cells before the
/// first index pulse (a real read starts anywhere); a revolution ends at
/// the next index.
pub fn encode_bits(
    bits: &[bool],
    cell_ticks: f64,
    sample_freq: u32,
    revs: usize,
    jitter: f64,
    lead_in: usize,
    rng: &mut Rng,
) -> Flux {
    let mut flux = Flux {
        sample_freq,
        ..Flux::default()
    };
    if bits.is_empty() {
        return flux;
    }
    let len = bits.len();
    let total = lead_in + revs * len + 1;
    let mut last = 0.0f64;
    let mut emitted: u64 = 0;
    for k in 0..total {
        // Bit k of the read is bit (k - lead_in) mod len of the track.
        let i = (k + revs * len - lead_in % len) % len;
        // A cell's transition at its end, so a read starting on a cell
        // boundary quantises exactly.
        let at_cell = (k as f64 + 1.0) * cell_ticks;
        if k >= lead_in && (k - lead_in).is_multiple_of(len) {
            flux.index.push((k as f64 * cell_ticks) as u64);
        }
        if bits[i] {
            let t = at_cell + rng.signed_unit() * jitter * cell_ticks;
            let t = t.max(last + 1.0);
            let tick = t.round() as u64;
            let d = tick.saturating_sub(emitted).max(1);
            flux.intervals.push(d as u32);
            emitted += d;
            last = t;
        }
    }
    flux
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(n: usize, rng: &mut Rng) -> Vec<bool> {
        // MFM-like: never two ones in a row, never more than three zeros.
        let mut v = Vec::with_capacity(n);
        let mut zeros = 1;
        while v.len() < n {
            if zeros >= 1 && (zeros == 3 || rng.next_u64() & 1 == 1) {
                v.push(true);
                zeros = 0;
            } else {
                v.push(false);
                zeros += 1;
            }
        }
        v
    }

    #[test]
    fn pack_unpack() {
        let b = unpack(&[0xa5, 0x01]);
        assert_eq!(b.len(), 16);
        assert!(b[0] && !b[1] && b[15]);
        assert_eq!(pack(&b), [0xa5, 0x01]);
        assert_eq!(pack(&[true]), [0x80]);
    }

    #[test]
    fn pll_recovers_bits_with_jitter_and_speed_error() {
        let mut rng = Rng::new(7);
        let bits = pattern(20_000, &mut rng);
        // 72 MHz samples, 2 µs cells, the drive 3 % fast: the decoder is
        // told the nominal cell.
        let nominal = 144.0;
        let flux = encode_bits(&bits, nominal * 0.97, 72_000_000, 2, 0.12, 333, &mut rng);
        // An index pulse at the start and at the end of each revolution.
        assert_eq!(flux.index.len(), 3);
        let out = decode_bits(&flux, nominal);
        assert_eq!(out.index.len(), 3);
        let rev = out.revolution();
        // Same length and contents, maybe rotated by a cell at the index.
        assert!((rev.len() as i64 - bits.len() as i64).abs() <= 1);
        let start = out.index[0];
        let errors = |from: usize| {
            bits.iter()
                .zip(&out.bits[from..])
                .filter(|(a, b)| a != b)
                .count()
        };
        assert_eq!((start - 1..=start + 1).map(errors).min(), Some(0));
    }

    #[test]
    fn rpm_from_index_pulses() {
        let flux = Flux {
            sample_freq: 72_000_000,
            intervals: vec![1],
            index: vec![100, 14_400_100, 28_800_100],
        };
        assert!((flux.rpm().unwrap() - 300.0).abs() < 1e-9);
        assert_eq!(Flux::default().rpm(), None);
    }

    #[test]
    fn rng_is_deterministic() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            let v = a.signed_unit();
            assert_eq!(v, b.signed_unit());
            assert!((-1.0..1.0).contains(&v));
        }
    }
}
