//! Session credentials for the authenticated LinkedIn API.
//!
//! These are your real login. The cookie in `.env` is the same one your browser
//! holds, and anything done with it is done as you — which is why it stays in
//! the environment, never in the settings row, and never on screen.

/// The cookie header and CSRF token a Voyager call needs.
#[derive(Debug, Clone)]
pub struct Session {
    pub cookie: String,
    pub csrf: String,
}

/// LinkedIn's CSRF token is the JSESSIONID value verbatim, quotes stripped.
/// Not a design you would choose; it is the one the web app uses, and a
/// mismatch is a silent 403.
pub fn session(li_at: &str, jsessionid: Option<&str>) -> Session {
    let jsession = jsessionid.unwrap_or("ajax:0000000000000000000");
    Session {
        cookie: format!("li_at={li_at}; JSESSIONID=\"{jsession}\""),
        csrf: jsession.trim_matches('"').to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csrf_is_the_jsessionid_without_quotes() {
        let s = session("COOKIE", Some("\"ajax:12345\""));
        assert_eq!(s.csrf, "ajax:12345");
        assert!(s.cookie.contains("li_at=COOKIE"));
    }

    #[test]
    fn a_missing_jsessionid_still_produces_a_usable_pair() {
        let s = session("COOKIE", None);
        assert!(!s.csrf.is_empty());
        assert!(s.cookie.contains("JSESSIONID"));
    }
}
