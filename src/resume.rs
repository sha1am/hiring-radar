use std::collections::{HashMap, HashSet};

/// Resume-driven matching.
///
/// The keyword scorer it replaces had two structural problems: you had to
/// hand-maintain the keyword list, and it matched by raw substring, so "go" hit
/// *Mongo*, *Django* and *category* on every long job description. This scores
/// a post by how much of *your actual resume* it looks like, using IDF-weighted
/// cosine similarity over unigrams and bigrams.
///
/// IDF comes from the corpus of posts this instance has actually seen, which
/// means "distributed systems" earns weight and "responsibilities" does not,
/// without anyone curating a list. Cold start degrades to plain term frequency
/// — every term gets the same idf — and sharpens as posts accumulate.
///
/// Deliberately not an LLM: this runs on the instant path, for every post, and
/// has to be deterministic enough that a score you see on the dashboard is the
/// same one the release engine acted on.

/// English filler plus job-posting boilerplate. Boilerplate matters more than
/// grammar words here — "responsibilities" and "candidate" appear in every
/// posting and in most resumes, so they are pure noise in a similarity score.
const STOPWORDS: &[&str] = &[
    "a", "about", "above", "across", "after", "all", "also", "am", "an", "and", "any", "are", "as",
    "at", "be", "because", "been", "being", "both", "but", "by", "can", "could", "did", "do",
    "does", "doing", "during", "each", "for", "from", "further", "had", "has", "have", "having",
    "he", "her", "here", "hers", "him", "his", "how", "i", "if", "in", "into", "is", "it", "its",
    "just", "me", "more", "most", "my", "no", "nor", "not", "now", "of", "off", "on", "once",
    "only", "or", "other", "our", "ours", "out", "over", "own", "same", "she", "should", "so",
    "some", "such", "than", "that", "the", "their", "theirs", "them", "then", "there", "these",
    "they", "this", "those", "through", "to", "too", "under", "until", "up", "very", "was", "we",
    "were", "what", "when", "where", "which", "while", "who", "whom", "why", "will", "with",
    "would", "you", "your", "yours",
    // job-posting boilerplate
    "ability", "applicant", "applicants", "apply", "based", "benefits", "candidate", "candidates",
    "career", "colleagues", "company", "compensation", "culture", "description", "diverse",
    "diversity", "employee", "employees", "employer", "employment", "equal", "experience",
    "experienced", "get", "global", "great", "growth", "help", "hiring", "including", "inclusive",
    "job", "join", "like", "look", "looking", "love", "make", "mission", "must", "need", "new",
    "offer", "office", "opportunity", "organization", "people", "plus", "position", "preferred",
    // resume-verb boilerplate: present in every resume, so pure noise
    "built", "build", "building", "created", "designed", "developed", "development",
    "implemented", "led", "managed", "operated", "owned", "responsible", "wrote",
    "proven", "qualifications", "range", "required", "requirements", "resume", "role", "roles",
    "salary", "skills", "strong", "team", "teams", "us", "using", "want", "work", "working",
    "world", "year", "years",
];

fn stopwords() -> &'static HashSet<&'static str> {
    use std::sync::OnceLock;
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| STOPWORDS.iter().copied().collect())
}

/// Split text into scoring terms: unigrams plus adjacent bigrams.
///
/// Bigrams matter more than they look. "distributed systems", "event driven"
/// and "machine learning" are the terms that actually separate one backend role
/// from another, and unigrams alone dissolve them into common words.
///
/// `+ # . -` survive inside a token so c++, c#, node.js and ci-cd stay intact.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();

    for ch in text.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '.' | '-' | '_') {
            cur.push(c);
        } else if !cur.is_empty() {
            push_word(&mut words, std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        push_word(&mut words, cur);
    }

    let sw = stopwords();
    let kept: Vec<&String> = words.iter().filter(|w| !sw.contains(w.as_str())).collect();

    let mut out: Vec<String> = kept.iter().map(|w| (*w).clone()).collect();
    // Bigrams over the stopword-filtered stream, so "experience in distributed
    // systems" still yields "distributed systems".
    for pair in kept.windows(2) {
        out.push(format!("{} {}", pair[0], pair[1]));
    }
    out
}

fn push_word(out: &mut Vec<String>, mut w: String) {
    // Trailing punctuation that only survived because it's in the allowed set.
    while w.ends_with('.') || w.ends_with('-') || w.ends_with('_') {
        w.pop();
    }
    while w.starts_with('-') || w.starts_with('_') || w.starts_with('.') {
        w.remove(0);
    }
    if w.len() < 2 || w.len() > 40 {
        return;
    }
    // Bare numbers carry no signal; "c++" and "k8s" do.
    if w.chars().all(|c| c.is_ascii_digit()) {
        return;
    }
    out.push(w);
}

