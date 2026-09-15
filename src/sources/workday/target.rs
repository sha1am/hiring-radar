//! A Workday careers site, as named in `companies/workday.txt`.
//!
//! Workday is hosted per customer, so there is no single API to point at: every
//! company has its own tenant, its own data-centre number and its own site name,
//! and all three appear in the URL. `nvidia.wd5.myworkdayjobs.com/NVIDIAExternalCareerSite`
//! is the tenant `nvidia`, on `wd5`, site `NVIDIAExternalCareerSite`.
//!
//! Two ways to write one down, because getting this wrong is the single most
//! likely reason this source returns nothing:
//!
//! ```text
//! nvidia:wd5:NVIDIAExternalCareerSite
//! https://nvidia.wd5.myworkdayjobs.com/en-US/NVIDIAExternalCareerSite
//! ```
//!
//! The second form is the one you can paste straight out of the address bar,
//! which means fixing a broken entry is a copy rather than a puzzle.

/// One careers site to crawl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub tenant: String,
    /// The data-centre shard, e.g. `wd5`. Part of the hostname, not optional.
    pub dc: String,
    pub site: String,
}

impl Target {
    /// The CXS (career experience service) API root — the JSON endpoint the
    /// site's own front-end calls.
    pub fn api(&self) -> String {
        format!(
            "https://{}.{}.myworkdayjobs.com/wday/cxs/{}/{}",
            self.tenant, self.dc, self.tenant, self.site
        )
    }

    /// Where a human should land when they click the card. Note this is *not*
    /// the API path — the CXS root is an implementation detail of the site's
    /// own front-end and 404s in a browser.
    pub fn public_url(&self, external_path: &str) -> String {
        let path = external_path.trim_start_matches('/');
        let base = format!(
            "https://{}.{}.myworkdayjobs.com/{}",
            self.tenant, self.dc, self.site
        );
        if path.is_empty() {
            base
        } else {
            format!("{base}/{path}")
        }
    }

    /// How this target is written in the file, for notes and errors.
    pub fn label(&self) -> String {
        format!("{}:{}:{}", self.tenant, self.dc, self.site)
    }
}

/// Parse one line of `companies/workday.txt`.
///
/// Returns the reason it could not be read rather than skipping quietly — a
/// typo'd tenant is indistinguishable from a company with no openings unless
/// something says so.
pub fn parse(entry: &str) -> Result<Target, String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return Err("empty entry".into());
    }
    if entry.contains("://") || entry.contains("myworkdayjobs.com") {
        from_url(entry)
    } else {
        from_triple(entry)
    }
}

fn from_triple(entry: &str) -> Result<Target, String> {
    let parts: Vec<&str> = entry.split(':').map(str::trim).collect();
    match parts.as_slice() {
        [tenant, dc, site] if !tenant.is_empty() && !site.is_empty() && is_dc(dc) => Ok(Target {
            tenant: tenant.to_string(),
            dc: dc.to_lowercase(),
            site: site.to_string(),
        }),
        _ => Err(format!(
            "expected tenant:wdN:site or a careers URL, got {entry:?}"
        )),
    }
}

fn from_url(entry: &str) -> Result<Target, String> {
    let rest = entry.split("://").last().unwrap_or(entry);
    let mut parts = rest.split('/');
    let host = parts.next().unwrap_or("");

    let labels: Vec<&str> = host.split('.').collect();
    let (tenant, dc) = match labels.as_slice() {
        [tenant, dc, "myworkdayjobs", "com", ..] if is_dc(dc) => (*tenant, *dc),
        _ => {
            return Err(format!(
                "not a Workday careers host: {host:?} (expected <tenant>.wdN.myworkdayjobs.com)"
            ))
        }
    };

    // The path is /[locale/]<site>[/...]. The locale segment is optional and
    // looks like en-US; skipping it by shape avoids maintaining a locale list.
    let site = parts
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .find(|s| !is_locale(s))
        .ok_or_else(|| format!("no site name in {entry:?} — the part after the host"))?;

    Ok(Target {
        tenant: tenant.to_string(),
        dc: dc.to_lowercase(),
        site: site.to_string(),
    })
}

fn is_dc(s: &str) -> bool {
    let s = s.to_lowercase();
    s.strip_prefix("wd")
        .map(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
}

/// `en-US`, `en_US`, `fr-CA` — two letters, a separator, two letters.
fn is_locale(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5
        && b[0].is_ascii_alphabetic()
        && b[1].is_ascii_alphabetic()
        && (b[2] == b'-' || b[2] == b'_')
        && b[3].is_ascii_alphabetic()
        && b[4].is_ascii_alphabetic()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compact_form_parses() {
        let t = parse("nvidia:wd5:NVIDIAExternalCareerSite").unwrap();
        assert_eq!(t.tenant, "nvidia");
        assert_eq!(t.dc, "wd5");
        assert_eq!(t.site, "NVIDIAExternalCareerSite");
    }

    #[test]
    fn a_pasted_address_bar_url_parses_locale_and_all() {
        // This is the whole point: fixing a broken entry should be a paste.
        let t = parse("https://nvidia.wd5.myworkdayjobs.com/en-US/NVIDIAExternalCareerSite").unwrap();
        assert_eq!(t, parse("nvidia:wd5:NVIDIAExternalCareerSite").unwrap());
    }

    #[test]
    fn a_url_with_a_job_path_still_yields_the_site() {
        let t = parse("https://cisco.wd5.myworkdayjobs.com/cisco/job/India/Engineer_1234").unwrap();
        assert_eq!(t.site, "cisco");
    }

    #[test]
    fn the_api_and_public_urls_are_different_shapes() {
        // Getting these backwards yields a card that 404s for the human, or a
        // fetch that returns HTML instead of JSON — both silent-ish failures.
        let t = parse("nvidia:wd5:NVIDIAExternalCareerSite").unwrap();
        assert_eq!(
            t.api(),
            "https://nvidia.wd5.myworkdayjobs.com/wday/cxs/nvidia/NVIDIAExternalCareerSite"
        );
        assert_eq!(
            t.public_url("/job/India/Engineer_1234"),
            "https://nvidia.wd5.myworkdayjobs.com/NVIDIAExternalCareerSite/job/India/Engineer_1234"
        );
    }

    #[test]
    fn a_bad_entry_explains_itself() {
        let e = parse("nvidia").unwrap_err();
        assert!(e.contains("tenant:wdN:site"), "{e}");
        let e = parse("https://jobs.example.com/careers").unwrap_err();
        assert!(e.contains("myworkdayjobs"), "{e}");
    }

    #[test]
    fn a_missing_shard_is_rejected_rather_than_guessed() {
        // There is no default shard — wd1 and wd5 are different hosts, and
        // guessing produces a DNS error that looks like the company is gone.
        assert!(parse("nvidia::Careers").is_err());
        assert!(parse("nvidia:x5:Careers").is_err());
    }
}
