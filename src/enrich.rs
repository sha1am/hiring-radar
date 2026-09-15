//! What a posting actually *is*, as structured facts you can filter on.
//!
//! The tags module already pulls technologies out of the text, which answers
//! "does this mention Go". It does not answer the questions you actually sort
//! by: is this backend or data engineering, is it a senior role or a lead role,
//! how many years do they want, is it remote. Those are stated in prose, in a
//! hundred different phrasings, and a keyword list cannot read them.
//!
//! Two passes, and the order matters:
//!
//! 1. **Heuristics, at ingest, always.** Regex and keyword rules over the title
//!    and body. Instant, free, offline, and good enough that the filters work on
//!    a fresh install with no model configured. Roughly right on most listings.
//! 2. **An LLM, in the background, when one is configured.** Reads the whole
//!    posting and overwrites what it is confident about.
//!
//! Doing it the other way round — LLM only — would mean every filter is empty
//! until a model answers, the crawl stalls behind inference, and a machine with
//! no Ollama gets no filters at all. Heuristics first means the feature degrades
//! to "slightly coarser" rather than to "gone".
//!
//! The LLM is never trusted blindly. It returns JSON, every field is validated
//! against a closed vocabulary, and anything unrecognised is dropped rather than
//! stored — an invented role like "backend-ish" would become a filter chip that
//! matches one row and confuses you forever.

use crate::config::DraftCfg;
use crate::model::RawPost;
use serde::{Deserialize, Serialize};

/// The structured reading of one posting. Every field optional: a listing that
/// doesn't say is very different from one that says "mid-level", and guessing
/// would put rows behind filters they don't belong to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Facts {
    /// What kind of engineering. See [`ROLES`].
    pub role: Option<String>,
    /// How senior. See [`LEVELS`].
    pub level: Option<String>,
    /// Years of experience asked for. `years_max` is None for "5+".
    pub years_min: Option<i64>,
    pub years_max: Option<i64>,
    /// remote | hybrid | onsite
    pub work_mode: Option<String>,
    /// full-time | contract | internship
    pub employment: Option<String>,
    /// Technologies, canonical labels — the same vocabulary as the chips.
    pub stack: Vec<String>,
}

/// The closed vocabulary for `role`. Closed on purpose: filters are only useful
/// if the same job lands under the same label every time, and a free-text role
/// produces forty spellings of "backend".
pub const ROLES: &[&str] = &[
    "backend", "frontend", "fullstack", "mobile", "devops", "sre", "data",
    "ml", "security", "qa", "embedded", "engineering-manager",
];

/// The closed vocabulary for `level`.
pub const LEVELS: &[&str] = &["intern", "junior", "mid", "senior", "staff", "principal", "manager"];

pub const WORK_MODES: &[&str] = &["remote", "hybrid", "onsite"];
pub const EMPLOYMENT: &[&str] = &["full-time", "contract", "internship"];

fn in_vocab(v: &Option<String>, vocab: &[&str]) -> Option<String> {
    let s = v.as_deref()?.trim().to_lowercase();
    vocab.iter().find(|x| **x == s).map(|x| (*x).to_string())
}

impl Facts {
    /// Drop anything outside the closed vocabularies, and make the year range
    /// sane. Applied to LLM output before it is stored, and to heuristics too —
    /// one place where the invariants hold.
    pub fn sanitize(&mut self) {
        self.role = in_vocab(&self.role, ROLES);
        self.level = in_vocab(&self.level, LEVELS);
        self.work_mode = in_vocab(&self.work_mode, WORK_MODES);
        self.employment = in_vocab(&self.employment, EMPLOYMENT);

        // A listing asking for 40 years is a parse error, not a listing.
        if let Some(n) = self.years_min {
            if !(0..=30).contains(&n) {
                self.years_min = None;
            }
        }
        if let Some(n) = self.years_max {
            if !(0..=40).contains(&n) {
                self.years_max = None;
            }
        }
        if let (Some(lo), Some(hi)) = (self.years_min, self.years_max) {
            if hi < lo {
                // "3-2 years" means the parse went wrong; keep the floor, which
                // is the half people actually filter on.
                self.years_max = None;
            }
        }

        self.stack.retain(|t| !t.trim().is_empty());
        self.stack.sort();
        self.stack.dedup();
        self.stack.truncate(20);
    }