fn term_freq(terms: &[String]) -> HashMap<String, f64> {
    let mut tf: HashMap<String, f64> = HashMap::new();
    for t in terms {
        *tf.entry(t.clone()).or_insert(0.0) += 1.0;
    }
    // Sublinear: a resume saying "go" nine times is not nine times more about Go.
    for v in tf.values_mut() {
        *v = 1.0 + v.ln();
    }
    tf
}

/// Document frequencies over the posts this instance has seen.
#[derive(Default, Clone, Debug)]
pub struct Corpus {
    pub docs: u64,
    pub df: HashMap<String, u64>,
}

/// Below this many documents, document frequencies are noise: on a handful of
/// posts every term looks either universal or unique, and weighting by that
/// produces nonsense — "mysql" outranking "kubernetes" purely because one
/// fixture mentioned it. IDF influence therefore ramps in with corpus size and
/// is flat (every term equal) until there is something to learn from.
const IDF_CONFIDENCE_DOCS: f64 = 50.0;

impl Corpus {
    pub fn idf(&self, term: &str) -> f64 {
        let df = self.df.get(term).copied().unwrap_or(0);
        let raw = ((self.docs as f64 + 1.0) / (df as f64 + 1.0)).ln();
        let confidence = (self.docs as f64 / IDF_CONFIDENCE_DOCS).clamp(0.0, 1.0);
        1.0 + raw * confidence
    }

    /// Record one document's distinct terms.
    pub fn observe(&mut self, terms: &[String]) {
        let distinct: HashSet<&String> = terms.iter().collect();
        self.docs += 1;
        for t in distinct {
            *self.df.entry(t.clone()).or_insert(0) += 1;
        }
        if self.df.len() > 400_000 {
            self.compact();
        }
    }

    /// Terms seen exactly once carry no discriminative power yet and are most
    /// of the map. Dropping them keeps memory bounded; if they matter they come
    /// back on the next occurrence.
    fn compact(&mut self) {
        self.df.retain(|_, v| *v > 1);
        tracing::info!(terms = self.df.len(), "corpus compacted");
    }
}

/// The resume, pre-vectorised against a corpus snapshot.
#[derive(Clone, Debug)]
pub struct ResumeProfile {
    /// L2-normalised tf-idf vector.
    vec: HashMap<String, f64>,
    /// The most distinctive terms, highest weight first — what "coverage" is
    /// measured against, and what the UI shows as the reason for a match.
    top: Vec<(String, f64)>,
    top_weight: f64,
}

/// How much cosine similarity counts as a perfect topical match. Resume-to-JD
/// cosine rarely exceeds this even for an ideal fit, because a resume carries
/// employers, dates and projects the posting never mentions.
const COS_FULL: f64 = 0.35;
/// How many of the resume's distinctive terms define "coverage".
const TOP_TERMS: usize = 40;

impl ResumeProfile {
    pub fn build(resume_text: &str, corpus: &Corpus) -> Option<Self> {
        let terms = tokenize(resume_text);
        if terms.len() < 30 {
            return None; // not enough to be a resume
        }
        let tf = term_freq(&terms);

        let mut weighted: HashMap<String, f64> = HashMap::new();
        for (t, f) in &tf {
            weighted.insert(t.clone(), f * corpus.idf(t));
        }
        let norm = weighted.values().map(|v| v * v).sum::<f64>().sqrt();
        if norm <= 0.0 {
            return None;
        }
        for v in weighted.values_mut() {
            *v /= norm;
        }

        let mut top: Vec<(String, f64)> = weighted.iter().map(|(k, v)| (k.clone(), *v)).collect();
        top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        top.truncate(TOP_TERMS);
        let top_weight: f64 = top.iter().map(|(_, w)| *w).sum();

        Some(Self { vec: weighted, top, top_weight })
    }

