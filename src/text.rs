/// Whole-word substring matching, shared by the gazetteer and the tag extractor.
///
/// Both need it for the same reason: plain `contains` makes "pune" match
/// "Puneet", "india" match "Indiana", "go" match "Django", and "java" match
/// "javascript". Every one of those turns up in real hiring posts, and each
/// produces a wrong row the user has no way to explain.
fn boundary(hay: &str, start: usize, end: usize) -> bool {
    let before_ok = start == 0
        || !hay[..start]
            .chars()
            .next_back()
            .map(is_word_char)
            .unwrap_or(false);
    let after_ok =
        end >= hay.len() || !hay[end..].chars().next().map(is_word_char).unwrap_or(false);
    before_ok && after_ok
}

/// `+` and `#` count as part of a word so "c++" and "c#" are not truncated to
/// "c", but they are only word characters when a letter or digit precedes them.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

pub fn contains_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        let start = from + i;
        let end = start + needle.len();
        if boundary(hay, start, end) {
            return true;
        }
        from = end;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_whole_words_only() {
        assert!(contains_word("we use kafka here", "kafka"));
        assert!(!contains_word("kafkaesque", "kafka"));
        assert!(!contains_word("django", "go"));
        assert!(!contains_word("javascript", "java"));
    }

    #[test]
    fn symbol_suffixes_are_part_of_the_word() {
        assert!(contains_word("we write c++ daily", "c++"));
        assert!(contains_word("c# shop", "c#"));
    }

    #[test]
    fn handles_edges() {
        assert!(contains_word("go", "go"));
        assert!(!contains_word("", "go"));
        assert!(!contains_word("go", ""));
    }
}