    /// Overlay `other` onto self, keeping self where `other` says nothing.
    ///
    /// This is how the LLM pass refines the heuristic pass without being able to
    /// erase it: a model that returns `null` for everything leaves the board
    /// exactly as it was, which is the right outcome for a model having a bad
    /// day.
    pub fn merge_from(&mut self, other: Facts) {
        if other.role.is_some() {
            self.role = other.role;
        }
        if other.level.is_some() {
            self.level = other.level;
        }
        if other.years_min.is_some() {
            self.years_min = other.years_min;
        }
        if other.years_max.is_some() {
            self.years_max = other.years_max;
        }
        if other.work_mode.is_some() {
            self.work_mode = other.work_mode;
        }
        if other.employment.is_some() {
            self.employment = other.employment;
        }
        // The stack is a union: the extractor knows aliases the model won't
        // bother with, and the model reads prose the extractor can't.
        for t in other.stack {
            if !self.stack.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                self.stack.push(t);
            }
        }
        self.sanitize();
    }

    /// "5+ yrs", "3–5 yrs", "≤2 yrs" — for the card, and for a filter's label.
    pub fn years_label(&self) -> Option<String> {
        match (self.years_min, self.years_max) {
            (Some(lo), Some(hi)) if lo == hi => Some(format!("{lo} yrs")),
            (Some(lo), Some(hi)) => Some(format!("{lo}\u{2013}{hi} yrs")),
            (Some(lo), None) => Some(format!("{lo}+ yrs")),
            (None, Some(hi)) => Some(format!("\u{2264}{hi} yrs")),
            (None, None) => None,
        }
    }
}

// ===================== heuristics =====================

/// Role keywords, most specific first — order decides ties.
///
/// "Machine Learning Platform Engineer" is an ML role, not a platform one, and
/// "Senior Backend Engineer, Data Platform" is backend. Whichever rule matches
/// the *title* first wins, because the title is what the employer chose to call
/// it; the body is only consulted when the title says nothing useful.
const ROLE_RULES: &[(&str, &[&str])] = &[
    ("engineering-manager", &["engineering manager", "em, ", "team lead", "tech lead manager"]),
    ("ml", &["machine learning", "ml engineer", "deep learning", "nlp ", "computer vision", "mlops", "ai engineer"]),
    ("data", &["data engineer", "data platform", "analytics engineer", "etl", "data warehouse", "bi engineer"]),
    ("sre", &["site reliability", "sre", "reliability engineer", "production engineer"]),
    // The tool names here only ever fire on the *body* pass, because a titled
    // role already matched on the title. So "Backend Engineer" whose body
    // mentions Kubernetes stays backend, while a bare "Engineer, Platform Team"
    // gets labelled from what it actually describes.
    ("devops", &["devops", "platform engineer", "infrastructure engineer", "cloud engineer", "kubernetes", "terraform", "ci/cd"]),
    ("security", &["security engineer", "appsec", "infosec", "penetration test", "security analyst"]),
    ("qa", &["qa engineer", "test engineer", "sdet", "quality assurance", "automation test"]),
    ("embedded", &["embedded", "firmware", "rtos", "device driver"]),
    ("mobile", &["android", "ios engineer", "mobile engineer", "react native", "flutter"]),
    ("fullstack", &["full stack", "full-stack", "fullstack"]),
    ("frontend", &["frontend", "front-end", "front end", "ui engineer", "web developer"]),
    ("backend", &["backend", "back-end", "back end", "server side", "api engineer", "distributed systems", "microservices"]),
];

const LEVEL_RULES: &[(&str, &[&str])] = &[
    ("intern", &["intern", "internship", "trainee", "apprentice"]),
    ("principal", &["principal", "distinguished", "architect", "fellow"]),
    ("staff", &["staff engineer", "staff software", "sde iii", "sde 3", "sde-3", "l6", "senior staff"]),
    ("manager", &["engineering manager", "people manager", "team manager"]),
    ("senior", &["senior", "sr.", "sr ", "lead ", "sde ii", "sde 2", "sde-2", "l5"]),
    ("junior", &["junior", "jr.", "fresher", "graduate engineer", "entry level", "entry-level", "sde i", "sde 1", "sde-1"]),
];

const REMOTE_WORDS: &[&str] = &["fully remote", "100% remote", "remote-first", "work from home", "wfh", "remote"];
const HYBRID_WORDS: &[&str] = &["hybrid", "days in office", "days a week in", "flexible office"];
const ONSITE_WORDS: &[&str] = &["on-site", "onsite", "in office", "in-office", "office-based"];

