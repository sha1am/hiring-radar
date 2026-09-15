//! Workday's wire format -> `RawPost`, including its relative-date strings.

use super::client::{Detail, Posting};
use super::target::Target;
use crate::model::{ApplyChannel, RawPost};
use crate::sources::common::{pretty_slug, strip_html};

pub const SOURCE: &str = "workday";

/// Turn `postedOn` into a timestamp.
///
/// Workday sends a localised display string — "Posted Today", "Posted 5 Days
/// Ago", "Posted 30+ Days Ago" — with no machine-readable date anywhere in the
/// listing payload. The real date exists, but only on the per-job detail call,
/// which is one request per posting.
///
/// So this reads the display string, and is deliberately coarse: day
/// granularity, and "30+" means "at least 30", which we record as exactly 30 so
/// it sorts as old rather than as unknown. Anything it cannot read returns None,
/// which the pipeline treats as "no idea when" rather than "just now" — a job
/// from last quarter must never arrive as breaking news.
pub fn posted_at(display: &str, now: i64) -> Option<i64> {
    let s = display.to_lowercase();
    let day = 86_400i64;

    if s.contains("today") || s.contains("just posted") {
        return Some(now);
    }
    if s.contains("yesterday") {
        return Some(now - day);
    }

    // "Posted 5 Days Ago", "Posted 30+ Days Ago"
    let n: String = s
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if n.is_empty() {
        return None;
    }
    let n: i64 = n.parse().ok()?;

    if s.contains("day") {
        Some(now - n * day)
    } else if s.contains("hour") {
        Some(now - n * 3600)
    } else if s.contains("minute") {
        Some(now)
    } else if s.contains("month") {
        Some(now - n * 30 * day)
    } else {
        None
    }
}

/// `None` for a posting too incomplete to act on — no title, or no path to
/// link to. One malformed row costs one row.
pub fn to_post(
    t: &Target,
    company: &str,
    p: Posting,
    detail: Option<Detail>,
    now: i64,
) -> Option<RawPost> {
    let title = p.title.clone()?;
    let external_path = p.external_path.clone()?;
    let url = t.public_url(&external_path);

    // The requisition id is stable across re-postings and re-titles; the path is
    // not. Prefer it, and scope everything to the tenant because two companies
    // will happily both call something R12345.
    let req = detail
        .as_ref()
        .and_then(|d| d.job_req_id.clone())
        .or_else(|| p.bullet_fields.first().cloned())
        .unwrap_or_else(|| external_path.clone());

    // The detail call carries a real date. When we paid for it, use it.
    let posted = detail
        .as_ref()
        .and_then(|d| crate::timeparse::parse_opt(d.start_date.as_deref()))
        .or_else(|| p.posted_on.as_deref().and_then(|d| posted_at(d, now)));

    // Workday splits the country off from the city. Scoring wants them together,
    // or "Bengaluru" never satisfies a filter that says "India".
    let location = match (
        p.locations_text.as_deref(),
        detail
            .as_ref()
            .and_then(|d| d.country.as_ref())
            .and_then(|c| c.descriptor.as_deref()),
    ) {
        (Some(l), Some(c)) if !l.to_lowercase().contains(&c.to_lowercase()) => {
            Some(format!("{l}, {c}"))
        }
        (Some(l), _) => Some(l.to_string()),
        (None, Some(c)) => Some(c.to_string()),
        (None, None) => None,
    };

    Some(RawPost {
        source: SOURCE.into(),
        external_id: format!("{}:{req}", t.tenant),
        url: url.clone(),
        title,
        company: company.to_string(),
        location,
        body: detail
            .and_then(|d| d.job_description)
            .map(|b| strip_html(&b))
            .unwrap_or_default(),
        posted_at: posted,
        apply: ApplyChannel::ExternalUrl(url),
        synthetic_title: false,
    })
}

