use crate::model::RawPost;
use crate::settings::Settings;

/// Match scoring is a swappable trait. The default is a fast, zero-dependency
/// lexical scorer — good enough to drive the pipeline. To upgrade, implement
/// `Scorer` with fastembed cosine similarity or an LLM call and drop it in.
pub trait Scorer: Send + Sync {
    /// Returns a 0..100 match score (freshness is applied separately).
    fn score(&self, post: &RawPost, p: &Settings) -> f64;
}

pub struct LexicalScorer;

impl Scorer for LexicalScorer {
    fn score(&self, post: &RawPost, p: &Settings) -> f64 {
        let title = post.title.to_lowercase();
        let hay = post.haystack();

        // Hard filter: any dealbreaker present zeroes the post out.
        if p.dealbreakers.iter().any(|d| hay.contains(&d.to_lowercase())) {
            return 0.0;
        }

        let mut s = 0.0f64;

        // Title match is the strongest signal.
        let full_title_hit = p.titles.iter().any(|t| title.contains(&t.to_lowercase()));
        if full_title_hit {
            s += 45.0;
        } else {
            // Partial: any word of a target title appears in the post title.
            let title_words: Vec<String> = p
                .titles
                .iter()
                .flat_map(|t| t.to_lowercase().split_whitespace().map(String::from).collect::<Vec<_>>())
                .filter(|w| w.len() > 2)
                .collect();
            if title_words.iter().any(|w| title.contains(w.as_str())) {
                s += 20.0;
            }
        }

        // Keyword coverage across the whole post.
        if !p.keywords.is_empty() {
            let hits = p.keywords.iter().filter(|k| hay.contains(&k.to_lowercase())).count();
            let coverage = hits as f64 / p.keywords.len() as f64;
            s += coverage * 30.0;
        }

        // Location fit.
        let loc_hay = post
            .location
            .as_deref()
            .map(|l| l.to_lowercase())
            .unwrap_or_default();
        let loc_text = format!("{loc_hay} {hay}");
        let loc_match = p.locations.iter().any(|l| loc_text.contains(&l.to_lowercase()));
        let remote_match = p.remote_ok && (loc_text.contains("remote") || loc_text.contains("anywhere"));
        if loc_match || remote_match {
            s += 10.0;
        }

        // Seniority alignment.
        let wants = &p.seniority;
        let junior = ["intern", "internship", "junior", "fresher", "trainee", "graduate"]
            .iter()
            .any(|w| title.contains(w));
        let senior_wanted = wants.iter().any(|w| {
            let w = w.to_lowercase();
            w.contains("senior") || w.contains("staff") || w.contains("2") || w.contains("ii") || w.contains("lead")
        });
        if senior_wanted && junior {
            s -= 20.0; // clearly wrong level
        } else if wants.iter().any(|w| title.contains(&w.to_lowercase())) {
            s += 15.0;
        }

        // Optional salary penalty.
        if let Some(min) = p.min_salary {
            if let Some(found) = detect_salary(&hay) {
                if found < min {
                    s -= 30.0;
                }
            }
        }

        s.clamp(0.0, 100.0)
    }
}

/// Very rough pay detector: grabs the largest plain number >= 100000 in the text.
/// Deliberately conservative — only used to penalise clearly-below-floor posts.
fn detect_salary(hay: &str) -> Option<u64> {
    let mut best: Option<u64> = None;
    for token in hay.split(|c: char| !c.is_ascii_digit()) {
        if token.len() >= 6 {
            if let Ok(n) = token.parse::<u64>() {
                if n >= 100_000 {
                    best = Some(best.map_or(n, |b| b.max(n)));
                }
            }
        }
    }
    best
}

/// Fold freshness into the score to produce the queue priority. A newer post of
/// equal match outranks an older one, baking the "apply early" edge into
/// slot selection. Half-life of 2h; never decays below 40% weight.
pub fn priority(score: f64, posted_at: Option<i64>, detected_at: i64, now: i64) -> f64 {
    let ts = posted_at.unwrap_or(detected_at);
    let age_hours = ((now - ts).max(0) as f64) / 3600.0;
    let decay = 0.5f64.powf(age_hours / 2.0);
    let freshness = 0.4 + 0.6 * decay;
    score * freshness
}
