//! Disk formats for flux imports: which tracks to read, how to decode
//! them, and the image built from the sectors found.
//!
//! - Commodore 1541 (D64 or G64) and 1571 (D71): 5.25" GCR, 48 tpi.
//!   Track `t` lies at cylinder `t - 1` of a 40-track (48 tpi) drive and
//!   at cylinder `2 (t - 1)` of an 80-track (96 tpi) one ([`Step`]); the
//!   1571's tracks 36–70 are the second side (head 1) of cylinders 0–34.
//! - Commodore 1581 (D81): 3.5" IBM MFM, 80 cylinders, 2 sides, ten
//!   512-byte sectors; the sides come from the ID fields.
//! - Amiga (ADF): 80 cylinders × 2 heads, track `2 c + h`.
//! - IBM (raw sector image, PC .img/.ima, Atari ST .st): 40 or 80
//!   cylinders, sectors placed by physical cylinder/head and the sector
//!   number of their ID field; the sectors per track and their size come
//!   from the disk.
//!
//! Sources: Commodore 1541/1571/1581 user's guides; "Inside the 1541";
//! the ADF FAQ (Laurent Clévy); IBM diskette format as in the WD1772 and
//! µPD765 data sheets.

use std::collections::BTreeMap;

use emu_core::media::MediaError;
use media::d64::{sectors_per_track, speed_zone, SECTOR_SIZE};
use media::g64::G64;
use media::{adf, mfm, D64};

use super::{gcr, mfm as fmfm, Flux};

/// A disk format to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskFormat {
    /// 1541: 35, 40 or 42 tracks.
    Cbm1541 {
        tracks: u8,
    },
    /// 1571: 70 tracks on two sides.
    Cbm1571,
    Cbm1581,
    Amiga {
        hd: bool,
    },
    /// IBM MFM: `hd` None = try double density first, then high.
    Ibm {
        cylinders: u8,
        heads: u8,
        hd: Option<bool>,
    },
}

/// What to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    D64,
    G64,
    D71,
    D81,
    Adf,
    /// Raw sectors in cylinder/head/sector order.
    Raw,
}

/// Head stepping of the reading drive: cylinders per track of the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// One cylinder per track (drive and disk of the same track pitch).
    Single,
    /// Two (a 48 tpi disk in an 80-track 96 tpi drive).
    Double,
    /// Find out from the disk (48 tpi formats only).
    Auto,
}

impl DiskFormat {
    /// The format and output for an image file name, `hint` from the
    /// command line (`1541`, `1571`, `1581`, `amiga`, `amiga-hd`, `ibm`,
    /// `ibm-dd`, `ibm-hd`, `ibm360`).
    pub fn for_name(name: &str, hint: Option<&str>) -> Result<(DiskFormat, Output), MediaError> {
        let ext = std::path::Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let output = match ext.as_str() {
            "d64" => Output::D64,
            "g64" => Output::G64,
            "d71" => Output::D71,
            "d81" => Output::D81,
            "adf" => Output::Adf,
            "img" | "ima" | "st" | "raw" => Output::Raw,
            _ => {
                return Err(MediaError::Format(format!(
                    "{name}: write .d64, .g64, .d71, .d81, .adf or .img/.st"
                )))
            }
        };
        let format = match (hint, output) {
            (Some("1571"), _) | (None, Output::D71) => DiskFormat::Cbm1571,
            (Some("1581"), _) | (None, Output::D81) => DiskFormat::Cbm1581,
            (Some("1541"), _) | (None, Output::D64 | Output::G64) => {
                DiskFormat::Cbm1541 { tracks: 35 }
            }
            (Some("amiga"), _) | (None, Output::Adf) => DiskFormat::Amiga { hd: false },
            (Some("amiga-hd"), _) => DiskFormat::Amiga { hd: true },
            (Some(h @ ("ibm" | "ibm-dd" | "ibm-hd" | "ibm360")), _) | (Some(h), Output::Raw)
                if h.starts_with("ibm") =>
            {
                DiskFormat::Ibm {
                    cylinders: if h == "ibm360" { 40 } else { 80 },
                    heads: 2,
                    hd: match h {
                        "ibm-dd" | "ibm360" => Some(false),
                        "ibm-hd" => Some(true),
                        _ => None,
                    },
                }
            }
            (None, Output::Raw) => DiskFormat::Ibm {
                cylinders: 80,
                heads: 2,
                hd: None,
            },
            (Some(h), _) => {
                return Err(MediaError::Format(format!(
                    "unknown disk format '{h}' (1541, 1571, 1581, amiga, amiga-hd, ibm, ibm-dd, ibm-hd, ibm360)"
                )))
            }
        };
        let fits = matches!(
            (format, output),
            (DiskFormat::Cbm1541 { .. }, Output::D64 | Output::G64)
                | (DiskFormat::Cbm1571, Output::D71)
                | (DiskFormat::Cbm1581, Output::D81 | Output::Raw)
                | (DiskFormat::Amiga { .. }, Output::Adf | Output::Raw)
                | (DiskFormat::Ibm { .. }, Output::Raw)
        );
        if !fits {
            return Err(MediaError::Format(format!(
                "{name}: format {format:?} cannot be written as {output:?}"
            )));
        }
        Ok((format, output))
    }

