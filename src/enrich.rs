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
    /// What the posting says you must have, in its own words.
    ///
    /// Separate from `stack` because the difference between "5 years of Go" and
    /// "exposure to Go a plus" is the whole question, and a flat list of
    /// technologies cannot express it. Only a model reading prose can tell them
    /// apart, so these are empty until one has.
    pub must_have: Vec<String>,
    /// What it would like you to have. Worth points, never worth a rejection.
    pub nice_to_have: Vec<String>,
    /// What you would actually be doing, a few short phrases.
    pub responsibilities: Vec<String>,
    /// The business the work is in. See [`DOMAINS`].
    pub domain: Option<String>,
}

/// The closed vocabulary for `domain`. Closed for the same reason `ROLES` is:
/// a free-text industry produces forty spellings of "fintech".
pub const DOMAINS: &[&str] = &[
    "fintech",
    "ecommerce",
    "logistics",
    "healthtech",
    "edtech",
    "gaming",
    "adtech",
    "devtools",
    "infrastructure",
    "data",
    "security",
    "social",
    "travel",
    "mobility",
    "enterprise",
    "consulting",
    "other",
];

/// The closed vocabulary for `role`. Closed on purpose: filters are only useful
/// if the same job lands under the same label every time, and a free-text role
/// produces forty spellings of "backend".
pub const ROLES: &[&str] = &[
    "backend",
    "frontend",
    "fullstack",
    "mobile",
    "devops",
    "sre",
    "data",
    "ml",
    "security",
    "qa",
    "embedded",
    "engineering-manager",
];

/// The closed vocabulary for `level`.
pub const LEVELS: &[&str] = &[
    "intern",
    "junior",
    "mid",
    "senior",
    "staff",
    "principal",
    "manager",
];

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
        self.domain = in_vocab(&self.domain, DOMAINS);
        self.work_mode = in_vocab(&self.work_mode, WORK_MODES);
        // A model asked for a list will occasionally return an essay in one
        // element. These are shown on a card and matched as phrases; neither
        // survives a paragraph.
        for list in [
            &mut self.must_have,
            &mut self.nice_to_have,
            &mut self.responsibilities,
        ] {
            for item in list.iter_mut() {
                *item = item.trim().trim_matches('.').chars().take(80).collect();
            }
            list.retain(|i| !i.is_empty());
            list.truncate(12);
        }
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
        if other.domain.is_some() {
            self.domain = other.domain;
        }
        // The stack is a union: the extractor knows aliases the model won't
        // bother with, and the model reads prose the extractor can't.
        for t in other.stack {
            if !self.stack.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                self.stack.push(t);
            }
        }
        // These three only ever come from a model, so there is nothing to
        // merge with: a non-empty answer replaces whatever was there, and an
        // empty one leaves the last good reading alone.
        if !other.must_have.is_empty() {
            self.must_have = other.must_have;
        }
        if !other.nice_to_have.is_empty() {
            self.nice_to_have = other.nice_to_have;
        }
        if !other.responsibilities.is_empty() {
            self.responsibilities = other.responsibilities;
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
    (
        "engineering-manager",
        &[
            "engineering manager",
            "em, ",
            "team lead",
            "tech lead manager",
        ],
    ),
    (
        "ml",
        &[
            "machine learning",
            "ml engineer",
            "deep learning",
            "nlp ",
            "computer vision",
            "mlops",
            "ai engineer",
        ],
    ),
    (
        "data",
        &[
            "data engineer",
            "data platform",
            "analytics engineer",
            "etl",
            "data warehouse",
            "bi engineer",
        ],
    ),
    (
        "sre",
        &[
            "site reliability",
            "sre",
            "reliability engineer",
            "production engineer",
        ],
    ),
    // The tool names here only ever fire on the *body* pass, because a titled
    // role already matched on the title. So "Backend Engineer" whose body
    // mentions Kubernetes stays backend, while a bare "Engineer, Platform Team"
    // gets labelled from what it actually describes.
    (
        "devops",
        &[
            "devops",
            "platform engineer",
            "infrastructure engineer",
            "cloud engineer",
            "kubernetes",
            "terraform",
            "ci/cd",
        ],
    ),
    (
        "security",
        &[
            "security engineer",
            "appsec",
            "infosec",
            "penetration test",
            "security analyst",
        ],
    ),
    (
        "qa",
        &[
            "qa engineer",
            "test engineer",
            "sdet",
            "quality assurance",
            "automation test",
        ],
    ),
    (
        "embedded",
        &["embedded", "firmware", "rtos", "device driver"],
    ),
    (
        "mobile",
        &[
            "android",
            "ios engineer",
            "mobile engineer",
            "react native",
            "flutter",
        ],
    ),
    ("fullstack", &["full stack", "full-stack", "fullstack"]),
    (
        "frontend",
        &[
            "frontend",
            "front-end",
            "front end",
            "ui engineer",
            "web developer",
        ],
    ),
    (
        "backend",
        &[
            "backend",
            "back-end",
            "back end",
            "server side",
            "api engineer",
            "distributed systems",
            "microservices",
        ],
    ),
];