/// Disciplines that are not engineering at all.
///
/// Every company on the watchlist publishes its whole payroll through one
/// board: Optum posts nurses, Delhivery posts warehouse staff, and all of them
/// post recruiters. None of these score badly on their own merits, which is the
/// problem — an HR listing names no technology and no engineering discipline,
/// so a scorer that gives a posting the benefit of the doubt on both of those
/// then hands it a passing grade on years, level and location, the three
/// things that are true of you wherever you apply.
///
/// Title only, deliberately. "Our recruiting team is hiring" appears in half
/// the engineering JDs large companies write, and "sales platform" is a thing
/// backend engineers spend careers on.
const OFF_DISCIPLINE: &[(&str, &[&str])] = &[
    ("recruiting", &["recruiter", "recruiting", "recruitment", "talent acquisition", "talent partner",
                     "human resources", "hrbp", "hr business partner", "hr generalist", "hr operations",
                     "hr manager", "people partner", "people operations", "payroll", "sourcer"]),
    ("sales", &["sales", "account executive", "account manager", "business development",
                "inside sales", "key account", "bdr", "sdr"]),
    ("marketing", &["marketing", "seo", "copywriter", "content writer", "social media",
                    "brand manager", "public relations", "communications manager"]),
    ("finance", &["accountant", "accounting", "accounts payable", "accounts receivable",
                  "financial analyst", "controller", "auditor", "audit", "bookkeeper",
                  "taxation", "treasury", "billing specialist", "underwriter", "actuary"]),
    ("legal", &["counsel", "paralegal", "attorney", "compliance officer", "legal manager"]),
    ("support", &["customer support", "customer service", "customer success", "help desk",
                  "service desk", "call center", "call centre", "telecaller", "collections executive"]),
    ("operations", &["delivery executive", "delivery driver", "truck driver", "rider", "warehouse",
                     "logistics executive", "store manager", "field executive", "field officer",
                     "housekeeping", "security guard", "technician", "procurement",
                     "operations executive", "operations associate", "branch manager"]),
    ("clinical", &["nurse", "nursing", "physician", "clinical", "pharmacist", "therapist",
                   "medical coder", "radiologist", "dentist", "caregiver", "phlebotomist"]),
    ("design", &["graphic designer", "visual designer", "ux designer", "ui designer",
                 "product designer", "ux researcher", "illustrator", "motion designer"]),
    ("product", &["product manager", "program manager", "project manager", "scrum master",
                  "product owner", "business analyst", "delivery manager"]),
    ("teaching", &["teacher", "instructor", "tutor", "faculty", "professor", "trainer", "curriculum"]),
    ("consulting", &["business consultant", "management consultant", "strategy consultant",
                     "research associate"]),
];

/// Words that mean the posting is for an engineer whatever else the title says.
///
/// Matched as whole words, and that is the entire trick: "engineering" is not
/// "engineer", so "Engineering Recruiter" stays a recruiting job while
/// "Software Engineer, Sales Platform" stays an engineering one.
const ENGINEERING_WORDS: &[&str] = &[
    "engineer", "engineers", "developer", "developers", "programmer", "sde", "sdet", "sre",
    "devops", "architect", "scientist", "technologist", "coder", "hacker",
];

/// Whether a title says "engineer" in one of the ways titles say it.
///
/// Used as a guard, not as a filter: it decides whether a discipline word in a
/// title is describing the job or describing the team the job is on.
pub fn engineering_title(title: &str) -> bool {
    let t = title.to_lowercase();
    ENGINEERING_WORDS
        .iter()
        .any(|w| crate::text::contains_word(&t, w))
}

/// The non-engineering discipline this title belongs to, if any.
///
/// `None` means either "this is engineering" or "this is something I have no
/// rule for" — the caller must not read it as an endorsement.
pub fn off_discipline(title: &str) -> Option<&'static str> {
    if engineering_title(title) {
        return None;
    }
    let t = title.to_lowercase();
    OFF_DISCIPLINE
        .iter()
        .find(|(_, words)| words.iter().any(|w| crate::text::contains_word(&t, w)))
        .map(|(label, _)| *label)
}