    /// With a different track count (1541: 35/40/42; IBM: cylinders).
    pub fn with_tracks(self, n: u8) -> Self {
        match self {
            DiskFormat::Cbm1541 { .. } => DiskFormat::Cbm1541 { tracks: n },
            DiskFormat::Ibm { heads, hd, .. } => DiskFormat::Ibm {
                cylinders: n,
                heads,
                hd,
            },
            other => other,
        }
    }

    /// Disks of 48 tpi (5.25" 40-track): may need double steps.
    pub fn is_48tpi(self) -> bool {
        match self {
            DiskFormat::Cbm1541 { .. } | DiskFormat::Cbm1571 => true,
            DiskFormat::Ibm { cylinders, .. } => cylinders <= 42,
            _ => false,
        }
    }

    /// (cylinder of the disk, head) of every track to read, in order.
    pub fn tracks(self) -> Vec<(u8, u8)> {
        match self {
            DiskFormat::Cbm1541 { tracks } => (0..tracks).map(|c| (c, 0)).collect(),
            DiskFormat::Cbm1571 => (0..2).flat_map(|h| (0..35).map(move |c| (c, h))).collect(),
            DiskFormat::Cbm1581 | DiskFormat::Amiga { .. } => {
                (0..80).flat_map(|c| (0..2).map(move |h| (c, h))).collect()
            }
            DiskFormat::Ibm {
                cylinders, heads, ..
            } => (0..cylinders)
                .flat_map(|c| (0..heads).map(move |h| (c, h)))
                .collect(),
        }
    }

    /// The Commodore track number at a cylinder/head (GCR formats).
    fn cbm_track(self, cyl: u8, head: u8) -> u8 {
        cyl + 1 + if head == 1 { 35 } else { 0 }
    }

    /// Sectors expected on a track (0 = unknown).
    pub fn expected(self, cyl: u8, _head: u8) -> usize {
        match self {
            DiskFormat::Cbm1541 { .. } | DiskFormat::Cbm1571 => {
                usize::from(sectors_per_track(cyl + 1))
            }
            DiskFormat::Cbm1581 => 10,
            DiskFormat::Amiga { hd } => adf::SECTORS * if hd { 2 } else { 1 },
            DiskFormat::Ibm { .. } => 0,
        }
    }
}

/// Sectors of one track, by number.
type TrackSectors = BTreeMap<u8, Vec<u8>>;

/// The sectors read so far, merged over retries.
pub struct ImageBuilder {
    pub format: DiskFormat,
    tracks: BTreeMap<(u8, u8), TrackSectors>,
    /// GCR: headers seen per track (for the D64 error codes).
    headers: BTreeMap<(u8, u8), Vec<u8>>,
    /// GCR: the revolution of the best read per track (for G64).
    revolutions: BTreeMap<(u8, u8), (usize, Vec<u8>)>,
    /// IBM: density found on the first track.
    ibm_hd: Option<bool>,
}

/// What one read of a track gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackReport {
    pub cylinder: u8,
    pub head: u8,
    /// Good sectors of this track so far (all reads).
    pub good: usize,
    /// Expected sectors (0 = unknown, IBM).
    pub expected: usize,
}

