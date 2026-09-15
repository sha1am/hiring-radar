//! Applicant tracking, run from the candidate's side.
//!
//! A real ATS asks one question about a posting: *does this person meet the
//! requirements, and which ones do they miss?* Everything this file does
//! follows from taking that question literally.
//!
//! The scorer it replaces asked something much weaker. It matched job titles by
//! substring and the body by IDF cosine over a bag of words, which cannot tell
//! "five years of Go" from "we use Go, you'll pick it up", cannot tell that
//! "Software Engineer II, Server Platform" is the same job as "Backend
//! Engineer", and produces as its explanation a list of words the two documents
//! happen to share. A word list is not a reason.
//!
//! So this scores dimension by dimension against structured facts — the ones
//! `enrich` already pulls out of every posting — and every dimension is
//! something you could argue with:
//!
//! | dimension | weight | the question it asks                     |
//! |-----------|--------|------------------------------------------|
//! | stack     | 35     | do you write what they need written?      |
//! | years     | 20     | are you experienced enough for it?        |
//! | role      | 20     | is this the kind of engineering you do?   |
//! | level     | 15     | is it pitched at your level?              |
//! | location  | 10     | can you take it?                          |
//!
//! That is a weighting, not a truth, and the numbers are argued for at each
//! function below rather than tuned until the demo looked good.
//!
//! Two properties matter more than the weights:
//!
//! **It runs without a model.** Every dimension is computed from facts the
//! heuristic extractor produces, so the assessment works on a fresh install
//! with no Ollama. An LLM sharpens the *facts* (see `enrich`) and the reasoning
//! about requirements; it is not load-bearing.
//!
//! **It says what is missing.** `Assessment::missing` is the list of things the
//! posting wants that you do not have, which is the output a candidate actually
//! needs and that no real ATS ever shows them. Aggregated across the board it
//! answers "what should I learn next", which is a better question than "what
//! should I apply to".

use crate::enrich::Facts;
use crate::model::RawPost;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};

// ===================== the candidate =====================

/// What we know about the person applying.
///
/// Derived from the resume once, at upload, rather than per posting: it does
/// not change between postings, and re-deriving it for every row was most of
/// what made the old content score expensive.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Total professional experience. The single most load-bearing number here,
    /// which is why the settings page lets you simply type it: you know it
    /// exactly, and no extraction beats being told.
    pub years: Option<i64>,
    /// Where you sit on the ladder — see `enrich::LEVELS`.
    pub level: Option<String>,
    /// The kinds of engineering you have actually done, most recent first.
    pub roles: Vec<String>,
    /// Everything you have worked with, in the tag vocabulary.
    pub stack: Vec<String>,
}

impl Profile {
    /// Read a resume without a model.
    ///
    /// Stack comes from the same extractor the postings use, so the two sides
    /// of every comparison speak one vocabulary — which is the whole reason
    /// this can be compared at all. Level and role come from the title words a
    /// resume uses about itself.
    pub fn from_resume(text: &str, stated_years: Option<i64>) -> Profile {
        let hay = text.to_lowercase();

        let mut roles: Vec<String> = Vec::new();
        for (role, _) in crate::enrich::role_rules() {
            if crate::enrich::role_matches(&hay, role) && !roles.contains(&role.to_string()) {
                roles.push(role.to_string());
            }
        }

        // The highest level the resume claims. A resume that says "Senior
        // Engineer" once and "Engineer" five times is a senior engineer's
        // resume.
        let level = crate::enrich::LEVELS
            .iter()
            .rev()
            .find(|l| crate::enrich::level_matches(&hay, l))
            .map(|l| (*l).to_string());

        Profile {
            years: stated_years.or_else(|| crate::enrich::years(&hay).0),
            level,
            roles,
            stack: crate::tags::extract(&hay),
        }
    }

    /// Whether the profile carries enough to assess against. Below this the
    /// caller falls back to the lexical scorer rather than producing a
    /// confident-looking number from nothing.
    pub fn is_usable(&self) -> bool {
        !self.stack.is_empty() || self.years.is_some()
    }

    fn has(&self, tech: &str) -> bool {
        self.stack.iter().any(|t| t.eq_ignore_ascii_case(tech))
    }
}

