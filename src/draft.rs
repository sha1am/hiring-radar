use crate::config::DraftCfg;
use crate::model::{ApplyChannel, RawPost};
use crate::settings::Settings;
use serde_json::json;

/// Produces (subject, body). Subject is only meaningful for the email channel.
/// Drafts are generated speculatively at detection time for strong+ posts, so a
/// finished draft is already waiting the instant a card hits the dashboard.
#[async_trait::async_trait]
pub trait Drafter: Send + Sync {
    async fn draft(&self, post: &RawPost, p: &Settings, name: &str, email: &str) -> (String, String);
}

/// Instant, free, offline. Shapes the message to the post's apply channel.
pub struct TemplateDrafter;

#[async_trait::async_trait]
impl Drafter for TemplateDrafter {
    async fn draft(&self, post: &RawPost, p: &Settings, name: &str, email: &str) -> (String, String) {
        let subject = format!("Application: {} at {}", post.title, post.company);
        let skills = p.keywords.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
        let body = match &post.apply {
            ApplyChannel::LinkedInDm { .. } => format!(
                "Hi — saw your post about the {} role. I'm a backend engineer with \
                 experience in {}, and it looks like a strong fit. Would love to share \
                 my background — open to a quick chat?\n\n— {}",
                post.title, skills, name
            ),
            _ => format!(
                "Hi,\n\nI came across the {} opening at {} and I'm very interested. \
                 I work as a backend engineer with hands-on experience in {}, and I \
                 believe I'd contribute quickly to your team.\n\nI've attached my resume \
                 and would welcome the chance to discuss the role.\n\nBest regards,\n{}\n{}",
                post.title, post.company, skills, name, email
            ),
        };
        (subject, body)
    }
}

/// Local LLM via Ollama. Still $0, runs on your machine. Falls back to the
/// template on any error so a draft is never missing.
pub struct OllamaDrafter {
    pub client: reqwest::Client,
    pub url: String,
    pub model: String,
    pub fallback: TemplateDrafter,
}

#[async_trait::async_trait]
impl Drafter for OllamaDrafter {
    async fn draft(&self, post: &RawPost, p: &Settings, name: &str, email: &str) -> (String, String) {
        let channel = post.apply.kind();
        let prompt = format!(
            "You are helping {name} apply to a job. Write a short, specific, \
             non-generic outreach message for this {channel} channel. No fluff, \
             no placeholders, 90 words max. End with the name only.\n\n\
             Candidate skills: {skills}\n\
             Role: {title}\nCompany: {company}\n\
             Post:\n{body}\n\nMessage:",
            name = name,
            skills = p.keywords.join(", "),
            title = post.title,
            company = post.company,
            body = post.body.chars().take(1500).collect::<String>(),
        );

        let req = json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
        });

        let out = async {
            let resp = self
                .client
                .post(format!("{}/api/generate", self.url))
                .json(&req)
                .send()
                .await
                .ok()?;
            let v: serde_json::Value = resp.json().await.ok()?;
            v.get("response")?.as_str().map(|s| s.trim().to_string())
        }
        .await;

        match out {
            Some(text) if !text.is_empty() => {
                let subject = format!("Application: {} at {}", post.title, post.company);
                (subject, text)
            }
            _ => {
                tracing::warn!("ollama draft failed; using template");
                self.fallback.draft(post, p, name, email).await
            }
        }
    }
}

pub fn build_drafter(cfg: &DraftCfg, client: reqwest::Client) -> Box<dyn Drafter> {
    match cfg.provider.as_str() {
        "ollama" => Box::new(OllamaDrafter {
            client,
            url: cfg.ollama_url.clone(),
            model: cfg.ollama_model.clone(),
            fallback: TemplateDrafter,
        }),
        _ => Box::new(TemplateDrafter),
    }
}

/// Fill in drafts for cards that don't have one yet, and persist them.
///
/// Drafts are normally written at detection time, but only for posts that clear
/// the strong bar — drafting every backfilled post during the first crawl would
/// stall it, and with a local LLM configured it would stall it badly. The outbox
/// bar sits lower than that, so it surfaces cards that were never drafted.
///
/// So they are filled in here, lazily, for what is actually on screen, and
/// written back so it happens once per card rather than once per page view.
/// Shared between the HTML and JSON outboxes on purpose: two implementations of
/// "draft the undrafted" is how one of them ends up not persisting.
pub async fn fill_missing(
    st: &crate::state::AppState,
    items: &mut [crate::model::Candidate],
    live: &Settings,
) {
    let name = st.cfg.profile.name.clone();
    let email = st.cfg.profile.email.clone();

    for c in items.iter_mut() {
        if c.draft_body.is_some() {
            continue;
        }
        let post = RawPost {
            source: c.source.clone(),
            external_id: c.urn.clone(),
            url: c.url.clone(),
            title: c.title.clone(),
            company: c.company.clone(),
            location: c.location.clone(),
            body: c.body.clone(),
            posted_at: c.posted_at,
            apply: c.apply(),
            synthetic_title: c.source == "linkedin_voyager",
        };
        let (subject, body) = st.drafter.draft(&post, live, &name, &email).await;
        if let Err(e) = crate::db::set_draft(&st.pool, c.id, &subject, &body).await {
            tracing::warn!(id = c.id, %e, "draft backfill failed to persist");
        }
        c.draft_subject = Some(subject);
        c.draft_body = Some(body);
    }
}