impl TrackReport {
    pub fn complete(&self) -> bool {
        self.good >= self.expected && self.good > 0
    }
}

impl ImageBuilder {
    pub fn new(format: DiskFormat) -> Self {
        Self {
            format,
            tracks: BTreeMap::new(),
            headers: BTreeMap::new(),
            revolutions: BTreeMap::new(),
            ibm_hd: match format {
                DiskFormat::Ibm { hd, .. } => hd,
                _ => None,
            },
        }
    }

    /// Decode one read of disk cylinder `cyl`, head `head`.
    pub fn add(&mut self, cyl: u8, head: u8, flux: &Flux) -> TrackReport {
        let key = (cyl, head);
        let found: Vec<(u8, Vec<u8>)> = match self.format {
            DiskFormat::Cbm1541 { .. } | DiskFormat::Cbm1571 => {
                let track = self.format.cbm_track(cyl, head);
                let t = gcr::decode(flux, speed_zone(cyl + 1));
                let found: Vec<(u8, Vec<u8>)> = t
                    .sectors
                    .iter()
                    .filter(|s| s.track == track)
                    .map(|s| (s.sector, s.data.to_vec()))
                    .collect();
                let seen = self.headers.entry(key).or_default();
                for &(tr, s) in &t.headers {
                    if tr == track && !seen.contains(&s) {
                        seen.push(s);
                    }
                }
                let best = self.revolutions.get(&key).map_or(0, |(n, _)| *n);
                if found.len() >= best || !self.revolutions.contains_key(&key) {
                    self.revolutions.insert(key, (found.len(), t.revolution));
                }
                found
            }
            DiskFormat::Cbm1581 => fmfm::decode_ibm(flux, fmfm::DD_CELL)
                .into_iter()
                .filter(|s| s.track == cyl && s.data.len() == 512)
                // Key by side and sector: the 1581's sides come from the
                // ID fields, whichever head read them.
                .map(|s| (s.side * 16 + s.sector, s.data))
                .collect(),
            DiskFormat::Amiga { hd } => {
                let cell = if hd { fmfm::HD_CELL } else { fmfm::DD_CELL };
                let track = usize::from(cyl) * 2 + usize::from(head);
                fmfm::decode_amiga(flux, cell)
                    .into_iter()
                    .filter(|s| usize::from(s.track) == track)
                    .map(|s| (s.sector, s.data))
                    .collect()
            }
            DiskFormat::Ibm { .. } => {
                let mut sectors = Vec::new();
                let tries: &[bool] = match self.ibm_hd {
                    Some(true) => &[true],
                    Some(false) => &[false],
                    None => &[false, true],
                };
                for &hd in tries {
                    let cell = if hd { fmfm::HD_CELL } else { fmfm::DD_CELL };
                    sectors = fmfm::decode_ibm(flux, cell);
                    if !sectors.is_empty() {
                        self.ibm_hd = Some(hd);
                        break;
                    }
                }
                sectors.into_iter().map(|s| (s.sector, s.data)).collect()
            }
        };
        let good = if self.format == DiskFormat::Cbm1581 {
            // Both sides under one key, the side in the number; count the
            // sides this read delivered.
            let sides: Vec<u8> = found.iter().map(|(n, _)| n / 16).collect();
            let entry = self.tracks.entry((cyl, 0)).or_default();
            for (n, data) in found {
                entry.entry(n).or_insert(data);
            }
            match sides.iter().max() {
                Some(&side) => entry.keys().filter(|&&n| n / 16 == side).count(),
                None => 0,
            }
        } else {
            let entry = self.tracks.entry(key).or_default();
            for (n, data) in found {
                entry.entry(n).or_insert(data);
            }
            entry.len()
        };
        TrackReport {
            cylinder: cyl,
            head,
            good,
            expected: self.format.expected(cyl, head),
        }
    }

    /// Sectors still missing: (cylinder, head, how many); 1581: (cylinder,
    /// side, how many).
    pub fn missing(&self) -> Vec<(u8, u8, usize)> {
        self.format
            .tracks()
            .into_iter()
            .filter_map(|(c, h)| {
                let expected = self.format.expected(c, h);
                let good = match self.format {
                    DiskFormat::Cbm1581 => self
                        .tracks
                        .get(&(c, 0))
                        .map_or(0, |t| t.keys().filter(|&&n| n / 16 == h).count()),
                    _ => self.tracks.get(&(c, h)).map_or(0, BTreeMap::len),
                };
                (good < expected || (expected == 0 && good == 0)).then(|| {
                    (
                        c,
                        h,
                        expected.saturating_sub(good).max(usize::from(good == 0)),
                    )
                })
            })
            .collect()
    }