const LEVEL_RULES: &[(&str, &[&str])] = &[
    ("intern", &["intern", "internship", "trainee", "apprentice"]),
    (
        "principal",
        &["principal", "distinguished", "architect", "fellow"],
    ),
    (
        "staff",
        &[
            "staff engineer",
            "staff software",
            "sde iii",
            "sde 3",
            "sde-3",
            "l6",
            "senior staff",
        ],
    ),
    (
        "manager",
        &["engineering manager", "people manager", "team manager"],
    ),
    (
        "senior",
        &[
            "senior", "sr.", "sr ", "lead ", "sde ii", "sde 2", "sde-2", "l5",
        ],
    ),
    (
        "junior",
        &[
            "junior",
            "jr.",
            "fresher",
            "graduate engineer",
            "entry level",
            "entry-level",
            "sde i",
            "sde 1",
            "sde-1",
        ],
    ),
];

const REMOTE_WORDS: &[&str] = &[
    "fully remote",
    "100% remote",
    "remote-first",
    "work from home",
    "wfh",
    "remote",
];
const HYBRID_WORDS: &[&str] = &[
    "hybrid",
    "days in office",
    "days a week in",
    "flexible office",
];
const ONSITE_WORDS: &[&str] = &[
    "on-site",
    "onsite",
    "in office",
    "in-office",
    "office-based",
];

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
    (
        "recruiting",
        &[
            "recruiter",
            "recruiting",
            "recruitment",
            "talent acquisition",
            "talent partner",
            "human resources",
            "hrbp",
            "hr business partner",
            "hr generalist",
            "hr operations",
            "hr manager",
            "people partner",
            "people operations",
            "payroll",
            "sourcer",
        ],
    ),
    (
        "sales",
        &[
            "sales",
            "account executive",
            "account manager",
            "business development",
            "inside sales",
            "key account",
            "bdr",
            "sdr",
        ],
    ),
    (
        "marketing",
        &[
            "marketing",
            "seo",
            "copywriter",
            "content writer",
            "social media",
            "brand manager",
            "public relations",
            "communications manager",
        ],
    ),
    (
        "finance",
        &[
            "accountant",
            "accounting",
            "accounts payable",
            "accounts receivable",
            "financial analyst",
            "controller",
            "auditor",
            "audit",
            "bookkeeper",
            "taxation",
            "treasury",
            "billing specialist",
            "underwriter",
            "actuary",
        ],
    ),
    (
        "legal",
        &[
            "counsel",
            "paralegal",
            "attorney",
            "compliance officer",
            "legal manager",
        ],
    ),
    (
        "support",
        &[
            "customer support",
            "customer service",
            "customer success",
            "help desk",
            "service desk",
            "call center",
            "call centre",
            "telecaller",
            "collections executive",
        ],
    ),
    (
        "operations",
        &[
            "delivery executive",
            "delivery driver",
            "truck driver",
            "rider",
            "warehouse",
            "logistics executive",
            "store manager",
            "field executive",
            "field officer",
            "housekeeping",
            "security guard",
            "technician",
            "procurement",
            "operations executive",
            "operations associate",
            "branch manager",
        ],
    ),
    (
        "clinical",
        &[
            "nurse",
            "nursing",
            "physician",
            "clinical",
            "pharmacist",
            "therapist",
            "medical coder",
            "radiologist",
            "dentist",
            "caregiver",
            "phlebotomist",
        ],
    ),
    (
        "design",
        &[
            "graphic designer",
            "visual designer",
            "ux designer",
            "ui designer",
            "product designer",
            "ux researcher",
            "illustrator",
            "motion designer",
        ],
    ),
    (
        "product",
        &[
            "product manager",
            "program manager",
            "project manager",
            "scrum master",
            "product owner",
            "business analyst",
            "delivery manager",
        ],
    ),
    (
        "teaching",
        &[
            "teacher",
            "instructor",
            "tutor",
            "faculty",
            "professor",
            "trainer",
            "curriculum",
        ],
    ),
    (
        "consulting",
        &[
            "business consultant",
            "management consultant",
            "strategy consultant",
            "research associate",
        ],
    ),
];

