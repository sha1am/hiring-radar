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
        let text = std::fs::read_to_string(path)?;
        let mut cfg: Config = toml::from_str(&text)?;

        // Secrets never live in the file; pull them from the environment.
        cfg.email.smtp_password = std::env::var("SMTP_PASSWORD").ok();
        cfg.linkedin_voyager.li_at = std::env::var("LI_AT").ok();
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