    /// Returns (0.0..=1.0 match, the resume terms this post hit).
    ///
    /// Two halves, because either alone misleads. Cosine rewards overall
    /// topical fit but a long post of generic prose can drift upward; coverage
    /// asks the blunter question — how many of *your* signature terms are
    /// actually here — but ignores everything else the post says.
    pub fn match_post(&self, post_terms: &[String], corpus: &Corpus) -> (f64, Vec<String>) {
        if post_terms.is_empty() {
            return (0.0, Vec::new());
        }
        let tf = term_freq(post_terms);
        let mut weighted: HashMap<String, f64> = HashMap::with_capacity(tf.len());
        for (t, f) in &tf {
            weighted.insert(t.clone(), f * corpus.idf(t));
        }
        let norm = weighted.values().map(|v| v * v).sum::<f64>().sqrt();
        if norm <= 0.0 {
            return (0.0, Vec::new());
        }

        let mut cos = 0.0;
        for (t, w) in &weighted {
            if let Some(rw) = self.vec.get(t) {
                cos += (w / norm) * rw;
            }
        }

        let mut hits: Vec<(String, f64)> = Vec::new();
        let mut covered = 0.0;
        for (t, w) in &self.top {
            if weighted.contains_key(t) {
                covered += w;
                hits.push((t.clone(), *w));
            }
        }
        let coverage = if self.top_weight > 0.0 { covered / self.top_weight } else { 0.0 };

        let sim = (cos / COS_FULL).clamp(0.0, 1.0);
        let score = (0.5 * coverage + 0.5 * sim).clamp(0.0, 1.0);

        hits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(8);
        (score, hits.into_iter().map(|(t, _)| t).collect())
    }

    /// The resume's signature terms, for showing the user what was derived.
    pub fn top_terms(&self, n: usize) -> Vec<String> {
        self.top.iter().take(n).map(|(t, _)| t.clone()).collect()
    }
}

/// Corpus plus the current resume vector, rebuilt when either changes.
#[derive(Default, Clone, Debug)]
pub struct Matcher {
    pub corpus: Corpus,
    pub resume: Option<ResumeProfile>,
}

impl Matcher {
    pub fn rebuild_resume(&mut self, resume_text: &str) {
        self.resume = if resume_text.trim().is_empty() {
            None
        } else {
            ResumeProfile::build(resume_text, &self.corpus)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_tech_terms_intact() {
        let t = tokenize("Built c++ and node.js services with CI-CD on k8s");
        assert!(t.contains(&"c++".to_string()));
        assert!(t.contains(&"node.js".to_string()));
        assert!(t.contains(&"ci-cd".to_string()));
        assert!(t.contains(&"k8s".to_string()));
    }

    #[test]
    fn drops_stopwords_and_bare_numbers() {
        let t = tokenize("the and 2019 golang");
        assert!(t.contains(&"golang".to_string()));
        assert!(!t.iter().any(|x| x == "the"));
        assert!(!t.iter().any(|x| x == "2019"));
    }

    #[test]
    fn produces_bigrams_across_removed_stopwords() {
        let t = tokenize("experience in distributed systems");
        // "experience" is boilerplate, so the bigram forms from what remains.
        assert!(t.contains(&"distributed systems".to_string()));
    }

    /// The behaviour that actually matters: a relevant post must outscore an
    /// irrelevant one against the same resume.
    #[test]
    fn ranks_relevant_above_irrelevant() {
        let resume = "Backend engineer. Golang and Rust services. Kafka event pipelines, \
             PostgreSQL, Redis, Kubernetes on AWS. Distributed systems and microservices. \
             Elasticsearch search infrastructure. Built high throughput ingestion pipelines \
             and gRPC APIs. Docker, Terraform, observability with Prometheus.";
        let relevant = "We are hiring a backend engineer to build distributed systems in \
             Golang. You will own Kafka pipelines, PostgreSQL schema design and Kubernetes \
             deployments on AWS. Redis and Elasticsearch experience valued. gRPC microservices.";
        let irrelevant = "Seeking a frontend designer to own our brand identity, produce \
             marketing collateral, run social campaigns and manage the editorial calendar. \
             Figma, Illustrator and copywriting.";

        let mut corpus = Corpus::default();
        for d in [resume, relevant, irrelevant] {
            corpus.observe(&tokenize(d));
        }
        let p = ResumeProfile::build(resume, &corpus).expect("profile builds");

        let (good, hits) = p.match_post(&tokenize(relevant), &corpus);
        let (bad, _) = p.match_post(&tokenize(irrelevant), &corpus);

        assert!(good > bad, "relevant {good} should beat irrelevant {bad}");
        assert!(good > 0.25, "relevant post scored too low: {good}");
        assert!(bad < 0.15, "irrelevant post scored too high: {bad}");
        assert!(!hits.is_empty(), "should report why it matched");
    }

    #[test]
    fn short_text_is_not_a_resume() {
        assert!(ResumeProfile::build("hi", &Corpus::default()).is_none());
    }
}
