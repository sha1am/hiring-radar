use crate::model::RawPost;

/// Formal job-board listings are hiring posts by construction. Free-text sources
/// (the LinkedIn feed) need to be filtered: we want "we're hiring" posts, not
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
    // A post with a real title came from something that has a title field —
    // which is to say, a job board, where every row is a listing. Only sources
    // that had to invent a title from free text need the keyword filter.
    //
    // This deliberately keys off the data rather than a list of source names.
    // The name list was the bug: Workday landed, wasn't on it, and every
    // requisition it fetched was discarded as "not a hiring post" — visible
    // only as a counter on the status panel. A new board source cannot forget
    // to add itself to a property it already sets.
    if !post.synthetic_title {
        return true;
    }
    let hay = post.haystack();
    let hiring = HIRING_SIGNALS.iter().filter(|s| hay.contains(**s)).count();
    let seeking = SEEKING_SIGNALS.iter().filter(|s| hay.contains(**s)).count();
    hiring > 0 && hiring >= seeking
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ApplyChannel;

    fn post(source: &str, title: &str, body: &str, synthetic: bool) -> RawPost {
        RawPost {
            source: source.into(),
            external_id: "x".into(),
            url: "https://example.com".into(),
            title: title.into(),
            company: "Acme".into(),
            location: None,
            body: body.into(),
            posted_at: None,
            apply: ApplyChannel::Unknown,
            synthetic_title: synthetic,
        }
    }

    #[test]
    fn every_board_listing_is_a_hiring_post_whatever_it_says() {
        // No board listing contains the word "hiring", and requiring it dropped
        // every Workday requisition on the floor.
        for source in ["greenhouse", "linkedin_guest", "workday", "some_future_board"] {
            let p = post(source, "Backend Engineer", "Go, gRPC, Postgres.", false);
            assert!(is_hiring(&p), "{source} listing was discarded");
        }
    }

    #[test]
    fn a_feed_post_still_has_to_say_it_is_hiring() {
        let p = post("linkedin_voyager", "Thoughts on Go generics", "A thread.", true);
        assert!(!is_hiring(&p));
    }

    #[test]
    fn we_are_hiring_passes_and_open_to_work_does_not() {
        assert!(is_hiring(&post(
            "linkedin_voyager",
            "We're hiring backend engineers",
            "DM me your resume",
            true
        )));
        assert!(!is_hiring(&post(
            "linkedin_voyager",
            "Open to work",
            "Recently laid off, looking for opportunities",
            true
        )));
    }
}
