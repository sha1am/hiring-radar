use super::JobSource;
use crate::config::LinkedInQuery;
use crate::model::{ApplyChannel, RawPost};
use crate::timeparse;
use scraper::{Html, Selector};

/// The public "see more job postings" endpoint LinkedIn serves without login.
/// f_TPR=r3600 asks for postings from the last hour, which is exactly the
/// freshness window we care about. Returns HTML job cards.
pub struct LinkedInGuest {
    client: reqwest::Client,
    queries: Vec<LinkedInQuery>,
}

impl LinkedInGuest {
    pub fn new(client: reqwest::Client, queries: Vec<LinkedInQuery>) -> Self {
        Self { client, queries }
    }
}

#[async_trait::async_trait]
impl JobSource for LinkedInGuest {
    fn name(&self) -> &str {
        "linkedin_guest"
    }

    async fn fetch(&self) -> anyhow::Result<Vec<RawPost>> {
        let mut posts = Vec::new();
        for q in &self.queries {
            let url = "https://www.linkedin.com/jobs-guest/jobs/api/seeMoreJobPostings/search";
            let resp = self
                .client
                .get(url)
                .query(&[
                    ("keywords", q.keywords.as_str()),
                    ("location", q.location.as_str()),
                    ("f_TPR", "r3600"),
                    ("start", "0"),
                ])
                .send()
                .await;
            let html = match resp {
                Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
                Ok(r) => {
                    // 429 here means slow down: widen your poll interval / jitter.
                    tracing::warn!(status = %r.status(), kw = q.keywords, "linkedin guest non-200");
                    continue;
                }
                Err(e) => {
                    tracing::warn!(%e, kw = q.keywords, "linkedin guest fetch failed");
                    continue;
                }
            };
            posts.extend(parse_cards(&html));
        }
        Ok(posts)
    }
}

fn parse_cards(html: &str) -> Vec<RawPost> {
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
        let title = li
            .select(&title_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string());
        let Some(title) = title else { continue };

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

        // Prefer the stable jobPosting id from the entity urn; fall back to the url.
        let external_id = li
            .select(&base_sel)
            .next()
            .and_then(|e| e.value().attr("data-entity-urn"))
            .and_then(|urn| urn.rsplit(':').next())
            .map(|s| s.to_string())
            .unwrap_or_else(|| url.clone());

        let posted_at = li
            .select(&time_sel)
            .next()
            .and_then(|e| e.value().attr("datetime"))
            .and_then(timeparse::parse);

        if url.is_empty() {
            continue;
        }

        out.push(RawPost {
            source: "linkedin_guest".into(),
            external_id,
            url: url.clone(),
            title,
            company,
            location,
            // Guest cards carry no description; title+company still score fine.
            body: String::new(),
            posted_at,
            apply: ApplyChannel::ExternalUrl(url),
        });
    }
    out
}

fn clean_url(href: &str) -> String {
    // Drop tracking query params.
    href.split('?').next().unwrap_or(href).to_string()
}
