use crate::config::{DraftCfg, Profile};
use crate::model::{ApplyChannel, RawPost};
use serde_json::json;

/// Produces (subject, body). Subject is only meaningful for the email channel.
/// Drafts are generated speculatively at detection time for strong+ posts, so a
/// finished draft is already waiting the instant a card hits the dashboard.
#[async_trait::async_trait]
pub trait Drafter: Send + Sync {
    async fn draft(&self, post: &RawPost, p: &Profile) -> (String, String);
}

/// Instant, free, offline. Shapes the message to the post's apply channel.
pub struct TemplateDrafter;

#[async_trait::async_trait]
impl Drafter for TemplateDrafter {
    async fn draft(&self, post: &RawPost, p: &Profile) -> (String, String) {
        let subject = format!("Application: {} at {}", post.title, post.company);
        let skills = p.keywords.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
        let body = match &post.apply {
            ApplyChannel::LinkedInDm { .. } => format!(
                "Hi — saw your post about the {} role. I'm a backend engineer with \
                 experience in {}, and it looks like a strong fit. Would love to share \
                 my background — open to a quick chat?\n\n— {}",
                post.title, skills, p.name
            ),
            _ => format!(
                "Hi,\n\nI came across the {} opening at {} and I'm very interested. \
                 I work as a backend engineer with hands-on experience in {}, and I \
                 believe I'd contribute quickly to your team.\n\nI've attached my resume \
                 and would welcome the chance to discuss the role.\n\nBest regards,\n{}\n{}",
                post.title, post.company, skills, p.name, p.email
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
    async fn draft(&self, post: &RawPost, p: &Profile) -> (String, String) {
        let channel = post.apply.kind();
        let prompt = format!(
            "You are helping {name} apply to a job. Write a short, specific, \
             non-generic outreach message for this {channel} channel. No fluff, \
             no placeholders, 90 words max. End with the name only.\n\n\
             Candidate skills: {skills}\n\
             Role: {title}\nCompany: {company}\n\
             Post:\n{body}\n\nMessage:",
            name = p.name,
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
                self.fallback.draft(post, p).await
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
