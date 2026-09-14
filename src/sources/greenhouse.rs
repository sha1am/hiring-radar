use super::JobSource;
use crate::model::{ApplyChannel, RawPost};
use crate::settings::Settings;
use crate::timeparse;
use serde::Deserialize;

/// Greenhouse exposes every company board as public JSON. No auth, no ban risk.
/// This is the source you point at first to prove the pipeline works end to end.
pub struct Greenhouse {
    client: reqwest::Client,
    base: String,
}

/// Where the board API lives. Overridable so the pipeline can be exercised
/// against a fixture server — there is no other way to test ingest end to end
/// without hitting Greenhouse for real.
const DEFAULT_BASE: &str = "https://boards-api.greenhouse.io/v1/boards";

impl Greenhouse {
    pub fn new(client: reqwest::Client) -> Self {
        let base = std::env::var("RADAR_GREENHOUSE_BASE")
            .unwrap_or_else(|_| DEFAULT_BASE.to_string());
        if base != DEFAULT_BASE {
            tracing::warn!(%base, "greenhouse base URL overridden (fixture mode?)");
        }
        Self { client, base }
    }
}

#[derive(Deserialize)]
struct Resp {
    jobs: Vec<Job>,
}
#[derive(Deserialize)]
struct Job {
    id: i64,
    title: String,
    absolute_url: String,
    #[serde(default)]
    location: Option<Loc>,
    #[serde(default)]
    content: String,
    /// When the listing first went live. Present on most boards; this is the
    /// field that actually means "posted".
    #[serde(default)]
    first_published: Option<String>,
    /// Always present. Falls back here, accepting that an edited listing looks
    /// newer than it is — still far better than treating every job as brand new.
    #[serde(default)]
    updated_at: Option<String>,
}
#[derive(Deserialize)]
struct Loc {
    name: String,
}

fn strip_html(s: &str) -> String {
    // Good-enough tag stripper for scoring + display; not a real HTML parser.
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
}

#[async_trait::async_trait]
impl JobSource for Greenhouse {
    fn name(&self) -> &str {
        "greenhouse"
    }

    async fn fetch(&self, cfg: &Settings) -> anyhow::Result<Vec<RawPost>> {
        let mut posts = Vec::new();
        if !cfg.greenhouse_enabled {
            return Ok(posts);
        }
        for board in &cfg.greenhouse_boards {
            let url = format!("{}/{board}/jobs?content=true", self.base.trim_end_matches('/'));
            let resp = match self.client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(board, %e, "greenhouse fetch failed");
                    continue;
                }
            };
            if !resp.status().is_success() {
                tracing::warn!(board, status = %resp.status(), "greenhouse non-200");
                continue;
            }
            let data: Resp = match resp.json().await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(board, %e, "greenhouse json parse failed");
                    continue;
                }
            };
            let company = pretty(board);
            for j in data.jobs {
                let posted_at = timeparse::parse_opt(j.first_published.as_deref())
                    .or_else(|| timeparse::parse_opt(j.updated_at.as_deref()));
                posts.push(RawPost {
                    source: "greenhouse".into(),
                    external_id: format!("{board}:{}", j.id),
                    url: j.absolute_url.clone(),
                    title: j.title,
                    company: company.clone(),
                    location: j.location.map(|l| l.name),
                    body: strip_html(&j.content),
                    posted_at,
                    apply: ApplyChannel::ExternalUrl(j.absolute_url),
                });
            }
        }
        Ok(posts)
    }
}

fn pretty(board: &str) -> String {
    let mut c = board.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => board.to_string(),
    }
}
