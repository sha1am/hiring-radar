use crate::model::RawPost;
use crate::resume::{tokenize, Matcher};
use crate::settings::Settings;

/// Match scoring is a swappable trait. The default blends two things:
///
///  - *structural* signals (title, location, seniority) that a resume can't
///    express — you want a senior role in Bengaluru whatever your resume says;
///  - a *content* signal, which is resume similarity when a resume is loaded
///    and keyword coverage otherwise.
///
/// The two are kept separate on purpose. Resume similarity is good at "is this
/// the kind of work I do" and useless at "is this the right level and place",
/// and scoring one with the other is how you end up alerting on a Berlin
/// internship that happens to mention Kafka.
pub trait Scorer: Send + Sync {
    /// Returns a 0..100 match score and the resume terms that drove it, if any.
    /// Freshness is applied separately, at selection time.
    fn score(&self, post: &RawPost, p: &Settings, m: &Matcher) -> (f64, Vec<String>);
}

pub struct LexicalScorer;

/// Point budget. Sums to 100 with a full title hit, a location hit, a seniority
/// hit and perfect content coverage.
const TITLE_FULL: f64 = 40.0;
const TITLE_PARTIAL: f64 = 18.0;
const LOCATION: f64 = 10.0;
const SENIORITY_HIT: f64 = 12.0;
const SENIORITY_MISS: f64 = -20.0;
const CONTENT: f64 = 38.0;
const SALARY_PENALTY: f64 = -30.0;

impl Scorer for LexicalScorer {
    fn score(&self, post: &RawPost, p: &Settings, m: &Matcher) -> (f64, Vec<String>) {
        let title = post.title.to_lowercase();
        let hay = post.haystack();

        // Hard filter: any dealbreaker present zeroes the post out.
        if p.dealbreakers.iter().any(|d| hay.contains(&d.to_lowercase())) {
            return (0.0, Vec::new());
        }

        let mut s = 0.0f64;

        // --- title: the strongest single signal ---
        if p.titles.iter().any(|t| title.contains(&t.to_lowercase())) {
            s += TITLE_FULL;
        } else {
            let title_words: Vec<String> = p
                .titles
                .iter()
                .flat_map(|t| {
                    t.to_lowercase()
                        .split_whitespace()
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
                .filter(|w| w.len() > 2)
                .collect();
            if title_words.iter().any(|w| title.contains(w.as_str())) {
                s += TITLE_PARTIAL;
            }
        }

        // --- content: resume similarity, keyword coverage, or a blend ---
        let (content_frac, matched) = content_score(post, p, m, &hay);
        s += content_frac * CONTENT;

        // --- location ---
        let loc_hay = post
            .location
            .as_deref()
            .map(|l| l.to_lowercase())
            .unwrap_or_default();
        let loc_text = format!("{loc_hay} {hay}");
        let loc_match = p
            .locations
            .iter()
            .any(|l| loc_text.contains(&l.to_lowercase()));
        let remote_match =
            p.remote_ok && (loc_text.contains("remote") || loc_text.contains("anywhere"));
        if loc_match || remote_match {
            s += LOCATION;
        }

        // --- seniority ---
        let junior = ["intern", "internship", "junior", "fresher", "trainee", "graduate"]
            .iter()
            .any(|w| title.contains(w));
        let senior_wanted = p.seniority.iter().any(|w| {
            let w = w.to_lowercase();
            w.contains("senior")
                || w.contains("staff")
                || w.contains('2')
                || w.contains("ii")
                || w.contains("lead")
        });
        if senior_wanted && junior {
            s += SENIORITY_MISS;
        } else if p.seniority.iter().any(|w| title.contains(&w.to_lowercase())) {
            s += SENIORITY_HIT;
        }

        // --- optional salary floor ---
        if let Some(min) = p.min_salary {
            if let Some(found) = detect_salary(&hay) {
                if found < min {
                    s += SALARY_PENALTY;
                }
            }
        }

        (s.clamp(0.0, 100.0), matched)
    }
}

/// The 0..1 content component. Resume similarity when a resume is loaded,
/// keyword coverage otherwise, blended by `resume_weight` so you can dial
/// between them rather than flipping.
fn content_score(
    post: &RawPost,
    p: &Settings,
    m: &Matcher,
    hay: &str,
) -> (f64, Vec<String>) {
    let kw = keyword_coverage(p, hay);

    let Some(profile) = m.resume.as_ref() else {
        return (kw, Vec::new());
    };
    let w = p.resume_weight.clamp(0.0, 1.0);
    if w <= 0.0 {
        return (kw, Vec::new());
    }

    let terms = tokenize(&format!("{} {} {}", post.title, post.company, post.body));
    let (sim, matched) = profile.match_post(&terms, &m.corpus);
    (w * sim + (1.0 - w) * kw, matched)
}

/// Fraction of your keyword list present in the post. Kept as the fallback for
/// when no resume is loaded, and as the other half of the blend.
fn keyword_coverage(p: &Settings, hay: &str) -> f64 {
    if p.keywords.is_empty() {
        return 0.0;
    }
    let hits = p
        .keywords
        .iter()
        .filter(|k| hay.contains(&k.to_lowercase()))
        .count();
    hits as f64 / p.keywords.len() as f64
}

/// Pay detector, deliberately conservative.
///
/// The previous version took the largest 6-plus-digit number anywhere in the
/// text, so "serving 1000000 requests/day" read as a salary and quietly cost
/// exactly the infrastructure posts worth wanting 30 points. A number now only
/// counts if a currency or pay word sits within ~24 characters of it.
fn detect_salary(hay: &str) -> Option<u64> {
    const MARKERS: &[&str] = &[
        "salary", "ctc", "lpa", "compensation", "pay", "inr", "usd", "eur", "gbp", "rs.", "rs ",
        "₹", "$", "€", "£", "per annum", "annually", "package",
    ];
    let bytes = hay.as_bytes();
    let mut best: Option<u64> = None;
    let mut i = 0;

    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b',') {
            i += 1;
        }
        let raw: String = hay[start..i].chars().filter(|c| c.is_ascii_digit()).collect();
        if raw.len() < 5 {
            continue;
        }
        let Ok(n) = raw.parse::<u64>() else { continue };
        if n < 100_000 {
            continue;
        }

        // Look at a window either side for something that says "this is money".
        let lo = hay[..start]
            .char_indices()
            .rev()
            .nth(24)
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let hi = hay[i..]
            .char_indices()
            .nth(24)
            .map(|(idx, _)| i + idx)
            .unwrap_or(hay.len());
        let window = &hay[lo..hi];
        if MARKERS.iter().any(|mk| window.contains(mk)) {
            best = Some(best.map_or(n, |b: u64| b.max(n)));
        }
    }
    best
}

