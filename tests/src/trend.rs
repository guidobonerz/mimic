//! The trend of a test suite against its previous run.
//!
//! Each suite runner hands over its results (test name, passed) after a
//! run. The last known state of every test is kept in
//! `<tmp>/trend/<suite>.txt` and each run adds a line to
//! `<tmp>/trend/<suite>-history.tsv` (time, passed, run). The returned text
//! says how the tests that ran both times changed: passed before and now,
//! newly passing and newly failing tests. A filtered run updates only the
//! tests it ran; the others keep their last state.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// How many names of newly passing/failing tests the text lists.
const LIST: usize = 20;

/// Record `results` for `suite` under `tmp` and describe the change since
/// the previous run.
pub fn record(tmp: &Path, suite: &str, results: &[(String, bool)]) -> String {
    let dir = tmp.join("trend");
    let _ = std::fs::create_dir_all(&dir);
    let state_path = dir.join(format!("{suite}.txt"));
    let mut state: BTreeMap<String, bool> = std::fs::read_to_string(&state_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (verdict, name) = l.split_once(' ')?;
            Some((name.to_string(), verdict == "PASS"))
        })
        .collect();
    let first = state.is_empty();

    let (mut before, mut now, mut compared) = (0, 0, 0);
    let (mut fixed, mut broken) = (Vec::new(), Vec::new());
    for (name, pass) in results {
        if let Some(&was) = state.get(name) {
            compared += 1;
            before += usize::from(was);
            now += usize::from(*pass);
            match (was, *pass) {
                (false, true) => fixed.push(name.as_str()),
                (true, false) => broken.push(name.as_str()),
                _ => {}
            }
        }
    }
    let passed = results.iter().filter(|(_, p)| *p).count();
    let mut text = String::new();
    if first {
        let _ = write!(text, "Trend ({suite}): first run, nothing to compare");
    } else {
        let delta = now as i64 - before as i64;
        let _ = write!(
            text,
            "Trend ({suite}): {before} -> {now} passed ({delta:+}) of {compared} tests run both times; \
             newly passing {}, newly failing {}",
            fixed.len(),
            broken.len()
        );
        if results.len() > compared {
            let _ = write!(text, "; {} new tests", results.len() - compared);
        }
        for (label, names) in [("newly passing", &fixed), ("newly failing", &broken)] {
            if names.is_empty() {
                continue;
            }
            let _ = write!(text, "\n  {label}:");
            for name in names.iter().take(LIST) {
                let _ = write!(text, "\n    {name}");
            }
            if names.len() > LIST {
                let _ = write!(text, "\n    … {} more", names.len() - LIST);
            }
        }
    }

    for (name, pass) in results {
        state.insert(name.clone(), *pass);
    }
    let mut out = String::new();
    for (name, pass) in &state {
        let _ = writeln!(out, "{} {name}", if *pass { "PASS" } else { "FAIL" });
    }
    let _ = std::fs::write(&state_path, out);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let history = dir.join(format!("{suite}-history.tsv"));
    let mut line = format!("{secs}\t{passed}\t{}\n", results.len());
    if let Ok(old) = std::fs::read_to_string(&history) {
        line = old + &line;
    } else {
        line = format!("unix_time\tpassed\trun\n{line}");
    }
    let _ = std::fs::write(history, line);
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results(r: &[(&str, bool)]) -> Vec<(String, bool)> {
        r.iter().map(|(n, p)| (n.to_string(), *p)).collect()
    }

    #[test]
    fn reports_changes_against_the_previous_run() {
        let tmp = std::env::temp_dir().join(format!("emu-trend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let first = record(
            &tmp,
            "s",
            &results(&[("a", true), ("b", false), ("c", true)]),
        );
        assert!(first.contains("first run"), "{first}");
        let second = record(
            &tmp,
            "s",
            &results(&[("a", true), ("b", true), ("c", false)]),
        );
        assert!(second.contains("2 -> 2 passed (+0) of 3"), "{second}");
        assert!(second.contains("newly passing:\n    b"), "{second}");
        assert!(second.contains("newly failing:\n    c"), "{second}");
        // A filtered run compares and updates only what it ran.
        let third = record(&tmp, "s", &results(&[("c", true), ("d", false)]));
        assert!(third.contains("0 -> 1 passed (+1) of 1"), "{third}");
        assert!(third.contains("1 new tests"), "{third}");
        let state = std::fs::read_to_string(tmp.join("trend/s.txt")).unwrap();
        assert_eq!(state, "PASS a\nPASS b\nPASS c\nFAIL d\n");
        let history = std::fs::read_to_string(tmp.join("trend/s-history.tsv")).unwrap();
        assert_eq!(history.lines().count(), 4);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