// ===================== the assessment =====================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// You meet what it asks. Worth the time to apply properly.
    Apply,
    /// Short on something, but close enough that a good cover note carries it.
    Stretch,
    /// Materially above you or beside you. Apply if you love it, expect silence.
    Reach,
    /// Not your job.
    Skip,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Apply => "apply",
            Verdict::Stretch => "stretch",
            Verdict::Reach => "reach",
            Verdict::Skip => "skip",
        }
    }

    pub fn parse(s: &str) -> Option<Verdict> {
        match s.trim().to_lowercase().as_str() {
            "apply" => Some(Verdict::Apply),
            "stretch" => Some(Verdict::Stretch),
            "reach" => Some(Verdict::Reach),
            "skip" => Some(Verdict::Skip),
            _ => None,
        }
    }
}

/// One dimension's contribution, kept so the score can explain itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dimension {
    pub name: String,
    /// 0.0–1.0 before weighting.
    pub fit: f64,
    pub weight: f64,
    /// What this dimension concluded, in words.
    pub note: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Assessment {
    pub score: f64,
    pub verdict: Option<String>,
    /// Requirements you meet, in the posting's own vocabulary.
    pub met: Vec<String>,
    /// Requirements you don't. The output a candidate needs and no real ATS
    /// ever gives them.
    pub missing: Vec<String>,
    /// One sentence, suitable for a card.
    pub reason: String,
    pub dimensions: Vec<Dimension>,
}

const W_STACK: f64 = 35.0;
const W_YEARS: f64 = 20.0;
const W_ROLE: f64 = 20.0;
const W_LEVEL: f64 = 15.0;
const W_LOCATION: f64 = 10.0;

/// Score one posting against one profile.
///
/// Deterministic and cheap: no network, no model, no corpus. The same posting
/// assessed twice gives the same answer, which matters because the release
/// engine acts on this number and a score that drifts between a notification
/// and the dashboard is a bug you cannot reproduce.
pub fn assess(profile: &Profile, facts: &Facts, post: &RawPost, s: &Settings) -> Assessment {
    // Dealbreakers are absolute, as they are in the lexical scorer. A word you
    // said you will not work with is not a dimension to be outvoted.
    let hay = post.haystack();
    if s.dealbreakers.iter().any(|d| hay.contains(&d.to_lowercase())) {
        return Assessment {
            score: 0.0,
            verdict: Some(Verdict::Skip.as_str().into()),
            missing: vec!["matches a dealbreaker".into()],
            reason: "Ruled out by one of your dealbreakers.".into(),
            ..Default::default()
        };
    }

    let mut met: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    let stack = stack_fit(profile, facts, &mut met, &mut missing);
    let years = years_fit(profile, facts, &mut met, &mut missing);
    let role = role_fit(profile, facts, &mut met, &mut missing);
    let level = level_fit(profile, facts, &mut met, &mut missing);
    let location = location_fit(post, s);

    let dimensions = vec![stack, years, role, level, location];
    let sum: f64 = dimensions.iter().map(|d| d.fit * d.weight).sum();
    let score = (sum * relevance_factor(&dimensions)).clamp(0.0, 100.0);

    let verdict = verdict_for(score, &dimensions, &missing);
    let reason = reason_for(verdict, &dimensions, &missing);

    Assessment {
        score,
        verdict: Some(verdict.as_str().into()),
        met,
        missing,
        reason,
        dimensions,
    }
}

/// Can you do this job at all?
///
/// A flat weighted sum has a hole in it, and a test found it: a frontend
/// posting scored 49 for a backend engineer — nothing on stack, nothing on
/// role, but the right number of years in the right city at the right level.
/// Forty-five of those points came from dimensions that say nothing about
/// whether you can do the work.
///
/// So the sum is conditional rather than flat. Stack and role are the
/// *substance* — can you do it, is it your discipline — and everything else
/// only modulates. Nobody is hired for a frontend role because they live in the
/// right city, and the arithmetic should not imply otherwise.
///
/// The floor of 0.35 rather than 0 is deliberate: a posting can name no
/// technology at all, and collapsing those to nothing would punish a listing
/// for being vaguely written rather than for being wrong for you.
fn relevance_factor(dims: &[Dimension]) -> f64 {
    let fit = |name: &str| dims.iter().find(|d| d.name == name).map(|d| d.fit).unwrap_or(0.5);
    let relevance = 0.6 * fit("stack") + 0.4 * fit("role");
    0.35 + 0.65 * relevance
}

