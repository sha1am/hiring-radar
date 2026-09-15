//! Getting readable text out of a job description.

/// Good-enough tag stripper for scoring and display; not a real HTML parser.
///
/// Every board ships descriptions as HTML, and the scorer wants words. Pulling
/// in a full parser to do this would be the wrong trade: the input is trusted
/// (a jobs API), the output is never re-rendered as HTML, and a mis-stripped
/// `<` costs a word of context at worst.
pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_decodes_entities() {
        let html = "<p>Go &amp; Rust</p><ul><li>5+ yrs</li></ul>";
        assert_eq!(strip_html(html), "Go & Rust5+ yrs");
    }

    #[test]
    fn nbsp_becomes_a_space_not_a_word_join() {
        // Without this, "Go&nbsp;developer" tokenises as one unknown term and
        // the match for "go" is silently lost.
        assert_eq!(strip_html("Go&nbsp;developer"), "Go developer");
    }
}
