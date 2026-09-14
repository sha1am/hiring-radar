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

        // A feed post has no title field — `title` is just its first line — so
        // matching target job titles against it is close to meaningless. Rather
        // than handing every such post a structural zero (which would park the
        // entire LinkedIn-posts mode below the floor), the title budget moves
        // into content, where the post's actual text can earn it.
        let content_budget = if post.synthetic_title {
            CONTENT + TITLE_FULL
        } else {
            CONTENT
        };

        // --- title: the strongest single signal ---
        if post.synthetic_title {
            // no title signal available
        } else if p.titles.iter().any(|t| title.contains(&t.to_lowercase())) {
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
        s += content_frac * content_budget;

        // --- location ---
        // Rejection is the pipeline's call, not the scorer's — a post filtered
        // out for being in the wrong place needs to be *reported* as that,
        // rather than showing up as a mysteriously low score.
        if location_verdict(post, p) == LocationVerdict::Match {
            s += LOCATION;
        }

        // --- seniority ---
        // For a feed post the level words are in the body, not the first line.
        let level_hay: &str = if post.synthetic_title { &hay } else { &title };
        let junior = ["intern", "internship", "junior", "fresher", "trainee", "graduate"]
            .iter()
            .any(|w| level_hay.contains(w));
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
        } else if p.seniority.iter().any(|w| level_hay.contains(&w.to_lowercase())) {
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

#[derive(Debug, PartialEq, Eq)]
pub enum LocationVerdict {
    /// Somewhere you want to work.
    Match,
    /// Somewhere else, or unknown — scored without the location bonus.
    NoMatch,
    /// Somewhere else, under a policy that says don't show me those at all.
    Rejected,
}

/// Decide a post's location against the profile.
///
/// The whole point of "require" is that a location miss cannot be outvoted. As
/// a bonus worth ten points out of a hundred, a San Francisco role with a
/// perfect resume match still cleared the floor comfortably, which made "only
/// send me jobs in India" a suggestion rather than a filter.
pub fn location_verdict(post: &RawPost, p: &Settings) -> LocationVerdict {
    if p.location_policy == "off" || p.locations.is_empty() {
        return LocationVerdict::NoMatch;
    }

    let stated = post.location.as_deref().unwrap_or("");
    let body = post.haystack();
    // The stated field is authoritative when it names a place; the body is the
    // fallback, which is why a listing whose location reads "Remote" but whose
    // text says "India preferred" still resolves.
    let place = crate::geo::lookup(stated).or_else(|| crate::geo::lookup(&body));

    let hay = format!("{} {}", stated.to_lowercase(), body);

    let matched = p.locations.iter().any(|wanted| {
        // Canonical comparison first: it is what makes "Gurugram" match a post
        // saying "Gurgaon", and "India" match a post in Bengaluru.
        if let (Some(place), Some(wanted)) = (place, crate::geo::canonical(wanted)) {
            if crate::geo::satisfies(place, wanted) {
                return true;
            }
        }
        // Fallback for anywhere the gazetteer has never heard of — a suburb, an
        // office park, a country we do not list. Whole-word, so "pune" does not
        // match "Puneet".
        crate::geo::contains_word(&hay, &wanted.trim().to_lowercase())
    });

    let remote_match = p.remote_ok && crate::geo::is_remote(&hay);

    if matched || remote_match {
        return LocationVerdict::Match;
    }
    if p.location_policy != "require" {
        return LocationVerdict::NoMatch;
    }

    // Under "require": did we fail to match, or fail to tell? Those deserve
    // different treatment, and conflating them either floods the board with
    // foreign roles or silently bins every feed post that never names a city.
    if place.is_some() || !p.allow_unknown_location {
        LocationVerdict::Rejected
    } else {
        LocationVerdict::NoMatch
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
    use crate::model::ApplyChannel;
    use crate::resume::Matcher;
    use crate::settings::Settings;

    fn profile() -> Settings {
        let mut s: Settings = serde_json::from_str("{}").expect("all fields default");
        s.titles = vec!["backend engineer".into(), "software engineer".into()];
        s.keywords = vec![
            "golang".into(),
            "kafka".into(),
            "postgres".into(),
            "kubernetes".into(),
        ];
        s.locations = vec!["bengaluru".into()];
        s.seniority = vec!["senior".into()];
        s.remote_ok = true;
        s
    }

    fn post(title: &str, body: &str, synthetic: bool) -> RawPost {
        RawPost {
            source: "test".into(),
            external_id: "1".into(),
            url: "https://example.test/1".into(),
            title: title.into(),
            company: "Someone".into(),
            location: None,
            body: body.into(),
            posted_at: None,
            apply: ApplyChannel::Unknown,
            synthetic_title: synthetic,
        }
    }

    /// The whole LinkedIn-posts mode rests on this. A feed post has no title
    /// field, so its "title" is just the first line — under the old scoring it
    /// forfeited the 40-point title budget outright and parked below any sane
    /// floor, which would have made the mode look broken rather than empty.
    #[test]
    fn titleless_feed_post_still_scores() {
        let p = profile();
        let m = Matcher::default();
        let body = "We are hiring! Our team in Bengaluru is looking for a senior \
                    backend engineer to work on golang services, kafka pipelines, \
                    postgres and kubernetes. DM me if interested. #hiring";

        // Same text, once as a feed post (no usable title) and once as a listing.
        let feed = LexicalScorer.score(&post("We are hiring!", body, true), &p, &m).0;
        let listing = LexicalScorer
            .score(&post("Senior Backend Engineer", body, false), &p, &m)
            .0;

        assert!(
            feed >= 55.0,
            "a well-matched feed post must clear a normal floor, got {feed}"
        );
        // It should land in the same league as the equivalent listing, not 40 adrift.
        assert!(
            (feed - listing).abs() < 20.0,
            "feed {feed} and listing {listing} should be comparable"
        );
    }

    /// Redistributing the title budget must not make junk score well.
    #[test]
    fn titleless_irrelevant_post_still_loses() {
        let p = profile();
        let m = Matcher::default();
        let junk = post(
            "Thrilled to share!",
            "Thrilled to share that I have completed a certification in digital \
             marketing analytics. Grateful to my mentors. #blessed #marketing",
            true,
        );
        let score = LexicalScorer.score(&junk, &p, &m).0;
        assert!(score < 40.0, "irrelevant feed post scored {score}");
    }

    fn india_profile(policy: &str) -> Settings {
        let mut p = profile();
        p.locations = vec!["india".into(), "bengaluru".into(), "gurugram".into()];
        p.location_policy = policy.into();
        p.remote_ok = false;
        p
    }

    /// The behaviour that was actually missing: as a ten-point bonus, a strong
    /// match in the wrong country still cleared the floor and alerted.
    #[test]
    fn prefer_lets_a_foreign_job_through_require_does_not() {
        let m = Matcher::default();
        let sf = RawPost {
            location: Some("San Francisco, CA".into()),
            ..post(
                "Senior Backend Engineer",
                "Golang, kafka, postgres, kubernetes. Distributed systems at scale.",
                false,
            )
        };

        let preferred = LexicalScorer.score(&sf, &india_profile("prefer"), &m).0;
        assert!(
            preferred > 55.0,
            "under prefer a strong foreign match still scores well: {preferred}"
        );
        assert_eq!(
            location_verdict(&sf, &india_profile("prefer")),
            LocationVerdict::NoMatch
        );
        assert_eq!(
            location_verdict(&sf, &india_profile("require")),
            LocationVerdict::Rejected,
            "require must reject it outright, not merely score it lower"
        );
    }

    #[test]
    fn require_keeps_indian_jobs() {
        let blr = RawPost {
            location: Some("Bengaluru, India".into()),
            ..post("Senior Backend Engineer", "Golang and kafka.", false)
        };
        assert_eq!(
            location_verdict(&blr, &india_profile("require")),
            LocationVerdict::Match
        );
    }

    /// Feed posts state no location field, so the pipeline infers one from the
    /// text. Both directions have to work or the filter is useless there.
    #[test]
    fn require_reads_the_body_for_feed_posts() {
        let p = india_profile("require");
        let indian = post(
            "We are hiring!",
            "Our Gurgaon team is looking for a backend engineer. Golang, kafka.",
            true,
        );
        let foreign = post(
            "We are hiring!",
            "Our Berlin office is looking for a backend engineer. Golang, kafka.",
            true,
        );
        assert_eq!(location_verdict(&indian, &p), LocationVerdict::Match);
        assert_eq!(location_verdict(&foreign, &p), LocationVerdict::Rejected);
    }

    /// A post that names no place at all is a judgement call, not a miss, so it
    /// gets its own switch rather than a silent default.
    #[test]
    fn unknown_location_follows_its_own_switch() {
        let unknown = post("We are hiring!", "Backend engineer wanted. Golang, kafka.", true);

        let mut lenient = india_profile("require");
        lenient.allow_unknown_location = true;
        assert_eq!(location_verdict(&unknown, &lenient), LocationVerdict::NoMatch);

        let mut strict = india_profile("require");
        strict.allow_unknown_location = false;
        assert_eq!(location_verdict(&unknown, &strict), LocationVerdict::Rejected);
    }

    #[test]
    fn remote_counts_when_remote_is_acceptable() {
        let mut p = india_profile("require");
        p.remote_ok = true;
        let remote = post(
            "We are hiring!",
            "Fully remote backend engineer role. Golang, kafka.",
            true,
        );
        assert_eq!(location_verdict(&remote, &p), LocationVerdict::Match);
    }

    /// "require" with an empty location list would discard every post; sanitize
    /// downgrades it rather than silently emptying the board.
    #[test]
    fn require_without_locations_is_downgraded() {
        let mut p = profile();
        p.locations.clear();
        p.location_policy = "require".into();
        p.sanitize();
        assert_eq!(p.location_policy, "prefer");
    }

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