/// Words that mean the posting is for an engineer whatever else the title says.
///
/// Matched as whole words, and that is the entire trick: "engineering" is not
/// "engineer", so "Engineering Recruiter" stays a recruiting job while
/// "Software Engineer, Sales Platform" stays an engineering one.
const ENGINEERING_WORDS: &[&str] = &[
    "engineer",
    "engineers",
    "developer",
    "developers",
    "programmer",
    "sde",
    "sdet",
    "sre",
    "devops",
    "architect",
    "scientist",
    "technologist",
    "coder",
    "hacker",
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
    let role =
        first_match(&format!(" {title} "), ROLE_RULES).or_else(|| first_match(&hay, ROLE_RULES));

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
        // Required-versus-preferred, what the job involves and what business it
        // is in are all prose questions. The heuristics leave them empty rather
        // than guessing; a model fills them in later if one is configured.
        ..Default::default()
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

/// Everything one model call produces about one posting.
///
/// Wider than `Facts` on purpose. A call to a hosted model costs real money and
/// takes real seconds, and the old prompt spent both on seven fields — then
/// threw away everything else the model had already read in order to answer
/// them. Salary, visa sponsorship, what the job actually involves, what it
/// insists on versus what it would like: all of that was in the reply and none
/// of it was kept. Ask once, keep everything.
#[derive(Clone, Debug, Default)]
pub struct Extraction {
    pub facts: Facts,
    pub salary_min: Option<i64>,
    pub salary_max: Option<i64>,
    pub salary_currency: Option<String>,
    /// year | month | day | hour.
    pub salary_period: Option<String>,
    pub visa_sponsorship: Option<bool>,
    /// Things worth knowing before you spend an evening on an application.
    pub red_flags: Vec<String>,
    /// One sentence describing the job, in the model's words.
    pub summary: Option<String>,
    /// The model's own read of the fit, 0-100, given the candidate profile.
    pub fit: Option<i64>,
    /// One line saying why. Shown in the score breakdown next to the number.
    pub fit_reason: Option<String>,
    /// How sure the model says it is, 0-1. Low confidence still stores; it is
    /// the reader's job to decide, and hiding the number would not help.
    pub confidence: Option<f64>,
    pub model: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// The reply exactly as it arrived.
    ///
    /// Kept because a schema is a guess about what will matter later, and this
    /// is the only copy of what was actually said. Re-reading a stored reply
    /// costs nothing; re-asking costs a call per posting.
    pub raw: String,
}

#[async_trait::async_trait]
pub trait Enricher: Send + Sync {
    /// Read the posting against a candidate profile. `None` means "no opinion" —
    /// the heuristics stand.
    async fn read(&self, post: &RawPost, me: &crate::ats::Profile) -> Option<Extraction>;
    /// Whether this is worth running a background pass for at all.
    fn is_llm(&self) -> bool {
        false
    }
    /// For the log line at boot: which model, where.
    fn describe(&self) -> String {
        "none".into()
    }
}

/// No model configured. The heuristics already ran at ingest; there is nothing
/// to add, and saying so explicitly is better than a background worker that
/// spins on rows it can never improve.
pub struct NoEnricher;

#[async_trait::async_trait]
impl Enricher for NoEnricher {
    async fn read(&self, _post: &RawPost, _me: &crate::ats::Profile) -> Option<Extraction> {
        None
    }
}

/// What the model is asked for.
///
/// The candidate goes in the prompt as well as the posting, which is the
/// change that makes this worth paying for: "does this posting want five years
/// of Kafka" is a question about the text, and "is this worth Shadab's evening"
/// is a question about both. The model answers the first as facts and the
/// second as one number it has to justify in a sentence.
///
/// The vocabulary is spelled out because a filter is only useful if the same
/// job lands under the same label every time, and anything outside it is
/// dropped in `sanitize` rather than stored.
fn prompt(post: &RawPost, me: &crate::ats::Profile, max_body: usize) -> (String, String) {
    let system = format!(
        "You read job postings for one specific engineer and return JSON only. \
         No prose, no markdown fence. Use null for anything the posting does not \
         state — never guess, and never infer a requirement from a job title. \
         Reply with exactly this shape:\n\
         {{\n  \
         \"role\": one of [{roles}] or null,\n  \
         \"level\": one of [{levels}] or null,\n  \
         \"years_min\": integer or null,\n  \
         \"years_max\": integer or null,\n  \
         \"work_mode\": one of [{modes}] or null,\n  \
         \"employment\": one of [{emp}] or null,\n  \
         \"domain\": one of [{domains}] or null,\n  \
         \"stack\": [\"Go\", \"Postgres\"],\n  \
         \"must_have\": [\"5+ years building backend services\", \"Go\"],\n  \
         \"nice_to_have\": [\"Kubernetes\", \"fintech experience\"],\n  \
         \"responsibilities\": [\"own the payments ledger service\"],\n  \
         \"salary_min\": integer or null, \"salary_max\": integer or null,\n  \
         \"salary_currency\": \"INR\" | \"USD\" | ... or null,\n  \
         \"salary_period\": \"year\" | \"month\" | \"day\" | \"hour\" or null,\n  \
         \"visa_sponsorship\": true | false | null,\n  \
         \"red_flags\": [\"unpaid trial task\"],\n  \
         \"summary\": one sentence, what this job is,\n  \
         \"fit\": integer 0-100, \"fit_reason\": one sentence,\n  \
         \"confidence\": number 0-1\n}}\n\n\
         must_have is what the posting requires; nice_to_have is what it calls \
         preferred, bonus or a plus. Put a requirement in exactly one of them. \
         red_flags is for things a candidate would want warned about — an unpaid \
         task, equity instead of salary, ten years wanted for a mid-level title, \
         a title that does not match the work. Empty list if there are none.\n\n\
         fit is your judgement of this candidate against this posting: 100 means \
         they meet everything it asks for, 50 means a real stretch, 0 means a \
         different profession. Judge the work and the requirements, not the \
         company's prestige. fit_reason must name the single thing that decided \
         the number.",
        roles = ROLES.join(", "),
        levels = LEVELS.join(", "),
        modes = WORK_MODES.join(", "),
        emp = EMPLOYMENT.join(", "),
        domains = DOMAINS.join(", "),
    );

    let user =
        format!(
        "CANDIDATE\nExperience: {years}\nLevel: {level}\nDisciplines: {roles}\nStack: {stack}\n\n\
         POSTING\nTitle: {title}\nCompany: {company}\nLocation: {location}\n\n{body}",
        years = me
            .years
            .map(|y| format!("{y} years"))
            .unwrap_or_else(|| "not stated".into()),
        level = me.level.clone().unwrap_or_else(|| "not stated".into()),
        roles = if me.roles.is_empty() { "not stated".into() } else { me.roles.join(", ") },
        stack = if me.stack.is_empty() { "not stated".into() } else { me.stack.join(", ") },
        title = post.title,
        company = post.company,
        location = post.location.as_deref().unwrap_or("not stated"),
        body = post.body.chars().take(max_body).collect::<String>(),
    );

    (system, user)
}

/// The model's reply. Every field is `Option` and the whole struct tolerates
/// junk, because a model *will* occasionally return a string where a number
/// belongs, and one bad reply must not poison a whole batch.
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
    domain: Option<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    stack: Vec<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    must_have: Vec<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    nice_to_have: Vec<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    responsibilities: Vec<String>,
    #[serde(default, deserialize_with = "lenient_int")]
    salary_min: Option<i64>,
    #[serde(default, deserialize_with = "lenient_int")]
    salary_max: Option<i64>,
    #[serde(default)]
    salary_currency: Option<String>,
    #[serde(default)]
    salary_period: Option<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    visa_sponsorship: Option<bool>,
    #[serde(default, deserialize_with = "lenient_list")]
    red_flags: Vec<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default, deserialize_with = "lenient_int")]
    fit: Option<i64>,
    #[serde(default)]
    fit_reason: Option<String>,
    #[serde(default, deserialize_with = "lenient_float")]
    confidence: Option<f64>,
}