/// Do you write what they need written?
///
/// The heaviest dimension, because it is the one a hiring manager screens on
/// first and the one you cannot talk your way past in a week. Languages count
/// double: swapping Postgres for MySQL is a Monday, swapping Go for Scala is a
/// quarter.
fn stack_fit(p: &Profile, f: &Facts, met: &mut Vec<String>, missing: &mut Vec<String>) -> Dimension {
    let wanted = &f.stack;
    if wanted.is_empty() {
        // A posting that names no technology is not evidence against you. Half
        // marks: neither rewarded nor punished for how it was written.
        return Dimension {
            name: "stack".into(),
            fit: 0.5,
            weight: W_STACK,
            note: "posting names no technologies".into(),
        };
    }

    let mut have_w = 0.0;
    let mut want_w = 0.0;
    for tech in wanted {
        let w = if crate::tags::is_language(tech) { 2.0 } else { 1.0 };
        want_w += w;
        if p.has(tech) {
            have_w += w;
            met.push(tech.clone());
        } else {
            missing.push(tech.clone());
        }
    }

    let fit = if want_w > 0.0 { have_w / want_w } else { 0.5 };
    let note = if missing.is_empty() {
        format!("has all {} named", wanted.len())
    } else {
        format!("{} of {} named", met.len(), wanted.len())
    };
    Dimension {
        name: "stack".into(),
        fit,
        weight: W_STACK,
        note,
    }
}

/// Are you experienced enough for it?
///
/// A soft slope, not a cliff. "5+ years" with four years behind you is a real
/// application that people win; with one year behind you it is not. The old
/// scorer had no opinion on this at all, which is why an eight-year staff role
/// and a one-year graduate role scored identically for the same person.
fn years_fit(p: &Profile, f: &Facts, met: &mut Vec<String>, missing: &mut Vec<String>) -> Dimension {
    let (Some(have), Some(want)) = (p.years, f.years_min) else {
        return Dimension {
            name: "years".into(),
            fit: 0.6,
            weight: W_YEARS,
            note: if p.years.is_none() {
                "your experience isn't set — put it in settings".into()
            } else {
                "posting states no requirement".into()
            },
        };
    };

    let gap = want - have;
    let fit = match gap {
        g if g <= 0 => {
            // Comfortably over is not better than meeting it, and far over
            // starts to read as overqualified — a real reason for silence.
            if have >= want + 7 {
                0.85
            } else {
                1.0
            }
        }
        1 => 0.8,
        2 => 0.55,
        3 => 0.3,
        _ => 0.1,
    };

    let label = format!("{want}+ years");
    if gap <= 0 {
        met.push(label);
    } else {
        missing.push(format!("{gap} more year{}", if gap == 1 { "" } else { "s" }));
    }

    Dimension {
        name: "years".into(),
        fit,
        weight: W_YEARS,
        note: match gap {
            g if g <= 0 => format!("wants {want}, you have {have}"),
            g => format!("wants {want}, you have {have} — {g} short"),
        },
    }
}

/// Adjacent disciplines, and how far apart they really are.
///
/// SRE and DevOps are the same job at most companies. Backend and data share a
/// great deal. Backend and frontend share very little, whatever a "full stack"
/// posting claims. These numbers are judgement, and being explicit about them
/// beats the old behaviour — which had no concept of role at all, so a frontend
/// posting and a backend posting were the same to it.
const ROLE_NEIGHBOURS: &[(&str, &str, f64)] = &[
    ("sre", "devops", 0.9),
    ("backend", "fullstack", 0.7),
    ("frontend", "fullstack", 0.7),
    ("data", "ml", 0.7),
    ("backend", "data", 0.5),
    ("backend", "sre", 0.5),
    ("backend", "devops", 0.45),
    ("backend", "ml", 0.35),
    ("devops", "embedded", 0.2),
    ("frontend", "mobile", 0.4),
    ("backend", "frontend", 0.2),
];

fn role_affinity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    ROLE_NEIGHBOURS
        .iter()
        .find(|(x, y, _)| (*x == a && *y == b) || (*x == b && *y == a))
        .map(|(_, _, v)| *v)
        .unwrap_or(0.15)
}