    /// The image.
    pub fn finish(&self, output: Output) -> Result<Vec<u8>, MediaError> {
        match (self.format, output) {
            (DiskFormat::Cbm1541 { tracks }, Output::D64) => Ok(self.cbm_image(tracks, false)),
            (DiskFormat::Cbm1571, Output::D71) => Ok(self.cbm_image(70, true)),
            (DiskFormat::Cbm1541 { .. }, Output::G64) => Ok(self.g64()),
            (DiskFormat::Cbm1581, Output::D81 | Output::Raw) => {
                let mut d = D64::parse(&vec![0; 80 * 40 * SECTOR_SIZE])?;
                let sectors: Vec<mfm::Sector> = self
                    .tracks
                    .iter()
                    .flat_map(|(&(c, _), t)| {
                        t.iter().map(move |(&n, data)| mfm::Sector {
                            track: c,
                            side: n / 16,
                            sector: n % 16,
                            data: data.clone(),
                        })
                    })
                    .collect();
                mfm::store_sectors(&mut d, &sectors);
                Ok(d.to_bytes())
            }
            (DiskFormat::Amiga { hd }, Output::Adf | Output::Raw) => {
                let mut img = vec![0; if hd { adf::ADF_HD_SIZE } else { adf::ADF_SIZE }];
                for (&(c, h), t) in &self.tracks {
                    let track = usize::from(c) * 2 + usize::from(h);
                    let sectors: Vec<adf::Sector> = t
                        .iter()
                        .map(|(&n, data)| adf::Sector {
                            track: track as u8,
                            sector: n,
                            data: data.clone(),
                        })
                        .collect();
                    adf::store_track(&mut img, track, &sectors);
                }
                Ok(img)
            }
            (
                DiskFormat::Ibm {
                    cylinders, heads, ..
                },
                Output::Raw,
            ) => {
                let spt = self
                    .tracks
                    .values()
                    .flat_map(|t| t.keys())
                    .copied()
                    .max()
                    .unwrap_or(0);
                let size = self
                    .tracks
                    .values()
                    .flat_map(|t| t.values())
                    .map(Vec::len)
                    .max()
                    .unwrap_or(512);
                if spt == 0 {
                    return Err(MediaError::Format("no IBM MFM sectors found".into()));
                }
                let (spt, heads) = (usize::from(spt), usize::from(heads));
                let mut img = vec![0; usize::from(cylinders) * heads * spt * size];
                for (&(c, h), t) in &self.tracks {
                    for (&r, data) in t {
                        if r == 0 || data.len() != size {
                            continue;
                        }
                        let at = ((usize::from(c) * heads + usize::from(h)) * spt + usize::from(r)
                            - 1)
                            * size;
                        if let Some(dst) = img.get_mut(at..at + size) {
                            dst.copy_from_slice(data);
                        }
                    }
                }
                Ok(img)
            }
            (f, o) => Err(MediaError::Format(format!(
                "format {f:?} cannot be written as {o:?}"
            ))),
        }
    }

    /// D64/D71, with an error byte per sector when sectors are missing
    /// (20 = header not found → 2, 23 = data checksum → 5, as the 1541
    /// would report them).
    fn cbm_image(&self, tracks: u8, double_sided: bool) -> Vec<u8> {
        let mut data = Vec::new();
        let mut errors = Vec::new();
        let mut any_error = false;
        let layout: Vec<(u8, u8)> = if double_sided {
            (0..2).flat_map(|h| (0..35).map(move |c| (c, h))).collect()
        } else {
            (0..tracks).map(|c| (c, 0)).collect()
        };
        for (c, h) in layout {
            let t = self.tracks.get(&(c, h));
            let seen = self.headers.get(&(c, h));
            for s in 0..sectors_per_track(c + 1) {
                match t.and_then(|t| t.get(&s)) {
                    Some(d) => {
                        data.extend_from_slice(d);
                        errors.push(1);
                    }
                    None => {
                        data.extend_from_slice(&[0; SECTOR_SIZE]);
                        any_error = true;
                        errors.push(if seen.is_some_and(|v| v.contains(&s)) {
                            5
                        } else {
                            2
                        });
                    }
                }
            }
        }
        if any_error {
            data.extend_from_slice(&errors);
        }
        data
    }