/// Accept 5, "5", "5+" and null where an integer belongs.
fn lenient_int<'de, D>(d: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        serde_json::Value::String(s) => {
            let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        }
        _ => None,
    })
}

fn lenient_float<'de, D>(d: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

fn lenient_bool<'de, D>(d: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Bool(b) => Some(b),
        serde_json::Value::String(s) => match s.trim().to_lowercase().as_str() {
            "true" | "yes" => Some(true),
            "false" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

/// A list where a list is wanted, and a single string counted as a list of one.
fn lenient_list<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Array(a) => a
            .into_iter()
            .filter_map(|x| match x {
                serde_json::Value::String(s) => Some(s),
                // Some models answer [{"name": "Go"}] no matter how you ask.
                serde_json::Value::Object(o) => o
                    .get("name")
                    .or_else(|| o.get("skill"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                other => Some(other.to_string()),
            })
            .collect(),
        serde_json::Value::String(s) => vec![s],
        _ => Vec::new(),
    })
}

fn parse_reply(text: &str) -> Option<Reply> {
    if let Ok(r) = serde_json::from_str::<Reply>(text.trim()) {
        return Some(r);
    }
    serde_json::from_str::<Reply>(crate::llm::first_object(text)?).ok()
}

/// A validated `Extraction`, or `None` if the reply was not usable.
fn extraction_from(text: &str, model: &str) -> Option<Extraction> {
    let r = parse_reply(text)?;
    let mut facts = Facts {
        role: r.role,
        level: r.level,
        years_min: r.years_min,
        years_max: r.years_max,
        work_mode: r.work_mode,
        employment: r.employment,
        domain: r.domain,
        stack: r.stack,
        must_have: r.must_have,
        nice_to_have: r.nice_to_have,
        responsibilities: r.responsibilities,
    };
    facts.sanitize();

    let mut e = Extraction {
        facts,
        salary_min: r.salary_min,
        salary_max: r.salary_max,
        salary_currency: r.salary_currency.map(|c| c.trim().to_uppercase()),
        salary_period: in_vocab(&r.salary_period, &["year", "month", "day", "hour"]),
        visa_sponsorship: r.visa_sponsorship,
        red_flags: r.red_flags,
        summary: r.summary,
        fit: r.fit,
        fit_reason: r.fit_reason,
        confidence: r.confidence,
        model: model.to_string(),
        prompt_tokens: 0,
        completion_tokens: 0,
        raw: text.trim().to_string(),
    };
    e.sanitize();
    Some(e)
}

