//! Guest job cards (HTML) -> `RawPost`.

use crate::model::{ApplyChannel, RawPost};
use crate::sources::common::text::clean_url;
use crate::timeparse;
use scraper::{Html, Selector};

pub const SOURCE: &str = "linkedin_guest";

pub fn cards(html: &str) -> Vec<RawPost> {
    let doc = Html::parse_fragment(html);
    // Selectors are compiled from static strings, so unwrap is safe.
    let card_sel = Selector::parse("li").unwrap();
    let title_sel = Selector::parse("h3.base-search-card__title").unwrap();
    let company_sel = Selector::parse("h4.base-search-card__subtitle").unwrap();
    let loc_sel = Selector::parse("span.job-search-card__location").unwrap();
    let link_sel = Selector::parse("a.base-card__full-link").unwrap();
    let base_sel = Selector::parse("div.base-card").unwrap();
    // Guest cards carry the list date as <time datetime="YYYY-MM-DD">. Date
    // granularity only, but it separates today's postings from last week's.
    let time_sel = Selector::parse("time[datetime]").unwrap();

    let mut out = Vec::new();
    for li in doc.select(&card_sel) {
        let Some(title) = li
            .select(&title_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
        else {
            continue;
        };

        let company = li
            .select(&company_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| "Unknown".into());

        let location = li
            .select(&loc_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string());

        let url = li
            .select(&link_sel)
            .next()
            .and_then(|e| e.value().attr("href"))
            .map(clean_url)
            .unwrap_or_default();
        if url.is_empty() {
            continue;
        }

        // Prefer the stable jobPosting id from the entity urn; fall back to the
        // url, which changes when LinkedIn changes its slugs.
        let external_id = li
            .select(&base_sel)
            .next()
            .and_then(|e| e.value().attr("data-entity-urn"))
            .and_then(|urn| urn.rsplit(':').next())
            .map(str::to_string)
            .unwrap_or_else(|| url.clone());

        let posted_at = li
            .select(&time_sel)
            .next()
            .and_then(|e| e.value().attr("datetime"))
            .and_then(timeparse::parse);

        out.push(RawPost {
            source: SOURCE.into(),
            external_id,
            url: url.clone(),
            title,
            company,
            location,
            // Guest cards carry no description; title+company still score fine.
            body: String::new(),
            posted_at,
            apply: ApplyChannel::ExternalUrl(url),
            synthetic_title: false,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: &str = r#"
    <li><div class="base-card" data-entity-urn="urn:li:jobPosting:4111">
      <a class="base-card__full-link" href="https://www.linkedin.com/jobs/view/be-4111?trk=spam"></a>
      <h3 class="base-search-card__title">Senior Backend Engineer</h3>
      <h4 class="base-search-card__subtitle">Acme</h4>
      <span class="job-search-card__location">Bengaluru, India</span>
      <time datetime="2026-09-12"></time>
    </div></li>"#;

    #[test]
    fn a_card_becomes_a_post_without_its_tracking_params() {
        let posts = cards(CARD);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].title, "Senior Backend Engineer");
        assert_eq!(posts[0].external_id, "4111");
        assert!(!posts[0].url.contains("trk="), "{}", posts[0].url);
        assert!(posts[0].posted_at.is_some());
    }

    #[test]
    fn a_card_with_no_link_is_skipped_not_stored_blank() {
        // A post with no URL is a card you can never act on.
        assert!(cards(r#"<li><h3 class="base-search-card__title">Ghost</h3></li>"#).is_empty());
    }
}