fn role_fit(p: &Profile, f: &Facts, met: &mut Vec<String>, missing: &mut Vec<String>) -> Dimension {
    let Some(want) = f.role.as_deref() else {
        return Dimension {
            name: "role".into(),
            fit: 0.6,
            weight: W_ROLE,
            note: "couldn't tell what kind of role this is".into(),
        };
    };
    if p.roles.is_empty() {
        return Dimension {
            name: "role".into(),
            fit: 0.6,
            weight: W_ROLE,
            note: "your resume doesn't name a discipline".into(),
        };
    }

    // Best match across everything you have done — a backend engineer who has
    // also done data work should not be marked down on a data posting.
    let fit = p
        .roles
        .iter()
        .map(|r| role_affinity(r, want))
        .fold(0.0_f64, f64::max);

    if fit >= 0.9 {
        met.push(format!("{want} work"));
    } else if fit < 0.5 {
        missing.push(format!("{want} background"));
    }

    Dimension {
        name: "role".into(),
        fit,
        weight: W_ROLE,
        note: if fit >= 0.9 {
            format!("{want}, which is what you do")
        } else if fit >= 0.5 {
            format!("{want}, adjacent to your {}", p.roles.join("/"))
        } else {
            format!("{want}, away from your {}", p.roles.join("/"))
        },
    }
}

fn ladder(level: &str) -> Option<usize> {
    match level {
        "intern" => Some(0),
        "junior" => Some(1),
        "mid" => Some(2),
        "senior" => Some(3),
        // A manager role is a sideways step, not a rung — parked level with
        // staff so the distance maths stays sane.
        "staff" | "manager" => Some(4),
        "principal" => Some(5),
        _ => None,
    }
}

/// Is it pitched at your level?
///
/// Asymmetric on purpose. A posting one rung above you is a stretch people win
/// all the time; one rung below is a step down you probably don't want, and
/// three rungs below is a waste of an application either way.
fn level_fit(p: &Profile, f: &Facts, met: &mut Vec<String>, missing: &mut Vec<String>) -> Dimension {
    let (Some(want), Some(have)) = (f.level.as_deref(), p.level.as_deref()) else {
        return Dimension {
            name: "level".into(),
            fit: 0.6,
            weight: W_LEVEL,
            note: "level not stated on one side".into(),
        };
    };
    let (Some(w), Some(h)) = (ladder(want), ladder(have)) else {
        return Dimension {
            name: "level".into(),
            fit: 0.6,
            weight: W_LEVEL,
            note: "level not on the ladder".into(),
        };
    };

    let fit = match w as i64 - h as i64 {
        0 => 1.0,
        1 => 0.65,  // a stretch you can win
        2 => 0.25,  // two rungs up is usually a no
        n if n > 2 => 0.1,
        -1 => 0.7, // a step down, but they may still want you
        _ => 0.35, // well below you — likely a waste of an application
    };

    if fit >= 0.9 {
        met.push(format!("{want} level"));
    } else if w > h {
        missing.push(format!("{want} level"));
    }

    Dimension {
        name: "level".into(),
        fit,
        weight: W_LEVEL,
        note: format!("{want} role, you read as {have}"),
    }
}

fn location_fit(post: &RawPost, s: &Settings) -> Dimension {
    use crate::score::{location_verdict, LocationVerdict};
    let (fit, note) = match location_verdict(post, s) {
        LocationVerdict::Match => (1.0, "somewhere you want to be".to_string()),
        LocationVerdict::NoMatch => (0.3, "outside the places you listed".to_string()),
        // The pipeline drops these before scoring; if one arrives anyway, it is
        // worth nothing rather than worth arguing about.
        LocationVerdict::Rejected => (0.0, "outside a location you require".to_string()),
    };
    Dimension {
        name: "location".into(),
        fit,
        weight: W_LOCATION,
        note,
    }
}