/// Fold freshness into the score to produce the queue priority. A newer post of
/// equal match outranks an older one, baking the "apply early" edge into slot
/// selection. Half-life of 2h; never decays below 40% weight.
pub fn priority(score: f64, posted_at: Option<i64>, detected_at: i64, now: i64) -> f64 {
    let ts = posted_at.unwrap_or(detected_at);
    let age_hours = ((now - ts).max(0) as f64) / 3600.0;
    let decay = 0.5f64.powf(age_hours / 2.0);
    let freshness = 0.4 + 0.6 * decay;
    score * freshness
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salary_needs_a_currency_marker_nearby() {
        // The regression that motivated the rewrite.
        assert_eq!(detect_salary("serving 1000000 requests per day"), None);
        assert_eq!(detect_salary("we index 2500000 documents"), None);
        // Real pay still detected, with separators and either side.
        assert_eq!(detect_salary("salary 1,200,000 per annum"), Some(1_200_000));
        assert_eq!(detect_salary("ctc up to 2500000"), Some(2_500_000));
        assert_eq!(detect_salary("$180000 base"), Some(180_000));
    }

    #[test]
    fn freshness_decays_and_floors() {
        let now = 1_000_000;
        let fresh = priority(100.0, Some(now), now, now);
        let two_h = priority(100.0, Some(now - 7200), now, now);
        let ancient = priority(100.0, Some(now - 100 * 3600), now, now);
        assert!((fresh - 100.0).abs() < 0.01);
        assert!((two_h - 70.0).abs() < 0.01, "2h half-life => 0.4+0.6*0.5");
        assert!(ancient >= 40.0, "never decays below the 40% floor");
    }
}