/// The role rules, for anything that needs to ask the same question of a
/// different document — the ATS reads a resume with exactly these.
pub fn role_rules() -> &'static [(&'static str, &'static [&'static str])] {
    ROLE_RULES
}

/// Does this text describe the given role? Used to read a resume's own
/// discipline with the same rules a posting is read with, so the two sides of
/// the comparison cannot disagree about what "backend" means.
pub fn role_matches(hay: &str, role: &str) -> bool {
    ROLE_RULES
        .iter()
        .find(|(r, _)| *r == role)
        .map(|(_, words)| contains_any(hay, words))
        .unwrap_or(false)
}

/// How many times a role's vocabulary appears. Used to rank the disciplines a
/// resume demonstrates: "does the word appear" cannot tell one mention in a
/// project title from nine bullet points of the same work.
pub fn role_evidence(hay: &str, role: &str) -> usize {
    ROLE_RULES
        .iter()
        .find(|(r, _)| *r == role)
        .map(|(_, words)| words.iter().map(|w| hay.matches(w).count()).sum())
        .unwrap_or(0)
}

pub fn level_matches(hay: &str, level: &str) -> bool {
    LEVEL_RULES
        .iter()
        .find(|(l, _)| *l == level)
        .map(|(_, words)| contains_any(hay, words))
        .unwrap_or(false)
}

/// Read what can be read without a model.
pub fn heuristic(post: &RawPost) -> Facts {
    let title = post.title.to_lowercase();
    let body = post.body.to_lowercase();
    let hay = format!(" {title} {body} ");

    // Title first: it is what the employer chose to call the job. The body
    // mentions every adjacent discipline and would tag a backend role as "data"
    // for saying the word "pipeline".
    let role = first_match(&format!(" {title} "), ROLE_RULES)
        .or_else(|| first_match(&hay, ROLE_RULES));

    // Level, on the other hand, is only trustworthy from the title — a body
    // saying "you'll work with senior engineers" is not a senior req.
    let level = first_match(&format!(" {title} "), LEVEL_RULES);

    let (years_min, years_max) = years(&hay);

    let work_mode = if contains_any(&hay, HYBRID_WORDS) {
        // Checked before remote: "hybrid — 2 days remote" is hybrid, and the
        // word "remote" appears in most hybrid descriptions.
        Some("hybrid".into())
    } else if contains_any(&hay, REMOTE_WORDS) {
        Some("remote".into())
    } else if contains_any(&hay, ONSITE_WORDS) {
        Some("onsite".into())
    } else {
        None
    };

    let employment = if hay.contains("internship") || hay.contains(" intern ") {
        Some("internship".into())
    } else if hay.contains("contract") || hay.contains("contractor") || hay.contains("freelance") {
        Some("contract".into())
    } else if hay.contains("full-time") || hay.contains("full time") || hay.contains("permanent") {
        Some("full-time".into())
    } else {
        None
    };

    let mut f = Facts {
        role,
        level,
        years_min,
        years_max,
        work_mode,
        employment,
        stack: crate::tags::extract(&post.haystack()),
    };
    f.sanitize();
    f
}

fn contains_any(hay: &str, words: &[&str]) -> bool {
    words.iter().any(|w| hay.contains(w))
}

fn first_match(hay: &str, rules: &[(&str, &[&str])]) -> Option<String> {
    rules
        .iter()
        .find(|(_, words)| contains_any(hay, words))
        .map(|(label, _)| (*label).to_string())
}

/// Years of experience, from the phrasings listings actually use.
///
/// "5+ years", "3-5 years", "minimum 4 years", "at least 2 years", "2 to 4
/// years". Deliberately narrow: the number has to sit next to the word "year",
/// because a body full of "2024" and "10x" will otherwise hand you a range from
/// nowhere. A wrong number here is worse than none — it puts the row behind a
/// filter it does not belong to.
pub fn years(hay: &str) -> (Option<i64>, Option<i64>) {
    let bytes: Vec<char> = hay.chars().collect();
    let mut best: Option<(i64, Option<i64>)> = None;

    for (i, w) in hay.match_indices("year") {
        // Look back a short window — long enough for "at least 3 to 5 ", short
        // enough not to reach the previous sentence.
        let start = i.saturating_sub(24);
        let window: String = hay[start..i].to_string();
        let _ = (w, &bytes);

        let nums: Vec<i64> = window
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<i64>().ok())
            .filter(|n| (0..=30).contains(n))
            .collect();
        if nums.is_empty() {
            continue;
        }

        let plus = window.contains('+')
            || window.contains("at least")
            || window.contains("minimum")
            || window.contains("min ")
            || window.contains("or more");

        let found = if nums.len() >= 2 && !plus {
            // "3-5 years", "2 to 4 years"
            (nums[nums.len() - 2], Some(nums[nums.len() - 1]))
        } else {
            (*nums.last().unwrap(), None)
        };

        // Several mentions: keep the highest floor. A listing saying "2+ years
        // of Python, 5+ years overall" is a five-year job.
        best = Some(match best {
            Some(b) if b.0 >= found.0 => b,
            _ => found,
        });
    }

    match best {
        Some((lo, hi)) => (Some(lo), hi),
        None => (None, None),
    }
}