/// Turn a score into advice.
///
/// Not purely a threshold on the total, because a total hides the shape. A
/// posting can reach 70 on stack and location alone while asking for four more
/// years than you have, and telling you to apply to that is telling you to
/// waste an afternoon. So a hard shortfall on years or stack caps the verdict
/// regardless of the sum.
fn verdict_for(score: f64, dims: &[Dimension], missing: &[String]) -> Verdict {
    let fit_of = |name: &str| dims.iter().find(|d| d.name == name).map(|d| d.fit).unwrap_or(1.0);

    let badly_short = fit_of("years") <= 0.3 || fit_of("stack") < 0.25 || fit_of("role") < 0.3;
    if badly_short {
        return if score >= 55.0 { Verdict::Reach } else { Verdict::Skip };
    }

    let plain = match score {
        s if s >= 72.0 => Verdict::Apply,
        s if s >= 55.0 => Verdict::Stretch,
        s if s >= 35.0 => Verdict::Reach,
        _ => Verdict::Skip,
    };

    // A two-year shortfall is a considered application, not a slam dunk, and
    // the weighted sum does not say so on its own: being two years short costs
    // nine points out of a hundred, which a perfect stack swamps. The first
    // version of this returned "Meets what it asks for. Missing 2 more years."
    // — a sentence that contradicts itself, and the contradiction was the
    // model telling on itself.
    if plain == Verdict::Apply && fit_of("years") < 0.8 {
        return Verdict::Stretch;
    }
    plain
}

