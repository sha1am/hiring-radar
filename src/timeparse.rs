use crate::model::now;
use chrono::{DateTime, NaiveDate};

/// Sources report post times in whatever shape their API happens to use. This
/// normalises the handful we actually see into unix seconds, and refuses
/// anything implausible — a bad parse that lands in 1970 or 2087 would poison
/// freshness ranking far more quietly than a `None`.
///
/// Accepted: RFC3339 with offset (`2026-09-10T14:22:33-04:00`), the same with
/// `Z`, bare dates (`2026-09-13`, taken as 00:00 UTC), and epoch
/// seconds/milliseconds as digit strings (LinkedIn hands back ms).
pub fn parse(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let ts = if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        dt.timestamp()
    } else if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        d.and_hms_opt(0, 0, 0)?.and_utc().timestamp()
    } else if s.chars().all(|c| c.is_ascii_digit()) {
        let n: i64 = s.parse().ok()?;
        // 13 digits is milliseconds; 10 is seconds.
        if s.len() >= 13 {
            n / 1000
        } else {
            n
        }
    } else {
        return None;
    };

    plausible(ts)
}

/// Same, for an optional field.
pub fn parse_opt(s: Option<&str>) -> Option<i64> {
    s.and_then(parse)
}

/// A post timestamp is only useful if it could actually be a post timestamp.
/// Reject the future (beyond a little clock skew) and anything older than ~5
/// years, which in practice means a parse went wrong.
fn plausible(ts: i64) -> Option<i64> {
    let n = now();
    if ts > n + 3600 || ts < n - 5 * 365 * 86400 {
        return None;
    }
    Some(ts)
}

/// Recover a post time from a LinkedIn activity/share URN.
///
/// LinkedIn's activity IDs are Snowflake-style: the high 41 bits of the 64-bit
/// id are the creation time in milliseconds. So `id >> 22` is when the post was
/// made, with no extra request.
///
/// This matters more than it looks. Voyager reports no timestamp field, so
/// without this a "last 24 hours" window silently measures *when we crawled*
/// rather than when anything was posted — a week-old post looks brand new the
/// moment it is first seen, and freshness ranking is meaningless.
///
/// The bit layout is undocumented and could change, so the result is put
/// through the same plausibility gate as everything else: if the arithmetic is
/// wrong the answer lands decades away and is rejected, and the caller falls
/// back to detection time. Wrong-but-plausible is the only failure this cannot
/// catch, and a shifted layout would not be subtly wrong, it would be absurd.
pub fn from_linkedin_urn(urn: &str) -> Option<i64> {
    // urn:li:activity:7123456789012345678 — also share:, ugcPost:, etc.
    let id_str = urn.rsplit(':').next()?.trim();
    if id_str.is_empty() || !id_str.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let id: u64 = id_str.parse().ok()?;
    let ms = id >> 22;
    if ms == 0 {
        return None;
    }
    plausible((ms / 1000) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::now;

    #[test]
    fn parses_rfc3339_with_offset() {
        // Chosen relative to now so the plausibility window can't expire the test.
        let n = now();
        let dt = chrono::DateTime::from_timestamp(n - 7200, 0).unwrap();
        let s = dt.to_rfc3339();
        assert_eq!(parse(&s), Some(n - 7200));
    }

    #[test]
    fn parses_bare_date() {
        let today = chrono::DateTime::from_timestamp(now(), 0)
            .unwrap()
            .format("%Y-%m-%d")
            .to_string();
        assert!(parse(&today).is_some());
    }

    #[test]
    fn parses_epoch_millis() {
        let n = now();
        assert_eq!(parse(&format!("{}", n * 1000)), Some(n));
    }

    #[test]
    fn decodes_a_linkedin_activity_urn() {
        // Build an id the way LinkedIn does: known time in the high bits.
        let target_ms = (now() as u64 - 3600) * 1000;
        let id: u64 = target_ms << 22;
        let urn = format!("urn:li:activity:{id}");
        let decoded = from_linkedin_urn(&urn).expect("should decode");
        // Sub-second precision is lost to the shift; a second either way is fine.
        assert!(
            (decoded - (now() - 3600)).abs() <= 1,
            "decoded {decoded}, wanted about {}",
            now() - 3600
        );
    }

    #[test]
    fn urn_decoding_rejects_nonsense_rather_than_inventing_a_date() {
        assert_eq!(from_linkedin_urn("urn:li:activity:notanumber"), None);
        assert_eq!(from_linkedin_urn("urn:li:activity:1"), None); // year 1970
        assert_eq!(from_linkedin_urn(""), None);
        // A plainly wrong bit layout must fail the plausibility gate, not
        // silently produce a date in the far future.
        assert_eq!(from_linkedin_urn("urn:li:activity:99999999999999999999"), None);
    }

    #[test]
    fn rejects_implausible() {
        assert_eq!(parse("1970-01-01"), None);
        assert_eq!(parse("2099-01-01"), None);
        assert_eq!(parse("not a date"), None);
        assert_eq!(parse(""), None);
    }
}
