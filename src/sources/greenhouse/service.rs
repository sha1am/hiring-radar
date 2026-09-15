//! The Greenhouse crawl strategy.

use super::client::Client;
use super::parse;
use crate::settings::Settings;
use crate::sources::common::companies;
use crate::sources::{Fetched, JobSource};

pub struct Greenhouse {
    client: Client,
}

impl Greenhouse {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            client: Client::new(http),
        }
    }
}

#[async_trait::async_trait]
impl JobSource for Greenhouse {
    fn name(&self) -> &str {
        parse::SOURCE
    }

    fn is_enabled(&self, s: &Settings) -> bool {
        s.greenhouse_enabled
    }

    async fn fetch(&self, s: &Settings) -> anyhow::Result<Fetched> {
        let mut out = Fetched::default();
        if !s.greenhouse_enabled {
            return Ok(out);
        }

        // Re-read on every crawl, not at boot: adding a company should take
        // effect on the next cycle, not the next restart.
        let list = companies::load("greenhouse");
        out.info(list.note());
        let boards = companies::resolve(&list, &s.greenhouse_boards);

        if boards.is_empty() {
            out.info(format!(
                "nothing to crawl — add board tokens to {}",
                list.path.display()
            ));
            return Ok(out);
        }

        for board in &boards {
            match self.client.jobs(board).await {
                Err(why) => {
                    tracing::warn!(board, why, "greenhouse board failed");
                    out.fail(format!("{board}: {why}"));
                }
                Ok(jobs) => {
                    out.ok(format!("{board}: {} jobs", jobs.len()));
                    let company = parse::company_for(board);
                    out.posts
                        .extend(jobs.into_iter().map(|j| parse::to_post(board, &company, j)));
                }
            }
        }
        Ok(out)
    }
}