/// One sentence that is actually a reason.
///
/// The old explanation was the list of words the resume and the posting shared,
/// which reads as evidence but says nothing — "go, kafka, systems" is true of
/// every posting you would ever look at. This names the dimension that decided
/// it, which is the thing you would have wanted to know.
fn reason_for(v: Verdict, dims: &[Dimension], missing: &[String]) -> String {
    let weakest = dims
        .iter()
        .filter(|d| d.fit < 0.9)
        .min_by(|a, b| (a.fit * a.weight).partial_cmp(&(b.fit * b.weight)).unwrap());

    let gap = if missing.is_empty() {
        String::new()
    } else {
        let shown: Vec<&str> = missing.iter().take(3).map(String::as_str).collect();
        let more = missing.len().saturating_sub(3);
        format!(
            " Missing {}{}.",
            shown.join(", "),
            if more > 0 { format!(" and {more} more") } else { String::new() }
        )
    };

    let head = match v {
        // Only claim this when it is true. An "Apply" carrying a list of gaps
        // needs different words, or the sentence argues with itself.
        Verdict::Apply if missing.is_empty() => "Meets everything it asks for.",
        Verdict::Apply => "Worth applying to.",
        Verdict::Stretch => "Close — worth a considered application.",
        Verdict::Reach => "A reach.",
        Verdict::Skip => "Not a fit.",
    };

    match weakest {
        Some(d) if v != Verdict::Apply => format!("{head} Weakest on {}: {}.{gap}", d.name, d.note),
        _ => format!("{head}{gap}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ApplyChannel;

    fn post(title: &str, body: &str, loc: &str) -> RawPost {
        RawPost {
            source: "greenhouse".into(),
            external_id: "x".into(),
            url: "https://e.com".into(),
            title: title.into(),
            company: "Acme".into(),
            location: Some(loc.into()),
            body: body.into(),
            posted_at: None,
            apply: ApplyChannel::Unknown,
            synthetic_title: false,
        }
    }

    fn settings() -> Settings {
        let mut s: Settings = serde_json::from_str("{}").unwrap();
        s.locations = vec!["india".into()];
        s.location_policy = "prefer".into();
        s
    }

    /// A backend engineer with three years and a Go/Postgres/Kafka stack.
    fn candidate() -> Profile {
        Profile {
            years: Some(3),
            level: Some("mid".into()),
            roles: vec!["backend".into()],
            stack: vec!["Go".into(), "Postgres".into(), "Kafka".into(), "Docker".into()],
        }
    }

    fn facts(role: &str, level: &str, years: i64, stack: &[&str]) -> Facts {
        Facts {
            role: Some(role.into()),
            level: Some(level.into()),
            years_min: Some(years),
            years_max: None,
            work_mode: None,
            employment: None,
            stack: stack.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn the_right_job_scores_well_and_says_apply() {
        let a = assess(
            &candidate(),
            &facts("backend", "mid", 3, &["Go", "Postgres", "Kafka"]),
            &post("Backend Engineer", "Go, Postgres, Kafka.", "Bengaluru, India"),
            &settings(),
        );
        assert!(a.score >= 90.0, "{a:?}");
        assert_eq!(a.verdict.as_deref(), Some("apply"));
        assert!(a.missing.is_empty(), "{:?}", a.missing);
        assert!(a.reason.starts_with("Meets everything"), "{}", a.reason);
    }

    #[test]
    fn four_years_short_is_a_reach_however_well_the_stack_matches() {
        // This is the case the old scorer got most wrong: a perfect stack and
        // location reached the seventies while the posting wanted twice the
        // experience, and it read as a strong match.
        let a = assess(
            &candidate(),
            &facts("backend", "staff", 8, &["Go", "Postgres", "Kafka"]),
            &post("Staff Backend Engineer", "Go, Postgres, Kafka. 8+ years.", "Bengaluru, India"),
            &settings(),
        );
        assert_eq!(a.verdict.as_deref(), Some("reach"), "{a:?}");
        assert!(a.missing.iter().any(|m| m.contains("year")), "{:?}", a.missing);
    }

    #[test]
    fn a_reason_never_contradicts_its_own_verdict() {
        // "Meets what it asks for. Missing 2 more years." shipped once, and the
        // contradiction was the scoring model telling on itself.
        let a = assess(
            &candidate(),
            &facts("backend", "senior", 5, &["Go", "Postgres", "Kafka"]),
            &post("Senior Backend Engineer", "Go, Postgres, Kafka. 5+ years.", "Bengaluru, India"),
            &settings(),
        );
        if !a.missing.is_empty() {
            assert!(
                !a.reason.starts_with("Meets everything"),
                "claims to meet everything while listing gaps: {}",
                a.reason
            );
        }
        // Two years short is a considered application, not a slam dunk.
        assert_eq!(a.verdict.as_deref(), Some("stretch"), "{a:?}");
    }

    #[test]
    fn one_year_short_is_a_stretch_not_a_rejection() {
        // People win these. A cliff here would hide most of the jobs worth
        // applying to.
        let a = assess(
            &candidate(),
            &facts("backend", "senior", 4, &["Go", "Postgres"]),
            &post("Senior Backend Engineer", "Go, Postgres. 4+ years.", "Bengaluru, India"),
            &settings(),
        );
        assert!(
            matches!(a.verdict.as_deref(), Some("stretch") | Some("apply")),
            "{a:?}"
        );
    }

    #[test]
    fn a_language_you_dont_write_costs_more_than_a_database_you_dont() {
        // Swapping Postgres for MySQL is a Monday. Swapping Go for Scala is a
        // quarter, and the weighting has to say so.
        let base = facts("backend", "mid", 3, &["Go", "Postgres"]);
        let other_db = facts("backend", "mid", 3, &["Go", "Cassandra"]);
        let other_lang = facts("backend", "mid", 3, &["Scala", "Postgres"]);
        let p = post("Backend Engineer", "…", "Bengaluru, India");

        let a = assess(&candidate(), &base, &p, &settings()).score;
        let b = assess(&candidate(), &other_db, &p, &settings()).score;
        let c = assess(&candidate(), &other_lang, &p, &settings()).score;
        assert!(a > b, "an unknown database should cost something");
        assert!(b > c, "an unknown language should cost more than a database: {b} vs {c}");
    }

    #[test]
    fn sre_and_devops_are_nearly_the_same_job() {
        let sre = Profile { roles: vec!["sre".into()], ..candidate() };
        let a = assess(
            &sre,
            &facts("devops", "mid", 3, &["Go", "Docker"]),
            &post("DevOps Engineer", "Go, Docker.", "Bengaluru, India"),
            &settings(),
        );
        let role = a.dimensions.iter().find(|d| d.name == "role").unwrap();
        assert!(role.fit >= 0.85, "{role:?}");
    }

    #[test]
    fn the_qualifiers_cannot_carry_a_job_you_cannot_do() {
        // A frontend posting for a backend engineer: right years, right level,
        // right city, and nothing else. Under a flat weighted sum that reached
        // 49 — forty-five points from dimensions that say nothing about whether
        // you can do the work. It has to collapse.
        let a = assess(
            &candidate(),
            &facts("frontend", "mid", 3, &["TypeScript", "React"]),
            &post("Frontend Engineer", "React, TypeScript.", "Bengaluru, India"),
            &settings(),
        );
        assert!(a.score < 25.0, "{a:?}");
        assert_eq!(a.verdict.as_deref(), Some("skip"));
    }

    #[test]
    fn a_vague_posting_is_damped_but_not_destroyed() {
        // Naming no technology is a fact about the writing, not about you, so
        // the relevance floor keeps such a posting in contention.
        let vague = Facts { role: Some("backend".into()), years_min: Some(3), ..Default::default() };
        let a = assess(&candidate(), &vague, &post("Backend Engineer", "Own services.", "Bengaluru, India"), &settings());
        assert!(a.score > 45.0, "{a:?}");
    }

    #[test]
    fn a_posting_naming_no_technology_is_not_held_against_you() {
        // Half marks rather than zero: that is a fact about how the posting was
        // written, not about the candidate.
        let bare = Facts { role: Some("backend".into()), ..Default::default() };
        let a = assess(&candidate(), &bare, &post("Backend Engineer", "You'll own services.", "Bengaluru, India"), &settings());
        let stack = a.dimensions.iter().find(|d| d.name == "stack").unwrap();
        assert_eq!(stack.fit, 0.5);
        assert!(a.missing.is_empty());
    }

    #[test]
    fn a_dealbreaker_is_absolute() {
        let mut s = settings();
        s.dealbreakers = vec!["php".into()];
        let a = assess(
            &candidate(),
            &facts("backend", "mid", 3, &["Go"]),
            &post("Backend Engineer", "Some legacy PHP to migrate.", "Bengaluru, India"),
            &s,
        );
        assert_eq!(a.score, 0.0);
        assert_eq!(a.verdict.as_deref(), Some("skip"));
    }

    #[test]
    fn the_reason_names_the_dimension_that_decided_it() {
        // The old explanation was a list of shared words, which is true of
        // every posting you would ever look at and therefore says nothing.
        let a = assess(
            &candidate(),
            &facts("backend", "staff", 9, &["Go", "Postgres"]),
            &post("Staff Engineer", "Go, Postgres. 9+ years.", "Bengaluru, India"),
            &settings(),
        );
        assert!(a.reason.contains("years"), "{}", a.reason);
        assert!(a.reason.contains("Missing"), "{}", a.reason);
    }

    #[test]
    fn missing_is_the_list_you_could_act_on() {
        let a = assess(
            &candidate(),
            &facts("backend", "mid", 3, &["Go", "Kubernetes", "Rust"]),
            &post("Backend Engineer", "Go, Kubernetes, Rust.", "Bengaluru, India"),
            &settings(),
        );
        assert!(a.missing.contains(&"Rust".to_string()), "{:?}", a.missing);
        assert!(a.missing.contains(&"Kubernetes".to_string()), "{:?}", a.missing);
        assert!(a.met.contains(&"Go".to_string()), "{:?}", a.met);
    }

    #[test]
    fn overqualified_scores_below_a_clean_match() {
        // Ten years against a two-year req is a real reason for silence, and
        // pretending otherwise fills the board with roles that will not reply.
        let senior = Profile { years: Some(12), ..candidate() };
        let f = facts("backend", "mid", 2, &["Go"]);
        let p = post("Backend Engineer", "Go. 2+ years.", "Bengaluru, India");
        let over = assess(&senior, &f, &p, &settings());
        let exact = assess(&candidate(), &f, &p, &settings());
        assert!(over.score < exact.score, "{} vs {}", over.score, exact.score);
    }

    #[test]
    fn an_unusable_profile_is_recognised_rather_than_scored() {
        assert!(!Profile::default().is_usable());
        assert!(candidate().is_usable());
    }

    #[test]
    fn a_profile_reads_its_own_resume() {
        let p = Profile::from_resume(
            "Senior Backend Engineer at Acme. Go, Kafka, Postgres, Kubernetes. \
             Built distributed services. 6 years of experience.",
            None,
        );
        assert_eq!(p.level.as_deref(), Some("senior"));
        assert!(p.roles.contains(&"backend".to_string()), "{:?}", p.roles);
        assert!(p.has("Go") && p.has("Kafka"));
        assert_eq!(p.years, Some(6));
    }

    #[test]
    fn a_stated_year_count_beats_whatever_the_resume_says() {
        // You know your own number. No extraction beats being told.
        let p = Profile::from_resume("… 6 years of experience …", Some(3));
        assert_eq!(p.years, Some(3));
    }

    #[test]
    fn dimensions_sum_to_a_hundred() {
        // If these drift apart, every score silently changes meaning.
        assert_eq!(W_STACK + W_YEARS + W_ROLE + W_LEVEL + W_LOCATION, 100.0);
    }
}
