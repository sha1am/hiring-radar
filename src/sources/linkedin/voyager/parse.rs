//! Finding feed posts in a normalised Voyager payload.

use crate::model::{ApplyChannel, RawPost};
use crate::sources::common::{extract_email, first_line};
use serde_json::Value;

pub const SOURCE: &str = "linkedin_voyager";

/// Recursively scan the normalised JSON for anything that looks like a feed post.
///
/// A walk rather than a typed deserialisation because the payload is a
/// normalised entity graph whose exact nesting changes between releases — and a
/// serde struct that stops matching yields *nothing*, silently, whereas a walk
/// that stops matching yields nothing *and* can report what it saw instead.
pub fn walk(v: &Value, out: &mut Vec<RawPost>) {
    match v {
        Value::Object(map) => {
            if let Some(post) = looks_like_post(map) {
                out.push(post);
            }
            for val in map.values() {
                walk(val, out);
            }
        }
        Value::Array(arr) => {
            for val in arr {
                walk(val, out);
            }
        }
        _ => {}
    }
}

pub fn posts(body: &Value) -> Vec<RawPost> {
    let mut out = Vec::new();
    walk(body, &mut out);
    out
}

/// Heuristic extractor. Adjust the field names to whatever the live payload uses.
fn looks_like_post(map: &serde_json::Map<String, Value>) -> Option<RawPost> {
    // Commentary text lives under a few possible paths across API versions.
    let text = map
        .get("commentary")
        .and_then(|c| c.get("text"))
        .and_then(|t| t.get("text"))
        .and_then(|t| t.as_str())
        .or_else(|| map.get("commentaryText").and_then(|t| t.as_str()))?;

    if text.trim().is_empty() {
        return None;
    }

    let urn = map.get("entityUrn").and_then(|u| u.as_str())?.to_string();
    if urn.is_empty() {
        return None;
    }

    let author = map
        .get("actor")
        .and_then(|a| a.get("name"))
        .and_then(|n| n.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("LinkedIn member")
        .to_string();

    let profile_url = map
        .get("actor")
        .and_then(|a| a.get("navigationContext"))
        .and_then(|n| n.get("actionTarget"))
        .and_then(|t| t.as_str())
        .map(str::to_string);

    // How you'd actually reply: an address if the post names one, otherwise the
    // poster's profile. Unknown means the card is unactionable, so it is worth
    // keeping distinct rather than defaulting to a dead link.
    let apply = match extract_email(text) {
        Some(email) => ApplyChannel::Email(email),
        None => match &profile_url {
            Some(p) => ApplyChannel::LinkedInDm {
                profile_url: p.clone(),
            },
            None => ApplyChannel::Unknown,
        },
    };

    // The post itself, not the poster — clicking a card should land on the
    // thing you read about, and the feed permalink is derivable from the urn.
    let url = format!("https://www.linkedin.com/feed/update/{urn}");

    Some(RawPost {
        source: SOURCE.into(),
        // Voyager reports no timestamp; the activity id carries one.
        posted_at: crate::timeparse::from_linkedin_urn(&urn),
        external_id: urn,
        title: first_line(text, 120),
        company: author,
        location: None,
        body: text.to_string(),
        url,
        apply,
        // A feed post has no title; this is its first line.
        synthetic_title: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> Value {
        serde_json::json!({
            "included": [{
                "entityUrn": "urn:li:activity:7300000000000000000",
                "commentary": {"text": {"text": "We're hiring!\nBackend, Go. Mail cv@acme.io"}},
                "actor": {
                    "name": {"text": "Asha R"},
                    "navigationContext": {"actionTarget": "https://www.linkedin.com/in/asha"}
                }
            }]
        })
    }

    #[test]
    fn a_post_is_found_however_deeply_it_is_nested() {
        let wrapped = serde_json::json!({"data": {"x": [payload()]}});
        assert_eq!(posts(&wrapped).len(), 1);
    }

    #[test]
    fn the_title_is_the_first_line_and_the_body_is_everything() {
        let p = &posts(&payload())[0];
        assert_eq!(p.title, "We're hiring!");
        assert!(p.body.contains("Backend, Go"));
        assert!(p.synthetic_title);
    }

    #[test]
    fn an_address_in_the_text_beats_a_dm() {
        match &posts(&payload())[0].apply {
            ApplyChannel::Email(e) => assert_eq!(e, "cv@acme.io"),
            other => panic!("expected the address it asked for, got {other:?}"),
        }
    }

    #[test]
    fn the_link_goes_to_the_post_not_the_poster() {
        // Clicking a card should land on the thing you just read about.
        assert!(posts(&payload())[0]
            .url
            .contains("/feed/update/urn:li:activity:"));
    }

    #[test]
    fn the_activity_id_supplies_the_timestamp() {
        assert!(posts(&payload())[0].posted_at.is_some());
    }

    #[test]
    fn an_entity_without_commentary_is_not_a_post() {
        let v =
            serde_json::json!({"entityUrn": "urn:li:member:1", "actor": {"name": {"text": "X"}}});
        assert!(posts(&v).is_empty());
    }
}
