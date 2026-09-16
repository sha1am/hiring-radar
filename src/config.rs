use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub profile: Profile,
    pub crawl: Crawl,
    pub release: ReleaseCfg,
    pub ntfy: Ntfy,
    pub email: EmailCfg,
    pub server: ServerCfg,
    pub draft: DraftCfg,
    /// The model that reads postings. Absent means the section was never added
    /// to config.toml, which is a working configuration: the heuristics run
    /// either way and `build_enricher` falls back to the legacy `[draft]`
    /// ollama settings.
    #[serde(default)]
    pub llm: LlmCfg,
    #[serde(default)]
    pub greenhouse: Greenhouse,
    #[serde(default)]
    pub linkedin_voyager: Voyager,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Profile {
    pub name: String,
    pub email: String,
    pub titles: Vec<String>,
    pub keywords: Vec<String>,
    #[serde(default)]
    pub locations: Vec<String>,
    #[serde(default)]
    pub remote_ok: bool,
    #[serde(default)]
    pub seniority: Vec<String>,
    #[serde(default)]
    pub dealbreakers: Vec<String>,
    #[serde(default)]
    pub min_salary: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Crawl {
    pub user_agent: String,
    pub target_search_secs: u64,
    pub broad_search_secs: u64,
    #[serde(default)]
    pub jitter_secs: u64,
    #[serde(default)]
    pub linkedin_queries: Vec<LinkedInQuery>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LinkedInQuery {
    pub keywords: String,
    pub location: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Greenhouse {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub boards: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Voyager {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub query_id: String,
    #[serde(skip)]
    pub li_at: Option<String>,
    #[serde(skip)]
    pub jsessionid: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReleaseCfg {
    pub per_hour_cap: i64,
    pub per_poster_cap: i64,
    pub tick_secs: u64,
    pub score_floor: f64,
    pub strong_min: f64,
    pub exceptional_min: f64,
    pub settle_strong_secs: i64,
    pub candidate_ttl_secs: i64,
    #[serde(default)]
    pub adaptive_threshold: bool,
    #[serde(default = "def_adaptive_start")]
    pub adaptive_start: f64,
    #[serde(default = "def_adaptive_end")]
    pub adaptive_end: f64,
}
fn def_adaptive_start() -> f64 { 82.0 }
fn def_adaptive_end() -> f64 { 70.0 }

#[derive(Clone, Debug, Deserialize)]
pub struct Ntfy {
    pub server: String,
    pub topic: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EmailCfg {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub from: String,
    #[serde(skip)]
    pub smtp_password: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerCfg {
    pub bind: String,
    pub base_url: String,
    /// Default trailing window for the radar view, in hours. The dashboard can
    /// override per-request; this is what a fresh page load shows.
    #[serde(default = "def_radar_hours")]
    pub radar_hours: i64,
    /// Candidates older than this are deleted outright, except ones you acted
    /// on. Keeps SQLite from growing without bound on a busy board set.
    #[serde(default = "def_prune_days")]
    pub prune_after_days: i64,
}
fn def_radar_hours() -> i64 { 24 }
fn def_prune_days() -> i64 { 30 }

/// Who reads the postings, and where.
///
/// One section rather than a provider-shaped zoo of them, because every hosted
/// model worth using speaks the OpenAI chat-completions shape: point `base_url`
/// at api.openai.com, at OpenRouter, at Groq, or at Ollama's own `/v1`, and the
/// only thing that changes is the model name.
///
/// The key is never in here. It comes from the environment, like every other
/// secret in this project — a key in a mounted config file is a key in your
/// shell history and your backups.
#[derive(Clone, Debug, Deserialize)]
pub struct LlmCfg {
    /// "none" | "openai" | "ollama".
    #[serde(default = "def_llm_provider")]
    pub provider: String,
    #[serde(default = "def_llm_base_url")]
    pub base_url: String,
    #[serde(default = "def_llm_model")]
    pub model: String,
    /// From `OPENAI_API_KEY` (or `RADAR_LLM_API_KEY`). Never from the file.
    #[serde(skip)]
    pub api_key: Option<String>,
    /// How much of a posting to send. Whole JDs run to ten thousand characters
    /// of benefits and boilerplate; the requirements are always near the top.
    #[serde(default = "def_llm_max_chars")]
    pub max_body_chars: usize,
    /// Sent only when set. The newer reasoning models reject it outright, and
    /// a request that 400s because of a parameter nobody asked for is a bad
    /// default.
    #[serde(default)]
    pub temperature: Option<f64>,
}

fn def_llm_provider() -> String { "none".into() }
fn def_llm_base_url() -> String { "https://api.openai.com/v1".into() }
fn def_llm_model() -> String { "gpt-5.4-mini".into() }
fn def_llm_max_chars() -> usize { 6000 }

impl Default for LlmCfg {
    fn default() -> Self {
        Self {
            provider: def_llm_provider(),
            base_url: def_llm_base_url(),
            model: def_llm_model(),
            api_key: None,
            max_body_chars: def_llm_max_chars(),
            temperature: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct DraftCfg {
    pub provider: String,
    #[serde(default)]
    pub ollama_url: String,
    #[serde(default)]
    pub ollama_model: String,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();

        // Docker's classic footgun: a bind mount whose source file doesn't
        // exist on the host is created as a DIRECTORY, so the container starts
        // and dies on an opaque "Is a directory" read error. Name it instead.
        if path.is_dir() {
            anyhow::bail!(
                "{} is a directory, not a file. Docker does this when the bind \
                 mount source is missing — run `cp config.example.toml config.toml` \
                 on the host, remove the directory Docker created, and start again.",
                path.display()
            );
        }
        let text = std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!("cannot read config at {}: {e}", path.display())
        })?;
        let mut cfg: Config = toml::from_str(&text)?;

        // Secrets never live in the file; pull them from the environment.
        cfg.email.smtp_password = std::env::var("SMTP_PASSWORD").ok();
        cfg.linkedin_voyager.li_at = std::env::var("LI_AT").ok();
        cfg.llm.api_key = std::env::var("OPENAI_API_KEY")
            .or_else(|_| std::env::var("RADAR_LLM_API_KEY"))
            .ok()
            .filter(|k| !k.trim().is_empty());
        cfg.linkedin_voyager.jsessionid = std::env::var("LI_JSESSIONID").ok();

        // Deployment-shaped overrides. In a container the process must bind
        // 0.0.0.0 while the *published* port stays loopback-only on the host, so
        // bind and base_url have to be settable without editing the mounted file.
        if let Ok(v) = std::env::var("RADAR_BIND") {
            cfg.server.bind = v;
        }
        if let Ok(v) = std::env::var("RADAR_BASE_URL") {
            cfg.server.base_url = v;
        }

        if cfg.email.smtp_password.is_none() {
            tracing::warn!("SMTP_PASSWORD unset — email alerts and application sends will fail");
        }
        Ok(cfg)
    }
}
