use crate::model::RawPost;

/// Formal job-board listings are hiring posts by construction. Free-text sources
/// (Voyager feed/content) need to be filtered: we want "we're hiring" posts, not
/// "I'm open to work" posts.
const HIRING_SIGNALS: &[&str] = &[
    "we're hiring", "we are hiring", "hiring", "#hiring", "now hiring",
    "open role", "open position", "open positions", "join our team",
    "join my team", "we're looking for", "we are looking for", "apply now",
    "job opening", "vacancy", "vacancies", "send your resume", "send your cv",
    "dm me your", "actively recruiting", "expanding our team",
];

/// Signals the *poster* is a job seeker, not an employer — suppress these.
const SEEKING_SIGNALS: &[&str] = &[
    "open to work", "opentowork", "seeking new opportunities", "seeking opportunities",
    "looking for a new role", "looking for opportunities", "was laid off",
    "recently laid off", "impacted by layoffs", "available for hire",
    "please refer me", "kindly refer",
];

pub fn is_hiring(post: &RawPost) -> bool {
    // Board sources are already listings.
    if post.source == "greenhouse" || post.source == "linkedin_guest" {
        return true;
    }
    let hay = post.haystack();
    let hiring = HIRING_SIGNALS.iter().filter(|s| hay.contains(**s)).count();
    let seeking = SEEKING_SIGNALS.iter().filter(|s| hay.contains(**s)).count();
    hiring > 0 && hiring >= seeking
}