impl Extraction {
    /// Bound everything a model could get wrong in a way that matters.
    fn sanitize(&mut self) {
        // A "fit" of 250 is a model that did not read the instruction, not a
        // very good match.
        self.fit = self.fit.map(|f| f.clamp(0, 100));
        self.confidence = self.confidence.map(|c| c.clamp(0.0, 1.0));
        if let (Some(lo), Some(hi)) = (self.salary_min, self.salary_max) {
            if hi < lo {
                self.salary_max = None;
            }
        }
        // Salaries are reported in wildly different units; anything outside
        // this is a parse error rather than an offer.
        for s in [&mut self.salary_min, &mut self.salary_max] {
            if s.is_some_and(|v| !(1..=100_000_000).contains(&v)) {
                *s = None;
            }
        }
        for t in [&mut self.summary, &mut self.fit_reason] {
            if let Some(v) = t {
                *v = v.trim().chars().take(300).collect();
                if v.is_empty() {
                    *t = None;
                }
            }
        }
        for f in self.red_flags.iter_mut() {
            *f = f.trim().chars().take(120).collect();
        }
        self.red_flags.retain(|f| !f.is_empty());
        self.red_flags.truncate(6);
        // The raw reply is stored on the row; a model that returns an essay
        // must not turn one row into a megabyte.
        self.raw = self.raw.chars().take(20_000).collect();
    }
}

