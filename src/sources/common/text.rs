//! Small normalisations every source ends up needing.

/// The email someone told you to apply to, if the text names one.
///
/// Hiring posts say "send your CV to x@y.com" far more often than they link an
/// application form, so this is the difference between a draft you can send and
/// a card you can only stare at.
pub fn extract_email(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '@' && c != '.'))
        .find(|w| match w.find('@') {
            Some(i) => i > 0 && w[i + 1..].contains('.') && !w.ends_with('.'),
            None => false,
        })
        .map(|w| w.to_string())
}

/// A feed post has no title field. Its first non-empty line is the closest
/// thing, capped so one run-on paragraph doesn't become the whole card.
pub fn first_line(text: &str, max: usize) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Hiring post")
        .chars()
        .take(max)
        .collect()
}

/// A URL-ish identifier as a company name: `some-company` -> `Some Company`.
///
/// Board tokens and Workday tenants are all you get for a company name from
/// those APIs, and "ramp" in a notification reads as a typo while "Ramp" reads
/// as a company.
pub fn pretty_slug(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Drop tracking query parameters so the same posting seen twice is the same URL.
pub fn clean_url(href: &str) -> String {
    href.split('?').next().unwrap_or(href).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_apply_address_in_a_sentence() {
        assert_eq!(
            extract_email("Interested? Mail me at hire@acme.io, thanks!"),
            Some("hire@acme.io".into())
        );
    }

    #[test]
    fn a_bare_handle_is_not_an_email() {
        assert_eq!(extract_email("ping @shadab for details"), None);
    }

    #[test]
    fn first_line_skips_leading_blanks() {
        assert_eq!(
            first_line("\n\n  We're hiring!  \nBackend", 40),
            "We're hiring!"
        );
    }

    #[test]
    fn slug_becomes_a_company_name() {
        assert_eq!(pretty_slug("some-company"), "Some Company");
        assert_eq!(pretty_slug("ramp"), "Ramp");
    }
}
