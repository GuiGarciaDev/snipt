//! How much you use each snippet, so the ones you use most come first.
//!
//! Stored next to the snippets in `usage.txt`, one line per snippet:
//! `score x 1000 <tab> last used (unix seconds) <tab> name`. Each paste adds 1 to the
//! score, and the score halves every week you don't use the snippet, so this
//! week's favourites beat last month's. Kept out of `snippets.txt` so that
//! file stays clean to edit by hand or sync with git. Safe to delete.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds for a score to halve.
const HALF_LIFE: f64 = 7.0 * 24.0 * 3600.0;

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn file_path() -> PathBuf {
    crate::snippets::file_path().with_file_name("usage.txt")
}

/// Name, score, last used. A plain list: there are only a few dozen
/// snippets, and it keeps the exe smaller than a HashMap would.
#[derive(Default)]
pub struct Usage {
    entries: Vec<(String, f64, u64)>,
}

impl Usage {
    pub fn load() -> Usage {
        std::fs::read_to_string(file_path()).map(|t| Usage::parse(&t)).unwrap_or_default()
    }

    fn parse(text: &str) -> Usage {
        let mut u = Usage::default();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let mut parts = line.splitn(3, '\t');
            if let (Some(score), Some(last), Some(name)) = (parts.next(), parts.next(), parts.next()) {
                // Integers only: parsing/printing floats would add tens of KB to the exe.
                if let (Ok(milli), Ok(last)) = (score.parse::<u64>(), last.parse::<u64>()) {
                    if !name.is_empty() {
                        u.set(name, milli as f64 / 1000.0, last);
                    }
                }
            }
        }
        u
    }

    fn set(&mut self, name: &str, score: f64, last: u64) {
        match self.entries.iter_mut().find(|e| e.0 == name) {
            Some(e) => (e.1, e.2) = (score, last),
            None => self.entries.push((name.to_string(), score, last)),
        }
    }

    fn serialize(&self) -> String {
        let mut rows: Vec<_> = self.entries.iter().collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out = String::from("# Snipt usage counts, used to put your most used snippets first. Safe to delete.\n");
        for (name, score, last) in rows {
            let milli = (score * 1000.0 + 0.5) as u64;
            out.push_str(&format!("{milli}\t{last}\t{name}\n"));
        }
        out
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = file_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("txt.tmp");
        std::fs::write(&tmp, self.serialize())?;
        std::fs::rename(&tmp, &path)
    }

    /// Current score: decays by half every week since the last use.
    pub fn score(&self, name: &str, now: u64) -> f64 {
        self.entries.iter().find(|e| e.0 == name).map_or(0.0, |&(_, score, last)| decay(score, last, now))
    }

    /// Counts one use. Also forgets snippets that have faded to nothing.
    pub fn record(&mut self, name: &str, now: u64) {
        let score = self.score(name, now) + 1.0;
        self.set(name, score, now);
        self.entries.retain(|(n, s, last)| n == name || decay(*s, *last, now) > 0.01);
    }

    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(&(_, score, last)) = self.entries.iter().find(|e| e.0 == old) {
            self.remove(old);
            self.set(new, score, last);
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.entries.retain(|e| e.0 != name);
    }
}

/// `score` halved once per HALF_LIFE since `last`. Whole half-lives are exact;
/// the remaining fraction uses a short series for 2^-f (within 0.3%), which
/// keeps the C runtime's `pow` out of the exe.
fn decay(score: f64, last: u64, now: u64) -> f64 {
    let x = now.saturating_sub(last) as f64 / HALF_LIFE;
    let whole = x as u32;
    if whole >= 60 {
        return 0.0;
    }
    let f = x - whole as f64;
    let part = 1.0 - f * (0.693_147 - f * (0.240_227 - f * 0.055_504));
    score * part / (1u64 << whole) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_decay_and_round_trip() {
        let day = 24 * 3600;
        let mut u = Usage::default();
        u.record("Often", 0);
        u.record("Often", 0);
        u.record("Once", 0);
        assert_eq!(u.score("Often", 0), 2.0);
        assert!((u.score("Often", 7 * day) - 1.0).abs() < 1e-9); // halves after a week
        assert_eq!(u.score("Never", 0), 0.0);

        // Something used once today beats something used twice two weeks ago.
        u.record("Fresh", 14 * day);
        assert!(u.score("Fresh", 14 * day) > u.score("Often", 14 * day));

        u.rename("Once", "Renamed");
        assert_eq!(u.score("Renamed", 0), 1.0);
        let back = Usage::parse(&u.serialize());
        assert_eq!(back.score("Renamed", 0), 1.0);
        assert!((back.score("Often", 7 * day) - 1.0).abs() < 1e-3);
    }
}