/// A hosted model, over the OpenAI chat-completions shape.
pub struct OpenAiEnricher {
    pub client: crate::llm::Client,
    pub max_body_chars: usize,
    pub base_url: String,
}

#[async_trait::async_trait]
impl Enricher for OpenAiEnricher {
    fn is_llm(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        format!("{} at {}", self.client.model(), self.base_url)
    }

    async fn read(&self, post: &RawPost, me: &crate::ats::Profile) -> Option<Extraction> {
        let (system, user) = prompt(post, me, self.max_body_chars);
        let answer = match self.client.json(&system, &user).await {
            Ok(a) => a,
            Err(e) => {
                // Loud rather than quiet: this one costs money and is the
                // difference between a sharpened board and a heuristic one, so
                // a key that is wrong should be visible on the Status tab
                // rather than a debug line nobody reads.
                tracing::warn!(%e, title = %post.title, "llm read failed");
                return None;
            }
        };
        let mut e = extraction_from(&answer.content, &answer.model)?;
        e.prompt_tokens = answer.usage.prompt_tokens;
        e.completion_tokens = answer.usage.completion_tokens;
        Some(e)
    }
}

/// A local model, over Ollama's own endpoint. Same prompt, same validation.
pub struct OllamaEnricher {
    pub client: reqwest::Client,
    pub url: String,
    pub model: String,
    pub max_body_chars: usize,
}

#[async_trait::async_trait]
impl Enricher for OllamaEnricher {
    fn is_llm(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        format!("{} at {}", self.model, self.url)
    }

