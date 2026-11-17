//! Imports of common cheat formats into a [`CheatFile`].
//!
//! - `pokes`: BASIC POKE lists as printed in magazines for the C64,
//!   VIC-20, Plus/4, Atari 8-bit (`POKE 53280,0`) and the CPC (`POKE
//!   &A000,0`): statements separated by `:` or lines, optional line
//!   numbers, `REM`/`'` comments become the label. Each POKE becomes a
//!   freeze re-applied every frame (the classic trainer POKE, CLAUDE.md
//!   "Memory freezer").
//! - `vice`: VICE monitor commands (VICE manual, "Monitor"): `> [C:]addr
//!   bytes…` writes and `f start end byte…` fills become code patches;
//!   other commands are ignored.
//! - `codes`: address/value lists, one per line (`addr value`,
//!   `addr:value`, `addr=value`, hex; `$`/`0x` optional), as trainer
//!   lists and Action Replay-style codes give them: a 2/4/8-digit value
//!   freezes a byte/word/long (blocked writes).
//! - `toml`: the cheat file format itself.
//!
//! Numbers in `pokes` are decimal unless written `$hex`, `&hex` or `0xhex`.

use crate::{CheatFile, CheatFreeze, CheatPatch};

/// Import `text` in `format` (see the module docs); addresses refer to
/// memory space `space` (usually the main CPU's, `cpu`).
pub fn import(format: &str, text: &str, space: &str) -> Result<CheatFile, String> {
    let file = match format {
        "toml" => CheatFile::from_toml(text)?,
        "pokes" => pokes(text, space)?,
        "vice" => vice(text, space)?,
        "codes" => codes(text, space)?,
        other => {
            return Err(format!(
                "unknown cheat format '{other}' (toml, pokes, vice, codes)"
            ))
        }
    };
    if file.is_empty() {
        return Err(format!("no cheats found in the {format} text"));
    }
    Ok(file)
}

fn number(s: &str, hex_default: bool) -> Option<u32> {
    let s = s.trim();
    let (digits, radix) = if let Some(h) = s.strip_prefix('$').or_else(|| s.strip_prefix('&')) {
        (h, 16)
    } else if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (h, 16)
    } else if hex_default {
        (s, 16)
    } else {
        (s, 10)
    };
    u32::from_str_radix(digits, radix).ok()
}

fn freeze(
    space: &str,
    addr: u32,
    size: u8,
    value: u32,
    mode: &str,
    label: Option<String>,
) -> CheatFreeze {
    CheatFreeze {
        label,
        space: space.to_string(),
        addr,
        size,
        value,
        mode: mode.to_string(),
        enabled: true,
    }
}

/// Split off a trailing comment (`REM`, `'`, `;`, `#`); the comment text.
fn comment(line: &str, markers: &[&str]) -> (String, Option<String>) {
    let upper = line.to_ascii_uppercase();
    let at = markers
        .iter()
        .filter_map(|m| upper.find(&m.to_ascii_uppercase()).map(|i| (i, m.len())))
        .min_by_key(|&(i, _)| i);
    match at {
        Some((i, n)) => {
            let text = line[i + n..].trim().trim_start_matches(':').trim();
            (
                line[..i].to_string(),
                (!text.is_empty()).then(|| text.to_string()),
            )
        }
        None => (line.to_string(), None),
    }
}

fn pokes(text: &str, space: &str) -> Result<CheatFile, String> {
    let mut file = CheatFile::default();
    for (n, raw) in text.lines().enumerate() {
        let (code, label) = comment(raw, &["REM", "'"]);
        // A leading line number.
        let code = code
            .trim_start()
            .trim_start_matches(|c: char| c.is_ascii_digit());
        for stmt in code.split(':') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            let upper = stmt.to_ascii_uppercase();
            let Some(args) = upper.strip_prefix("POKE") else {
                continue; // RUN, SYS … are not cheats.
            };
            let mut it = args.split(',');
            let (Some(a), Some(v), None) = (it.next(), it.next(), it.next()) else {
                return Err(format!(
                    "line {}: POKE needs an address and a value: {stmt}",
                    n + 1
                ));
            };
            let addr =
                number(a, false).ok_or_else(|| format!("line {}: bad address in {stmt}", n + 1))?;
            let value = number(v, false)
                .filter(|&v| v <= 0xff)
                .ok_or_else(|| format!("line {}: bad value in {stmt}", n + 1))?;
            file.freeze.push(freeze(
                space,
                addr,
                1,
                value,
                "reapply_per_frame",
                label.clone(),
            ));
        }
    }
    Ok(file)
}

