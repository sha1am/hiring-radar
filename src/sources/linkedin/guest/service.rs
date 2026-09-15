//! The guest-search crawl strategy.

use super::client::Client;
use super::parse;
use crate::settings::Settings;
use crate::sources::{Fetched, JobSource};

pub struct LinkedInGuest {
    client: Client,
}

impl LinkedInGuest {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            client: Client::new(http),
        }
    }
}

#[async_trait::async_trait]
impl JobSource for LinkedInGuest {
    fn name(&self) -> &str {
        parse::SOURCE
    }

    fn is_enabled(&self, s: &Settings) -> bool {
        s.linkedin_guest_enabled
    }

    async fn fetch(&self, s: &Settings) -> anyhow::Result<Fetched> {
        let mut out = Fetched::default();
        if !s.linkedin_guest_enabled {
            return Ok(out);
        }
        if s.linkedin_queries.is_empty() {
            out.info("no queries configured");
            return Ok(out);
        }

        for q in &s.linkedin_queries {
            match self.client.search(&q.keywords, &q.location).await {
                Err(why) => {
                    tracing::warn!(kw = q.keywords, why, "linkedin guest search failed");
                    out.fail(format!("{}: {why}", q.keywords));
                }
                Ok(html) => {
                    let cards = parse::cards(&html);
                    out.ok(format!("{}: {} cards", q.keywords, cards.len()));
                    out.posts.extend(cards);
                }
            }
        }
        Ok(out)
    }
}
