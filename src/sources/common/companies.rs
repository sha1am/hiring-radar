//! Company lists read off disk, at crawl time, every time.
//!
//! Which companies to watch is the part of this tool that changes most often
//! and matters most: you hear about a place, you want it on the radar before
//! you forget. A text file is the right home for that — you edit it in any
//! editor, `git log` tells you when a name appeared, and adding thirty
//! companies is a paste rather than thirty form submissions.
//!
//! So the file is the authority, not the database, and it is re-read on every
//! crawl rather than at boot. Editing `companies/greenhouse.txt` takes effect
//! on the next cycle, with no restart and no click. The settings page shows the
//! list but will not let you edit it, because two places to change one thing is
//! how they end up disagreeing.
//!
//! Format: one entry per line. Blank lines are skipped. `#` starts a comment,
//! at the start of a line or after an entry, so a list can carry its own notes
//! about why a name is there.

use std::path::{Path, PathBuf};

/// The result of trying to read one list.
#[derive(Debug, Clone, Default)]
pub struct List {
    /// Where we looked, for the note the dashboard shows.
    pub path: PathBuf,
    pub entries: Vec<String>,
    /// The file is not there at all. Distinct from an empty file: absent means
    /// "fall back to whatever is stored in settings", empty means "the operator
    /// deliberately watches nothing". Treating those the same would resurrect a
    /// list someone had just cleared out.
    pub missing: bool,
    /// Present but unreadable — permissions, a directory where a file should
    /// be. Never silently equivalent to empty.
    pub error: Option<String>,
}

impl List {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// One line for the status panel, naming the file so the fix is obvious.
    pub fn note(&self) -> String {
        let p = self.path.display();
        match (&self.error, self.missing) {
            (Some(e), _) => format!("{p}: unreadable — {e}"),
            (None, true) => format!("{p}: not found, using the list saved in settings"),
            (None, false) if self.entries.is_empty() => {
                format!("{p}: empty — add one company per line")
            }
            (None, false) => format!("{p}: {} companies", self.entries.len()),
        }
    }
}

/// Where the lists live. Overridable so the container can mount them from a
/// volume and tests can point somewhere disposable.
pub fn dir() -> PathBuf {
    PathBuf::from(std::env::var("RADAR_COMPANIES_DIR").unwrap_or_else(|_| "companies".into()))
}

/// Read `<dir>/<name>.txt`.
pub fn load(name: &str) -> List {
    load_from(&dir().join(format!("{name}.txt")))
}

pub fn load_from(path: &Path) -> List {
    let mut list = List {
        path: path.to_path_buf(),
        ..Default::default()
    };
    match std::fs::read_to_string(path) {
        Ok(body) => list.entries = parse(&body),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => list.missing = true,
        Err(e) => list.error = Some(e.to_string()),
    }
    list
}

/// Strip comments and blanks, keep order, drop duplicates.
///
/// Order is kept because a list is usually written most-wanted first, and a
/// crawl that runs out of time should have spent it on the top of the file.
pub fn parse(body: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in body.lines() {
        let entry = line.split('#').next().unwrap_or("").trim();
        if entry.is_empty() {
            continue;
        }
        if seen.insert(entry.to_lowercase()) {
            out.push(entry.to_string());
        }
    }
    out
}

/// The list a source should actually crawl: the file when there is one, the
/// stored settings when there is not.
///
/// The fallback is deliberately narrow. A *missing* file means this install
/// predates the file, or nobody has written one yet — falling back keeps it
/// working. A file that exists and is empty is an instruction, and is obeyed.
pub fn resolve(list: &List, stored: &[String]) -> Vec<String> {
    if list.missing || list.error.is_some() {
        stored.to_vec()
    } else {
        list.entries.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_blanks_are_not_companies() {
        let body = "# backend-heavy\nramp\n\n  stripe  # pays well\n#airbnb (left)\n";
        assert_eq!(parse(body), vec!["ramp", "stripe"]);
    }

    #[test]
    fn duplicates_collapse_case_insensitively_but_order_holds() {
        assert_eq!(parse("zeta\nAlpha\nalpha\nzeta\n"), vec!["zeta", "Alpha"]);
    }

    #[test]
    fn a_missing_file_falls_back_but_an_empty_one_does_not() {
        let dir = std::env::temp_dir().join(format!("radar-companies-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stored = vec!["from-settings".to_string()];

        let absent = load_from(&dir.join("nope.txt"));
        assert!(absent.missing);
        assert_eq!(resolve(&absent, &stored), stored);

        // An operator who empties the file means it. Falling back here would
        // silently resurrect a list they had just cleared.
        let p = dir.join("empty.txt");
        std::fs::write(&p, "# everything removed\n").unwrap();
        let empty = load_from(&p);
        assert!(!empty.missing);
        assert!(resolve(&empty, &stored).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_note_always_names_the_file() {
        let dir = std::env::temp_dir().join(format!("radar-note-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("greenhouse.txt");
        std::fs::write(&p, "ramp\nstripe\n").unwrap();
        let l = load_from(&p);
        assert!(l.note().contains("greenhouse.txt"), "{}", l.note());
        assert!(l.note().contains('2'), "{}", l.note());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
