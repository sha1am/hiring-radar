//! Lever's wire format -> a `RawPost`.

use super::client::Posting;
use crate::model::{ApplyChannel, RawPost};
use crate::sources::common::{pretty_slug, strip_html};

pub const SOURCE: &str = "lever";

pub fn to_post(slug: &str, company: &str, p: Posting) -> RawPost {
    // The bullet blocks carry the requirements — "5+ years of Go" is almost
    // never in the summary paragraph. Without them the scorer and the years
    // extractor are reading the marketing copy and missing the job.
    let mut body = p.description_plain.clone();
    for l in &p.lists {
        body.push_str("\n\n");
        body.push_str(&l.text);
        body.push('\n');
        body.push_str(&strip_html(&l.content));
    }
    if !p.additional_plain.trim().is_empty() {
        body.push_str("\n\n");
        body.push_str(&p.additional_plain);
    }

    // Lever reports milliseconds. Treating them as seconds puts every listing
    // fifty thousand years in the future, where the freshness weighting hands
    // it a perfect score forever.
    let posted_at = p
        .created_at
        .filter(|ms| *ms > 0)
        .and_then(|ms| crate::timeparse::plausible(ms / 1000));

    let url = p.hosted_url.clone();
    RawPost {
        source: SOURCE.into(),
        // Lever ids are UUIDs and globally unique, but scoping to the board
        // keeps every source's ids readable and costs nothing.
        external_id: format!("{slug}:{}", p.id),
        title: p.text,
        company: company.to_string(),
        location: p.categories.location.clone(),
        body,
        posted_at,
        // The apply URL when given, so the card's button lands on the form
        // rather than on the description you have already read.
        apply: ApplyChannel::ExternalUrl(p.apply_url.clone().unwrap_or_else(|| url.clone())),
        url,
        synthetic_title: false,
    }
}

pub fn company_for(slug: &str) -> String {
    pretty_slug(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posting() -> Posting {
        serde_json::from_value(serde_json::json!({
            "id": "abc-123",
            "text": "Senior Backend Engineer",
            "hostedUrl": "https://jobs.lever.co/cred/abc-123",
            "applyUrl": "https://jobs.lever.co/cred/abc-123/apply",
            "createdAt": 1789000000000i64,
            "categories": {"location": "Bengaluru", "commitment": "Full-time"},
            "descriptionPlain": "Join us.",
            "lists": [{"text": "Requirements", "content": "<ul><li>5+ years of Go</li></ul>"}],
            "additionalPlain": "Benefits."
        }))
        .unwrap()
    }

    #[test]
    fn milliseconds_are_not_mistaken_for_seconds() {
        // Treating ms as s puts the listing fifty thousand years out, where the
        // freshness weighting hands it a perfect score forever.
        let p = to_post("cred", "Cred", posting());
        let ts = p.posted_at.expect("should parse");
        assert!(ts > 1_700_000_000 && ts < 2_000_000_000, "{ts}");
    }

    #[test]
    fn the_requirement_bullets_reach_the_scorer() {
        // "5+ years of Go" is never in the summary paragraph. Without the
        // bullets, the scorer is reading marketing copy.
        let p = to_post("cred", "Cred", posting());
        assert!(p.body.contains("5+ years of Go"), "{}", p.body);
        assert!(p.body.contains("Join us."));
        assert!(p.body.contains("Benefits."));
    }

    #[test]
    fn the_button_goes_to_the_form_and_the_card_to_the_description() {
        let p = to_post("cred", "Cred", posting());
        assert!(p.url.ends_with("abc-123"));
        match p.apply {
            ApplyChannel::ExternalUrl(u) => assert!(u.ends_with("/apply"), "{u}"),
            other => panic!("expected an external apply URL, got {other:?}"),
        }
    }

    #[test]
    fn a_posting_with_no_date_is_unknown_rather_than_now() {
        let mut p = posting();
        p.created_at = None;
        assert_eq!(to_post("cred", "Cred", p).posted_at, None);
    }
}