// ===================== the LLM pass =====================

#[async_trait::async_trait]
pub trait Enricher: Send + Sync {
    /// Read the posting. `None` means "no opinion" — the heuristics stand.
    async fn read(&self, post: &RawPost) -> Option<Facts>;
    /// Whether this is worth running a background pass for at all.
    fn is_llm(&self) -> bool {
        false
    }
}

/// No model configured. The heuristics already ran at ingest; there is nothing
/// to add, and saying so explicitly is better than a background worker that
/// spins on rows it can never improve.
pub struct NoEnricher;

#[async_trait::async_trait]
impl Enricher for NoEnricher {
    async fn read(&self, _post: &RawPost) -> Option<Facts> {
        None
    }
}

pub struct OllamaEnricher {
    pub client: reqwest::Client,
    pub url: String,
    pub model: String,
}

/// What the model is asked for, and the exact words of the vocabulary it must
/// choose from. Sent as JSON mode so the reply is parseable rather than prose.
fn prompt(post: &RawPost) -> String {
    format!(
        "Read this job posting and extract facts. Reply with JSON only, no prose.\n\n\
         Schema — use null for anything the posting does not state. Do not guess.\n\
         {{\n  \"role\": one of [{roles}] or null,\n  \
         \"level\": one of [{levels}] or null,\n  \
         \"years_min\": integer or null,   // years of experience required, the lower bound\n  \
         \"years_max\": integer or null,   // upper bound; null for \"5+\"\n  \
         \"work_mode\": one of [{modes}] or null,\n  \
         \"employment\": one of [{emp}] or null,\n  \
         \"stack\": [\"Go\", \"Postgres\"]  // named technologies only, [] if none\n}}\n\n\
         Rules: pick the single best role for the whole job, not every discipline \
         mentioned. Level is the seniority of THIS role, not of the team. Years \
         must be stated in the posting; if it only says \"experienced\", use null.\n\n\
         Title: {title}\nCompany: {company}\nLocation: {location}\n\nPosting:\n{body}\n\nJSON:",
        roles = ROLES.join(", "),
        levels = LEVELS.join(", "),
        modes = WORK_MODES.join(", "),
        emp = EMPLOYMENT.join(", "),
        title = post.title,
        company = post.company,
        location = post.location.as_deref().unwrap_or("not stated"),
        body = post.body.chars().take(4000).collect::<String>(),
    )
}

/// The model's reply. Every field is `Option` and the whole struct tolerates
/// junk, because a local model *will* occasionally return a string where a
/// number belongs, and one bad reply must not poison a whole batch.
#[derive(Deserialize, Default)]
struct Reply {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default, deserialize_with = "lenient_int")]
    years_min: Option<i64>,
    #[serde(default, deserialize_with = "lenient_int")]
    years_max: Option<i64>,
    #[serde(default)]
    work_mode: Option<String>,
    #[serde(default)]
    employment: Option<String>,
    #[serde(default)]
    stack: Vec<String>,
}

/// Accept 5, "5", "5+" and null where an integer belongs.
fn lenient_int<'de, D>(d: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok(),
        _ => None,
    })
}

#[async_trait::async_trait]
impl Enricher for OllamaEnricher {
    fn is_llm(&self) -> bool {
        true
    }