    fn g64(&self) -> Vec<u8> {
        let mut g = G64::new();
        for (&(c, _), (_, rev)) in &self.revolutions {
            g.set_track(c + 1, rev.clone(), speed_zone(c + 1));
        }
        g.to_bytes()
    }
}

/// Where flux comes from: a Greaseweazle, or a test double.
pub trait FluxSource {
    /// Read `revs` revolutions at physical cylinder `cyl`, head `head`.
    fn read_flux(&mut self, cyl: u8, head: u8, revs: u8) -> Result<Flux, MediaError>;
}

/// Import settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportOptions {
    pub step: Step,
    pub revs: u8,
    /// Extra reads of a track with missing sectors.
    pub retries: u8,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            step: Step::Auto,
            revs: 2,
            retries: 3,
        }
    }
}

/// Result of an import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    pub image: Vec<u8>,
    /// Tracks still missing sectors: (cylinder, head, how many).
    pub missing: Vec<(u8, u8, usize)>,
    pub step: Step,
}

/// Decide the head step of a 48 tpi disk: at cylinder 1 a 40-track drive
/// is on the disk's second track, an 80-track drive still on its first.
fn detect_step(
    source: &mut dyn FluxSource,
    format: DiskFormat,
    revs: u8,
) -> Result<Step, MediaError> {
    let flux = source.read_flux(1, 0, revs)?;
    let id = match format {
        DiskFormat::Cbm1541 { .. } | DiskFormat::Cbm1571 => {
            // Zone 3 and 2 tracks differ little in speed; the PLL locks
            // on either.
            gcr::decode(&flux, 3).track_id().map(|t| t - 1)
        }
        _ => fmfm::decode_ibm(&flux, fmfm::DD_CELL)
            .first()
            .map(|s| s.track),
    };
    Ok(match id {
        Some(1) => Step::Single,
        Some(0) => Step::Double,
        _ => Step::Single,
    })
}