    async fn read(&self, post: &RawPost, me: &crate::ats::Profile) -> Option<Extraction> {
        let (system, user) = prompt(post, me, self.max_body_chars);
        let req = serde_json::json!({
            "model": self.model,
            "system": system,
            "prompt": user,
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

        extraction_from(&text, &self.model)
    }
}

/// Pick the reader.
///
/// `[llm]` is the modern section; `[draft]` is where the ollama settings used
/// to live and still works, because a config file that stops working on
/// upgrade is a worse sin than a slightly awkward fallback.
pub fn build_enricher(
    llm: &crate::config::LlmCfg,
    draft: &DraftCfg,
    client: reqwest::Client,
) -> Box<dyn Enricher> {
    match llm.provider.as_str() {
        "openai" => {
            if llm.api_key.is_none() {
                // Configured but unusable. Saying which variable is missing is
                // the difference between a five-second fix and an afternoon.
                tracing::error!(
                    "llm.provider is \"openai\" but no API key is set — put \
                     OPENAI_API_KEY in .env. Falling back to heuristics."
                );
                return Box::new(NoEnricher);
            }
            Box::new(OpenAiEnricher {
                client: crate::llm::Client::new(client, llm.clone()),
                max_body_chars: llm.max_body_chars,
                base_url: llm.base_url.clone(),
            })
        }
        "ollama" => Box::new(OllamaEnricher {
            client,
            url: llm.base_url.clone(),
            model: llm.model.clone(),
            max_body_chars: llm.max_body_chars,
        }),
        // No [llm] section at all: honour the old [draft] one.
        _ if draft.provider == "ollama" => Box::new(OllamaEnricher {
            client,
            url: draft.ollama_url.clone(),
            model: draft.ollama_model.clone(),
            max_body_chars: llm.max_body_chars,
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
        assert_eq!(
            years("2+ years of python and 5+ years overall"),
            (Some(5), None)
        );
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
        let f = heuristic(&post(
            "Engineer, Platform Team",
            "Kubernetes, Terraform, CI/CD.",
        ));
        assert_eq!(f.role.as_deref(), Some("devops"));
    }

    #[test]
    fn level_comes_only_from_the_title() {
        // "you'll work with senior engineers" is not a senior req.
        let f = heuristic(&post(
            "Backend Engineer",
            "You will work with senior engineers.",
        ));
        assert_eq!(f.level, None);
        assert_eq!(
            heuristic(&post("Staff Engineer, Infra", ""))
                .level
                .as_deref(),
            Some("staff")
        );
    }

    #[test]
    fn hybrid_beats_remote_because_hybrid_posts_say_remote() {
        let f = heuristic(&post(
            "Backend Engineer",
            "Hybrid — 3 days in office, 2 days remote.",
        ));
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
        let r =
            parse_reply("Here you go:\n```json\n{\"role\":\"backend\",\"years_min\":\"5+\"}\n```")
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
        let f = |lo, hi| {
            Facts {
                years_min: lo,
                years_max: hi,
                ..Default::default()
            }
            .years_label()
        };
        assert_eq!(f(Some(5), None).as_deref(), Some("5+ yrs"));
        assert_eq!(f(Some(3), Some(5)).as_deref(), Some("3\u{2013}5 yrs"));
        assert_eq!(f(None, None), None);
    }

    #[test]
    fn the_jobs_that_arrive_with_the_engineering_ones_are_named() {
        // One board per company means the whole payroll comes through it.
        assert_eq!(
            off_discipline("Senior HR Business Partner"),
            Some("recruiting")
        );
        assert_eq!(
            off_discipline("Talent Acquisition Specialist"),
            Some("recruiting")
        );
        assert_eq!(
            off_discipline("Account Executive, Enterprise"),
            Some("sales")
        );
        assert_eq!(off_discipline("Registered Nurse - ICU"), Some("clinical"));
        assert_eq!(off_discipline("Technical Program Manager"), Some("product"));
        assert_eq!(off_discipline("Warehouse Associate"), Some("operations"));
    }

    #[test]
    fn a_discipline_word_about_the_team_is_not_the_job() {
        // The whole reason the guard matches whole words: half of what an
        // engineer is hired to build is named after another department.
        assert_eq!(off_discipline("Software Engineer, Sales Platform"), None);
        assert_eq!(
            off_discipline("Backend Developer - Marketing Technology"),
            None
        );
        assert_eq!(off_discipline("Data Scientist, Clinical Research"), None);
        assert_eq!(off_discipline("Staff Engineer, Payments & Billing"), None);
    }

    #[test]
    fn engineering_recruiter_is_still_a_recruiter() {
        // "engineering" is not "engineer", and this is the case that decides it.
        assert_eq!(off_discipline("Engineering Recruiter"), Some("recruiting"));
        assert_eq!(
            off_discipline("Technical Recruiter, Engineering"),
            Some("recruiting")
        );
    }

    #[test]
    fn a_title_with_no_rule_at_all_is_not_an_endorsement() {
        // None means "no rule matched", never "this one is for you".
        assert_eq!(off_discipline("Member of Technical Staff"), None);
        assert!(!engineering_title("Member of Technical Staff"));
        assert!(engineering_title("Senior Software Engineer II"));
    }
}
