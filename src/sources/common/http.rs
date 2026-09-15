//! Turning transport failures into something the status panel can show.

/// reqwest errors stringify into a paragraph. Keep the cause, drop the URL echo
/// and the nested "error trying to connect:" chain.
pub fn brief(e: &impl std::fmt::Display) -> String {
    let s = e.to_string();
    let tail = s.rsplit(':').next().unwrap_or(&s).trim().to_string();
    if tail.is_empty() {
        s
    } else {
        tail
    }
}

/// What a status code means *for a crawler*, in the words of the fix.
///
/// A bare "HTTP 403" in the dashboard tells you nothing you can act on. The
/// interesting part is always the same three questions — is my cookie dead, am
/// I going too fast, did the endpoint move — and the code answers them.
pub fn status_hint(code: u16) -> &'static str {
    match code {
        401 => " — not authenticated; the cookie is missing or dead",
        403 => " — rejected; the cookie expired or the endpoint now refuses bots",
        404 => " — no such target; check the identifier",
        429 => " — rate limited; widen the poll interval or fetch fewer pages",
        400 => " — the request shape was rejected; the API changed",
        500..=599 => " — the site is having problems; this usually clears on its own",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_keeps_the_cause_not_the_chain() {
        struct E;
        impl std::fmt::Display for E {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "error sending request for url (https://x/y): connection refused")
            }
        }
        assert_eq!(brief(&E), "connection refused");
    }

    #[test]
    fn every_hint_reads_as_a_continuation() {
        // These get concatenated onto "HTTP 404", so a hint that doesn't start
        // with the separator produces "HTTP 404no such target".
        for code in [400u16, 401, 403, 404, 429, 503] {
            let h = status_hint(code);
            assert!(h.starts_with(" — "), "hint for {code} is not a continuation: {h:?}");
        }
        assert_eq!(status_hint(200), "");
    }
}