/// The company name to show for a tenant.
pub fn company_for(t: &Target) -> String {
    pretty_slug(&t.tenant)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn target() -> Target {
        super::super::target::parse("acme:wd5:Careers").unwrap()
    }

    fn posting() -> Posting {
        serde_json::from_value(serde_json::json!({
            "title": "Backend Engineer",
            "externalPath": "/job/Bengaluru/Backend_R99",
            "locationsText": "Bengaluru",
            "postedOn": "Posted 3 Days Ago",
            "bulletFields": ["R99"]
        }))
        .unwrap()
    }

    #[test]
    fn relative_dates_become_timestamps() {
        assert_eq!(posted_at("Posted Today", NOW), Some(NOW));
        assert_eq!(posted_at("Posted Yesterday", NOW), Some(NOW - 86_400));
        assert_eq!(posted_at("Posted 3 Days Ago", NOW), Some(NOW - 3 * 86_400));
        assert_eq!(posted_at("Posted 5 Hours Ago", NOW), Some(NOW - 5 * 3600));
    }

    #[test]
    fn thirty_plus_days_sorts_as_old_not_as_unknown() {
        let t = posted_at("Posted 30+ Days Ago", NOW).unwrap();
        assert!(t <= NOW - 30 * 86_400);
    }

    #[test]
    fn an_unreadable_date_is_unknown_rather_than_now() {
        // The alternative — defaulting to now — makes a job from last quarter
        // arrive as breaking news, which is the one thing an alert tool must
        // never do.
        assert_eq!(posted_at("Veröffentlicht vor langer Zeit", NOW), None);
        assert_eq!(posted_at("", NOW), None);
    }

    #[test]
    fn a_posting_with_no_title_is_dropped_not_stored_blank() {
        let mut p = posting();
        p.title = None;
        assert!(to_post(&target(), "Acme", p, None, NOW).is_none());
    }

    #[test]
    fn the_req_id_is_the_identity_and_it_is_tenant_scoped() {
        let p = to_post(&target(), "Acme", posting(), None, NOW).unwrap();
        assert_eq!(p.external_id, "acme:R99");
    }

    #[test]
    fn the_card_links_to_the_human_page_not_the_api() {
        let p = to_post(&target(), "Acme", posting(), None, NOW).unwrap();
        assert!(p.url.contains("/Careers/job/Bengaluru/"), "{}", p.url);
        assert!(!p.url.contains("/wday/cxs/"), "{}", p.url);
    }

    #[test]
    fn the_country_is_appended_so_a_city_satisfies_a_country_filter() {
        // "Bengaluru" alone never matches a filter that says "India".
        let detail: Detail = serde_json::from_value(serde_json::json!({
            "jobDescription": "<p>Go &amp; Kafka</p>",
            "country": {"descriptor": "India"},
            "startDate": "2026-09-10"
        }))
        .unwrap();
        let p = to_post(&target(), "Acme", posting(), Some(detail), NOW).unwrap();
        assert_eq!(p.location.as_deref(), Some("Bengaluru, India"));
        assert_eq!(p.body, "Go & Kafka");
    }

    #[test]
    fn a_country_already_in_the_location_is_not_repeated() {
        let mut posting = posting();
        posting.locations_text = Some("Bengaluru, India".into());
        let detail: Detail =
            serde_json::from_value(serde_json::json!({"country": {"descriptor": "India"}})).unwrap();
        let p = to_post(&target(), "Acme", posting, Some(detail), NOW).unwrap();
        assert_eq!(p.location.as_deref(), Some("Bengaluru, India"));
    }

    #[test]
    fn a_real_start_date_beats_the_display_string() {
        let detail: Detail =
            serde_json::from_value(serde_json::json!({"startDate": "2026-09-10T00:00:00Z"}))
                .unwrap();
        let p = to_post(&target(), "Acme", posting(), Some(detail), NOW).unwrap();
        assert_eq!(p.posted_at, crate::timeparse::parse("2026-09-10T00:00:00Z"));
    }
}