fn vice(text: &str, space: &str) -> Result<CheatFile, String> {
    let mut file = CheatFile::default();
    for (n, raw) in text.lines().enumerate() {
        let (code, label) = comment(raw, &[";"]);
        let mut words = code.split_whitespace();
        let Some(cmd) = words.next() else { continue };
        let bad = |what: &str| format!("line {}: {what}: {}", n + 1, raw.trim());
        match cmd.to_ascii_lowercase().as_str() {
            ">" => {
                let a = words.next().ok_or_else(|| bad("address missing"))?;
                let a = a.split_once(':').map_or(a, |(_, a)| a);
                let addr = number(a, true).ok_or_else(|| bad("bad address"))?;
                let bytes = words
                    .map(|w| number(w, true).filter(|&b| b <= 0xff).map(|b| b as u8))
                    .collect::<Option<Vec<u8>>>()
                    .ok_or_else(|| bad("bad byte"))?;
                if bytes.is_empty() {
                    return Err(bad("no bytes"));
                }
                file.patch.push(CheatPatch {
                    label,
                    space: space.to_string(),
                    addr,
                    bytes,
                    enabled: true,
                });
            }
            "f" | "fill" => {
                let addr = |w: Option<&str>| {
                    w.map(|a| a.split_once(':').map_or(a, |(_, a)| a))
                        .and_then(|a| number(a, true))
                        .ok_or_else(|| bad("bad address"))
                };
                let start = addr(words.next())?;
                let end = addr(words.next())?;
                let pattern = words
                    .map(|w| number(w, true).filter(|&b| b <= 0xff).map(|b| b as u8))
                    .collect::<Option<Vec<u8>>>()
                    .ok_or_else(|| bad("bad byte"))?;
                if pattern.is_empty() || end < start || end - start > 0xffff {
                    return Err(bad("bad fill"));
                }
                let bytes = (0..=end - start)
                    .map(|i| pattern[i as usize % pattern.len()])
                    .collect();
                file.patch.push(CheatPatch {
                    label,
                    space: space.to_string(),
                    addr: start,
                    bytes,
                    enabled: true,
                });
            }
            _ => {}
        }
    }
    Ok(file)
}

fn codes(text: &str, space: &str) -> Result<CheatFile, String> {
    let mut file = CheatFile::default();
    for (n, raw) in text.lines().enumerate() {
        let (code, label) = comment(raw, &[";", "#", "//"]);
        let code = code.replace([':', '='], " ");
        let mut words = code.split_whitespace();
        let (Some(a), Some(v)) = (words.next(), words.next()) else {
            if code.trim().is_empty() {
                continue;
            }
            return Err(format!(
                "line {}: expected address and value: {}",
                n + 1,
                raw.trim()
            ));
        };
        let bad = || format!("line {}: bad number: {}", n + 1, raw.trim());
        let addr = number(a, true).ok_or_else(bad)?;
        let digits = v.trim_start_matches('$').trim_start_matches("0x").len();
        let size = match digits {
            0..=2 => 1,
            3 | 4 => 2,
            5..=8 => 4,
            _ => return Err(bad()),
        };
        let value = number(v, true).ok_or_else(bad)?;
        file.freeze
            .push(freeze(space, addr, size, value, "block_writes", label));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poke_lists() {
        let f = import(
            "pokes",
            "10 POKE 2048,0: POKE $D020,1 : REM infinite lives\nPOKE &A000,255 ' CPC energy\nRUN\n",
            "cpu",
        )
        .unwrap();
        let got: Vec<(u32, u32)> = f.freeze.iter().map(|c| (c.addr, c.value)).collect();
        assert_eq!(got, [(2048, 0), (0xd020, 1), (0xa000, 255)]);
        assert_eq!(f.freeze[0].label.as_deref(), Some("infinite lives"));
        assert_eq!(f.freeze[2].label.as_deref(), Some("CPC energy"));
        assert_eq!(f.freeze[0].mode, "reapply_per_frame");
        assert!(import("pokes", "POKE 1,300", "cpu").is_err(), "value > 255");
        assert!(import("pokes", "POKE 1", "cpu").is_err());
        assert!(import("pokes", "PRINT 1", "cpu").is_err(), "no cheats");
    }

    #[test]
    fn vice_monitor_commands() {
        let f = import(
            "vice",
            "> C:0810 ea ea ea ; no death\nf 1000 1003 a9 00\nm 0800 0810\n",
            "cpu",
        )
        .unwrap();
        assert_eq!(f.patch[0].addr, 0x0810);
        assert_eq!(f.patch[0].bytes, [0xea; 3]);
        assert_eq!(f.patch[0].label.as_deref(), Some("no death"));
        assert_eq!(f.patch[1].bytes, [0xa9, 0, 0xa9, 0]);
        assert!(import("vice", "> 0810 zz", "cpu").is_err());
    }

    #[test]
    fn address_value_codes() {
        let f = import(
            "codes",
            "C0D123 05 # lives\n$2000:1234\n0x4000=DEADBEEF\n\n",
            "chip",
        )
        .unwrap();
        let got: Vec<(u32, u8, u32)> = f.freeze.iter().map(|c| (c.addr, c.size, c.value)).collect();
        assert_eq!(
            got,
            [
                (0xc0d123, 1, 5),
                (0x2000, 2, 0x1234),
                (0x4000, 4, 0xdead_beef)
            ]
        );
        assert_eq!(f.freeze[0].space, "chip");
        assert_eq!(f.freeze[0].label.as_deref(), Some("lives"));
        assert!(import("codes", "1234", "cpu").is_err());
        assert!(import("nope", "", "cpu").is_err());
    }
}
