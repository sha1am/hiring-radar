//! Greenhouse's wire format -> a `RawPost` the pipeline can score.

use super::client::Job;
use crate::model::{ApplyChannel, RawPost};
use crate::sources::common::{pretty_slug, strip_html};
use crate::timeparse;

pub const SOURCE: &str = "greenhouse";

pub fn to_post(board: &str, company: &str, j: Job) -> RawPost {
    // first_published is what "posted" means; updated_at is the fallback, and
    // it makes an edited listing look new. Better than treating everything as
    // brand new, which is what no timestamp at all would do.
    let posted_at = timeparse::parse_opt(j.first_published.as_deref())
        .or_else(|| timeparse::parse_opt(j.updated_at.as_deref()));

    RawPost {
        source: SOURCE.into(),
        // Board-scoped: two boards can and do reuse the same numeric id.
        external_id: format!("{board}:{}", j.id),
        url: j.absolute_url.clone(),
        title: j.title,
        company: company.to_string(),
        location: j.location.map(|l| l.name),
        body: strip_html(&j.content),
        posted_at,
        apply: ApplyChannel::ExternalUrl(j.absolute_url),
        synthetic_title: false,
    }
}

/// The company name to show for a board token.
pub fn company_for(board: &str) -> String {
    pretty_slug(board)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> Job {
        serde_json::from_value(serde_json::json!({
            "id": 7,
            "title": "Backend Engineer",
            "absolute_url": "https://boards.greenhouse.io/acme/jobs/7",
            "location": {"name": "Bengaluru, India"},
            "content": "<p>Go &amp; Postgres</p>",
            "first_published": "2026-09-01T10:00:00Z",
            "updated_at": "2026-09-10T10:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn ids_are_scoped_to_the_board() {
        // Two boards reusing id 7 must not dedup against each other.
        assert_eq!(to_post("acme", "Acme", job()).external_id, "acme:7");
    }

    #[test]
    fn first_published_beats_updated_at() {
        let p = to_post("acme", "Acme", job());
        let first = timeparse::parse("2026-09-01T10:00:00Z").unwrap();
        assert_eq!(p.posted_at, Some(first));
    }

    #[test]
    fn description_reaches_the_scorer_as_words() {
        assert_eq!(to_post("acme", "Acme", job()).body, "Go & Postgres");
    }
}
