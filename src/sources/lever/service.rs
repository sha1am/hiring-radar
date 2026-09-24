//! The Lever crawl strategy — deliberately the same shape as Greenhouse's.

use super::client::Client;
use super::parse;
use crate::settings::Settings;
use crate::sources::common::companies;
use crate::sources::{Fetched, JobSource};

pub struct Lever {
    client: Client,
}

impl Lever {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            client: Client::new(http),
        }
    }
}

#[async_trait::async_trait]
impl JobSource for Lever {
    fn name(&self) -> &str {
        parse::SOURCE
    }

    fn is_enabled(&self, s: &Settings) -> bool {
        s.lever_enabled
    }

    async fn fetch(&self, s: &Settings) -> anyhow::Result<Fetched> {
        let mut out = Fetched::default();
        if !s.lever_enabled {
            return Ok(out);
        }

        let list = companies::load("lever");
        out.info(list.note());
        let slugs = companies::resolve(&list, &s.lever_boards);
        if slugs.is_empty() {
            out.info(format!(
                "nothing to crawl — add board slugs to {}",
                list.path.display()
            ));
            return Ok(out);
        }

        for slug in &slugs {
            match self.client.postings(slug).await {
                Err(why) => {
                    tracing::warn!(slug, why, "lever board failed");
                    out.fail(format!("{slug}: {why}"));
                }
                Ok(postings) => {
                    out.ok(format!("{slug}: {} jobs", postings.len()));
                    let company = parse::company_for(slug);
                    out.posts.extend(
                        postings
                            .into_iter()
                            .map(|p| parse::to_post(slug, &company, p)),
                    );
                }
            }
        }
        Ok(out)
    }
}