/// Read a whole disk. `progress` hears about every track read.
pub fn import(
    source: &mut dyn FluxSource,
    format: DiskFormat,
    output: Output,
    options: ImportOptions,
    progress: &mut dyn FnMut(&TrackReport),
) -> Result<Imported, MediaError> {
    let step = match options.step {
        Step::Auto if format.is_48tpi() => detect_step(source, format, options.revs)?,
        Step::Auto => Step::Single,
        s => s,
    };
    let factor = if step == Step::Double { 2 } else { 1 };
    let mut builder = ImageBuilder::new(format);
    for (cyl, head) in format.tracks() {
        let phys = cyl
            .checked_mul(factor)
            .ok_or_else(|| MediaError::Format("cylinder out of range".into()))?;
        for attempt in 0..=options.retries {
            let flux = source.read_flux(phys, head, options.revs)?;
            let report = builder.add(cyl, head, &flux);
            progress(&report);
            if report.complete() || (report.expected == 0 && attempt > 0) {
                break;
            }
        }
    }
    Ok(Imported {
        image: builder.finish(output)?,
        missing: builder.missing(),
        step,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{encode_bits, unpack, Rng};
    use super::*;

    /// Flux from images, as a drive would read it.
    struct FakeDrive {
        rng: Rng,
        /// Track bits and cell length (ticks) by physical (cyl, head).
        tracks: BTreeMap<(u8, u8), (Vec<bool>, f64)>,
        reads: usize,
        /// Reads of this (cyl, head) come back damaged the first time.
        flaky: Option<(u8, u8)>,
    }

    impl FluxSource for FakeDrive {
        fn read_flux(&mut self, cyl: u8, head: u8, revs: u8) -> Result<Flux, MediaError> {
            self.reads += 1;
            let Some((bits, cell)) = self.tracks.get(&(cyl, head)) else {
                // Unformatted: noise-free silence.
                return Ok(Flux {
                    sample_freq: 72_000_000,
                    intervals: vec![14_400_000],
                    index: vec![0, 14_400_000],
                });
            };
            let mut bits = bits.clone();
            if self.flaky == Some((cyl, head)) {
                self.flaky = None;
                let mid = bits.len() / 2;
                for b in &mut bits[mid..mid + 4000] {
                    *b = false;
                }
            }
            let lead = (self.rng.next_u64() % 5000) as usize;
            Ok(encode_bits(
                &bits,
                *cell,
                72_000_000,
                usize::from(revs),
                0.1,
                lead,
                &mut self.rng,
            ))
        }
    }

    fn test_d64() -> D64 {
        let mut d = D64::blank(b"FLUX TEST", *b"FT");
        let data: Vec<u8> = (0..20_000u32).map(|i| (i * 31 % 251) as u8).collect();
        d.write_file(b"DATA", 2, &data).unwrap();
        d
    }

    fn cbm_drive(d: &D64, step: u8) -> FakeDrive {
        let mut tracks = BTreeMap::new();
        for t in 1..=35u8 {
            let bits = unpack(&media::gcr::encode_track(d, t));
            let cell = gcr::cell_seconds(speed_zone(t)) * 72e6;
            for k in 0..step {
                // An 80-track drive reads the 48 tpi track at both of its
                // cylinders.
                tracks.insert(((t - 1) * step + k, 0), (bits.clone(), cell));
            }
        }
        FakeDrive {
            rng: Rng::new(11),
            tracks,
            reads: 0,
            flaky: None,
        }
    }

    #[test]
    fn names_pick_formats() {
        assert_eq!(
            DiskFormat::for_name("x.d64", None).unwrap(),
            (DiskFormat::Cbm1541 { tracks: 35 }, Output::D64)
        );
        assert_eq!(
            DiskFormat::for_name("x.ADF", None).unwrap(),
            (DiskFormat::Amiga { hd: false }, Output::Adf)
        );
        assert_eq!(
            DiskFormat::for_name("x.st", Some("ibm-dd")).unwrap().0,
            DiskFormat::Ibm {
                cylinders: 80,
                heads: 2,
                hd: Some(false)
            }
        );
        assert!(DiskFormat::for_name("x.adf", Some("1541")).is_err());
        assert!(DiskFormat::for_name("x.zip", None).is_err());
        assert!(DiskFormat::for_name("x.d64", Some("c64")).is_err());
    }

    #[test]
    fn d64_from_flux_with_a_40_and_an_80_track_drive() {
        let d = test_d64();
        for step in [1, 2] {
            let mut drive = cbm_drive(&d, step);
            let mut reports = 0;
            let out = import(
                &mut drive,
                DiskFormat::Cbm1541 { tracks: 35 },
                Output::D64,
                ImportOptions::default(),
                &mut |_| reports += 1,
            )
            .unwrap();
            assert_eq!(
                out.step,
                if step == 1 {
                    Step::Single
                } else {
                    Step::Double
                }
            );
            assert!(out.missing.is_empty(), "{:?}", out.missing);
            assert_eq!(out.image, d.to_bytes(), "step {step}");
            assert_eq!(reports, 35);
        }
    }

    #[test]
    fn retries_fill_a_damaged_read_and_bad_sectors_get_error_bytes() {
        let d = test_d64();
        let mut drive = cbm_drive(&d, 1);
        drive.flaky = Some((17, 0));
        let out = import(
            &mut drive,
            DiskFormat::Cbm1541 { tracks: 35 },
            Output::D64,
            ImportOptions {
                step: Step::Single,
                ..ImportOptions::default()
            },
            &mut |_| {},
        )
        .unwrap();
        assert!(out.missing.is_empty());
        assert_eq!(out.image, d.to_bytes());
        assert_eq!(drive.reads, 36);

        // Track 35 unformatted: its sectors get error code 2 (20).
        let mut drive = cbm_drive(&d, 1);
        drive.tracks.remove(&(34, 0));
        let out = import(
            &mut drive,
            DiskFormat::Cbm1541 { tracks: 35 },
            Output::D64,
            ImportOptions {
                step: Step::Single,
                retries: 1,
                ..ImportOptions::default()
            },
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(out.missing, [(34, 0, 17)]);
        let back = D64::parse(&out.image).unwrap();
        assert_eq!(back.error(35, 0), 2);
        assert_eq!(back.error(34, 0), 1);
        assert_eq!(back.sector(18, 0), d.sector(18, 0));
    }

    #[test]
    fn g64_from_flux() {
        let d = test_d64();
        let mut drive = cbm_drive(&d, 1);
        let out = import(
            &mut drive,
            DiskFormat::Cbm1541 { tracks: 35 },
            Output::G64,
            ImportOptions::default(),
            &mut |_| {},
        )
        .unwrap();
        let g = G64::parse(&out.image).unwrap();
        for t in 1..=35u8 {
            let tr = g.track(t).unwrap();
            assert_eq!(tr.speed, speed_zone(t));
            let sectors = media::gcr::decode_track(&tr.data);
            assert_eq!(
                sectors.len(),
                usize::from(sectors_per_track(t)),
                "track {t}"
            );
            for s in sectors {
                assert_eq!(&s.data[..], d.sector(t, s.sector).unwrap());
            }
        }
        assert!(g.track(36).is_none());
    }

    #[test]
    fn adf_and_d81_from_flux() {
        let img: Vec<u8> = (0..adf::ADF_SIZE)
            .map(|i| (i * 13 / 7) as u8 ^ (i / 512) as u8)
            .collect();
        let mut tracks = BTreeMap::new();
        for t in 0..160 {
            let bits = unpack(&adf::encode_track(&img, t));
            tracks.insert(((t / 2) as u8, (t % 2) as u8), (bits, 144.0));
        }
        let mut drive = FakeDrive {
            rng: Rng::new(5),
            tracks,
            reads: 0,
            flaky: None,
        };
        let out = import(
            &mut drive,
            DiskFormat::Amiga { hd: false },
            Output::Adf,
            ImportOptions::default(),
            &mut |_| {},
        )
        .unwrap();
        assert!(out.missing.is_empty());
        assert!(out.image == img);

        let mut d81 = D64::blank_d81(b"D81", *b"81");
        let data: Vec<u8> = (0..60_000u32).map(|i| (i % 253) as u8).collect();
        d81.write_file(b"BIG", 2, &data).unwrap();
        let mut tracks = BTreeMap::new();
        for c in 0..80u8 {
            for side in 0..2u8 {
                let bits = fmfm::ibm_encode(&mfm::encode_track(&d81, c, side));
                // The 1581 reads side 0 with head 1 and side 1 with
                // head 0: the builder sorts by the ID fields.
                tracks.insert((c, 1 - side), (bits, 144.0));
            }
        }
        let mut drive = FakeDrive {
            rng: Rng::new(6),
            tracks,
            reads: 0,
            flaky: None,
        };
        let out = import(
            &mut drive,
            DiskFormat::Cbm1581,
            Output::D81,
            ImportOptions::default(),
            &mut |_| {},
        )
        .unwrap();
        assert!(out.missing.is_empty(), "{:?}", out.missing);
        assert!(out.image == d81.to_bytes());
    }

    #[test]
    fn ibm_720k_from_flux() {
        // 9 sectors of 512 bytes per track, 80 cylinders, 2 heads.
        let mut tracks = BTreeMap::new();
        let mut img = Vec::new();
        for c in 0..80u8 {
            for h in 0..2u8 {
                let sectors: Vec<[u8; 512]> = (0..9u8)
                    .map(|r| [c ^ h.wrapping_mul(0x40) ^ r.wrapping_mul(3); 512])
                    .collect();
                for s in &sectors {
                    img.extend_from_slice(s);
                }
                // format_track numbers sectors 1… with the side byte.
                let stream = mfm::format_track(c, h, &sectors);
                tracks.insert((c, h), (fmfm::ibm_encode(&stream), 144.0));
            }
        }
        let mut drive = FakeDrive {
            rng: Rng::new(8),
            tracks,
            reads: 0,
            flaky: None,
        };
        let out = import(
            &mut drive,
            DiskFormat::Ibm {
                cylinders: 80,
                heads: 2,
                hd: None,
            },
            Output::Raw,
            ImportOptions::default(),
            &mut |_| {},
        )
        .unwrap();
        assert!(out.missing.is_empty());
        assert!(out.image == img);
    }
}