    async fn read(&self, post: &RawPost) -> Option<Facts> {
        let req = serde_json::json!({
            "model": self.model,
            "prompt": prompt(post),
            "format": "json",
            "stream": false,
            // Extraction, not writing. Temperature 0 so the same posting reads
            // the same way twice — a filter whose contents shuffle between
            // crawls is worse than no filter.
            "options": { "temperature": 0 },
        });

        let text = async {
            let resp = self
                .client
                .post(format!("{}/api/generate", self.url))
                .json(&req)
                .send()
                .await
                .ok()?;
            let v: serde_json::Value = resp.json().await.ok()?;
            v.get("response")?.as_str().map(str::to_string)
        }
        .await?;

        let reply: Reply = parse_reply(&text)?;
        let mut f = Facts {
            role: reply.role,
            level: reply.level,
            years_min: reply.years_min,
            years_max: reply.years_max,
            work_mode: reply.work_mode,
            employment: reply.employment,
            stack: reply.stack,
        };
        f.sanitize();
        Some(f)
    }
}

/// Pull the JSON object out of a reply, even when the model wrapped it in a
/// code fence or a sentence. Format-json mode usually prevents that; usually is
/// not always, and the fallback costs three lines.
fn parse_reply(text: &str) -> Option<Reply> {
    if let Ok(r) = serde_json::from_str::<Reply>(text.trim()) {
        return Some(r);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Reply>(&text[start..=end]).ok()
}

pub fn build_enricher(cfg: &DraftCfg, client: reqwest::Client) -> Box<dyn Enricher> {
    match cfg.provider.as_str() {
        "ollama" => Box::new(OllamaEnricher {
            client,
            url: cfg.ollama_url.clone(),
            model: cfg.ollama_model.clone(),
        }),
        _ => Box::new(NoEnricher),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ApplyChannel;

    fn post(title: &str, body: &str) -> RawPost {
        RawPost {
            source: "greenhouse".into(),
            external_id: "x".into(),
            url: "https://e.com".into(),
            title: title.into(),
            company: "Acme".into(),
            location: Some("Bengaluru, India".into()),
            body: body.into(),
            posted_at: None,
            apply: ApplyChannel::Unknown,
            synthetic_title: false,
        }
    }

    #[test]
    fn reads_the_common_year_phrasings() {
        assert_eq!(years("we want 5+ years of backend"), (Some(5), None));
        assert_eq!(years("3-5 years of experience"), (Some(3), Some(5)));
        assert_eq!(years("2 to 4 years building services"), (Some(2), Some(4)));
        assert_eq!(years("minimum of 4 years"), (Some(4), None));
        assert_eq!(years("at least 6 years"), (Some(6), None));
    }

    #[test]
    fn a_number_not_next_to_year_is_not_a_year() {
        // A body full of "2024" and "10x engineers" must not produce a range.
        assert_eq!(years("founded in 2019, we are a 10x team"), (None, None));
        assert_eq!(years("experienced engineers welcome"), (None, None));
    }

    #[test]
    fn the_highest_floor_wins_when_several_are_stated() {
        // "2+ years of Python, 5+ years overall" is a five-year job.
        assert_eq!(years("2+ years of python and 5+ years overall"), (Some(5), None));
    }

    #[test]
    fn the_title_decides_the_role_not_the_body() {
        // A backend job that says "pipeline" is not a data job.
        let f = heuristic(&post(
            "Senior Backend Engineer",
            "You will build data pipelines and ETL for our analytics team.",
        ));
        assert_eq!(f.role.as_deref(), Some("backend"));
    }

    #[test]
    fn the_body_is_used_when_the_title_says_nothing() {
        let f = heuristic(&post("Engineer, Platform Team", "Kubernetes, Terraform, CI/CD."));
        assert_eq!(f.role.as_deref(), Some("devops"));
    }

    #[test]
    fn level_comes_only_from_the_title() {
        // "you'll work with senior engineers" is not a senior req.
        let f = heuristic(&post("Backend Engineer", "You will work with senior engineers."));
        assert_eq!(f.level, None);
        assert_eq!(heuristic(&post("Staff Engineer, Infra", "")).level.as_deref(), Some("staff"));
    }

    #[test]
    fn hybrid_beats_remote_because_hybrid_posts_say_remote() {
        let f = heuristic(&post("Backend Engineer", "Hybrid — 3 days in office, 2 days remote."));
        assert_eq!(f.work_mode.as_deref(), Some("hybrid"));
    }

    #[test]
    fn an_invented_role_is_dropped_rather_than_stored() {
        // An LLM returning "backend-ish" would otherwise become a chip that
        // matches one row and confuses you forever.
        let mut f = Facts {
            role: Some("backend-ish".into()),
            level: Some("very senior".into()),
            years_min: Some(99),
            ..Default::default()
        };
        f.sanitize();
        assert_eq!(f.role, None);
        assert_eq!(f.level, None);
        assert_eq!(f.years_min, None);
    }

    #[test]
    fn a_model_with_no_opinion_cannot_erase_the_heuristics() {
        let mut base = heuristic(&post("Senior Backend Engineer", "5+ years of Go."));
        let before = base.clone();
        base.merge_from(Facts::default());
        assert_eq!(base.role, before.role);
        assert_eq!(base.level, before.level);
        assert_eq!(base.years_min, before.years_min);
    }

    #[test]
    fn the_model_refines_what_it_does_answer() {
        let mut base = heuristic(&post("Engineer", "Build things."));
        assert_eq!(base.role, None);
        base.merge_from(Facts {
            role: Some("data".into()),
            years_min: Some(4),
            ..Default::default()
        });
        assert_eq!(base.role.as_deref(), Some("data"));
        assert_eq!(base.years_min, Some(4));
    }

    #[test]
    fn stacks_union_rather_than_replace() {
        // The extractor knows aliases the model won't bother with; the model
        // reads prose the extractor can't.
        let mut base = Facts {
            stack: vec!["Go".into(), "Postgres".into()],
            ..Default::default()
        };
        base.merge_from(Facts {
            stack: vec!["Postgres".into(), "Temporal".into()],
            ..Default::default()
        });
        assert_eq!(base.stack, vec!["Go", "Postgres", "Temporal"]);
    }

    #[test]
    fn a_fenced_or_chatty_reply_still_parses() {
        let r = parse_reply("Here you go:\n```json\n{\"role\":\"backend\",\"years_min\":\"5+\"}\n```")
            .expect("should recover the object");
        assert_eq!(r.role.as_deref(), Some("backend"));
        assert_eq!(r.years_min, Some(5));
    }

    #[test]
    fn garbage_is_none_not_a_panic() {
        assert!(parse_reply("I'm sorry, I can't do that").is_none());
        assert!(parse_reply("").is_none());
    }

    #[test]
    fn year_labels_read_the_way_a_listing_does() {
        let f = |lo, hi| Facts { years_min: lo, years_max: hi, ..Default::default() }.years_label();
        assert_eq!(f(Some(5), None).as_deref(), Some("5+ yrs"));
        assert_eq!(f(Some(3), Some(5)).as_deref(), Some("3\u{2013}5 yrs"));
        assert_eq!(f(None, None), None);
    }

    #[test]
    fn the_jobs_that_arrive_with_the_engineering_ones_are_named() {
        // One board per company means the whole payroll comes through it.
        assert_eq!(off_discipline("Senior HR Business Partner"), Some("recruiting"));
        assert_eq!(off_discipline("Talent Acquisition Specialist"), Some("recruiting"));
        assert_eq!(off_discipline("Account Executive, Enterprise"), Some("sales"));
        assert_eq!(off_discipline("Registered Nurse - ICU"), Some("clinical"));
        assert_eq!(off_discipline("Technical Program Manager"), Some("product"));
        assert_eq!(off_discipline("Warehouse Associate"), Some("operations"));
    }

    #[test]
    fn a_discipline_word_about_the_team_is_not_the_job() {
        // The whole reason the guard matches whole words: half of what an
        // engineer is hired to build is named after another department.
        assert_eq!(off_discipline("Software Engineer, Sales Platform"), None);
        assert_eq!(off_discipline("Backend Developer - Marketing Technology"), None);
        assert_eq!(off_discipline("Data Scientist, Clinical Research"), None);
        assert_eq!(off_discipline("Staff Engineer, Payments & Billing"), None);
    }

    #[test]
    fn engineering_recruiter_is_still_a_recruiter() {
        // "engineering" is not "engineer", and this is the case that decides it.
        assert_eq!(off_discipline("Engineering Recruiter"), Some("recruiting"));
        assert_eq!(off_discipline("Technical Recruiter, Engineering"), Some("recruiting"));
    }

    #[test]
    fn a_title_with_no_rule_at_all_is_not_an_endorsement() {
        // None means "no rule matched", never "this one is for you".
        assert_eq!(off_discipline("Member of Technical Staff"), None);
        assert!(!engineering_title("Member of Technical Staff"));
        assert!(engineering_title("Senior Software Engineer II"));
    }
}
