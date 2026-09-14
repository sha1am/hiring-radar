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
    fn rejects_implausible() {
        assert_eq!(parse("1970-01-01"), None);
        assert_eq!(parse("2099-01-01"), None);
        assert_eq!(parse("not a date"), None);
        assert_eq!(parse(""), None);
    }
}
