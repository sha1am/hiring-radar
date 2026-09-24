//! One small client for every hosted model worth using.
//!
//! They all speak the same shape — `POST /chat/completions`, messages in, a
//! string out — so this is deliberately one file and one function rather than a
//! provider abstraction. Point `base_url` at api.openai.com, at OpenRouter, at
//! Groq, or at Ollama's own `/v1`, and the only thing that changes is the model
//! name.
//!
//! Two decisions worth stating.
//!
//! **JSON mode, not a JSON schema.** `response_format: json_object` is
//! understood everywhere; strict `json_schema` is not, and the parser has to
//! tolerate a bad reply regardless — a model that returns a string where a
//! number belongs is a Tuesday, and one bad reply must never poison a batch.
//! The schema lives in the prompt and the validation lives in Rust.
//!
//! **Temperature is only sent when asked for.** The reasoning models reject it
//! outright, and a request that 400s because of a parameter nobody set is a bad
//! default.

use crate::config::LlmCfg;
use serde::Deserialize;

pub struct Client {
    http: reqwest::Client,
    cfg: LlmCfg,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    message: Message,
}

#[derive(Deserialize, Default)]
struct Message {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize, Default, Clone, Copy, Debug)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: i64,
    #[serde(default)]
    pub completion_tokens: i64,
}

/// What came back, with the accounting attached.
///
/// The token counts are kept because this is the one part of the system that
/// costs money per posting, and "why was the bill what it was" is a question
/// you can only answer if you wrote the numbers down.
pub struct Answer {
    pub content: String,
    pub usage: Usage,
    pub model: String,
}

impl Client {
    pub fn new(http: reqwest::Client, cfg: LlmCfg) -> Self {
        Self { http, cfg }
    }

    pub fn model(&self) -> &str {
        &self.cfg.model
    }

    /// Ask for one JSON object back.
    ///
    /// Retries twice, and only on the failures that are worth retrying: a rate
    /// limit, a gateway hiccup, a dropped connection. A 400 means the request
    /// is wrong and sending it again just spends money on the same mistake.
    pub async fn json(&self, system: &str, user: &str) -> anyhow::Result<Answer> {
        let mut body = serde_json::json!({
            "model": self.cfg.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "response_format": {"type": "json_object"},
        });
        if let Some(t) = self.cfg.temperature {
            body["temperature"] = serde_json::json!(t);
        }

        let url = format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        );
        let mut last = String::new();

        for attempt in 0..3u32 {
            if attempt > 0 {
                // 2s, then 6s. Enough for a rate limit window to move on,
                // short enough that a backlog still drains.
                let wait = 2u64 * 3u64.pow(attempt - 1);
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }

            let mut req = self.http.post(&url).json(&body);
            if let Some(key) = self.cfg.api_key.as_deref() {
                req = req.bearer_auth(key);
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last = format!("unreachable — {}", crate::sources::common::brief(&e));
                    continue;
                }
            };

            let status = resp.status();
            if status.is_success() {
                let parsed: ChatResponse = resp
                    .json()
                    .await
                    .map_err(|e| anyhow::anyhow!("unreadable reply — {e}"))?;
                let content = parsed
                    .choices
                    .first()
                    .map(|c| c.message.content.clone())
                    .unwrap_or_default();
                if content.trim().is_empty() {
                    anyhow::bail!("the model returned nothing");
                }
                return Ok(Answer {
                    content,
                    usage: parsed.usage.unwrap_or_default(),
                    model: self.cfg.model.clone(),
                });
            }

            // The body of an error is where the useful sentence is: "model not
            // found", "insufficient quota", "unsupported parameter".
            let detail = resp.text().await.unwrap_or_default();
            let detail = detail.chars().take(300).collect::<String>();
            last = format!("HTTP {} — {}", status.as_u16(), detail.trim());

            let retryable = status.as_u16() == 429 || status.is_server_error();
            if !retryable {
                break;
            }
        }
        anyhow::bail!("{last}")
    }
}

/// Pull the first JSON object out of a reply.
///
/// JSON mode is a request, not a guarantee: a model that has been told to
/// answer in JSON will still occasionally wrap it in a ```json fence or
/// apologise first. Recovering the object costs three lines and saves a
/// posting.
pub fn first_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fenced_reply_is_still_an_object() {
        let r = first_object("Sure!\n```json\n{\"role\":\"backend\"}\n```\n").unwrap();
        assert_eq!(r, "{\"role\":\"backend\"}");
    }

    #[test]
    fn prose_with_no_object_is_none() {
        assert!(first_object("I'm sorry, I can't do that.").is_none());
        assert!(first_object("").is_none());
    }

    #[test]
    fn nested_objects_survive() {
        let r = first_object("{\"a\":{\"b\":1}}").unwrap();
        assert_eq!(r, "{\"a\":{\"b\":1}}");
    }
}
