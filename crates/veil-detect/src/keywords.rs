//! Layer 1: user keywords and phrases.
//!
//! Matching is case- and accent-insensitive (`Confidencial` matches `confidencial`, `ÉTÉ` matches `ete`),
//! whitespace-insensitive inside phrases, and whole-word: `confidential` does not match `confidentiality`.
//! Findings carry byte spans in the original text so they can be redacted.

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;
use veil_core::Finding;

/// Text folded for matching, with a map back to the original byte offsets.
pub struct Folded {
    pub text: String,
    /// For each byte of `text`: (start, end) byte offsets of the original character it came from.
    map: Vec<(usize, usize)>,
}

impl Folded {
    /// Original byte span for a folded byte span.
    pub fn original_span(&self, start: usize, end: usize) -> std::ops::Range<usize> {
        if start >= end || end > self.map.len() {
            return 0..0;
        }
        self.map[start].0..self.map[end - 1].1
    }
}

/// Fold text: NFKD, drop combining marks, lowercase, collapse whitespace runs to one space.
pub fn fold(text: &str) -> Folded {
    let mut out = String::with_capacity(text.len());
    let mut map = Vec::with_capacity(text.len());
    let mut last_space = false;
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                map.push((start, end));
                last_space = true;
            } else if let Some(m) = map.last_mut() {
                m.1 = end;
            }
            continue;
        }
        last_space = false;
        for d in std::iter::once(ch).nfkd() {
            if is_combining_mark(d) {
                continue;
            }
            for l in d.to_lowercase() {
                let before = out.len();
                out.push(l);
                for _ in before..out.len() {
                    map.push((start, end));
                }
            }
        }
    }
    Folded { text: out, map }
}

fn fold_keyword(k: &str) -> String {
    fold(k.trim()).text.trim().to_string()
}

pub struct KeywordMatcher {
    /// Original keywords, index-aligned with the automaton patterns.
    words: Vec<String>,
    folded: Vec<String>,
    ac: Option<AhoCorasick>,
}

impl KeywordMatcher {
    pub fn new(keywords: &[String]) -> Self {
        let mut words = Vec::new();
        let mut folded = Vec::new();
        for k in keywords {
            let f = fold_keyword(k);
            if f.is_empty() || folded.contains(&f) {
                continue;
            }
            words.push(k.trim().to_string());
            folded.push(f);
        }
        let ac = (!folded.is_empty()).then(|| {
            AhoCorasickBuilder::new().match_kind(MatchKind::Standard).build(&folded).expect("keyword automaton")
        });
        KeywordMatcher { words, folded, ac }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn scan(&self, text: &str, source: &str) -> Vec<Finding> {
        let Some(ac) = &self.ac else { return Vec::new() };
        let folded = fold(text);
        let ft = folded.text.as_str();
        let mut out = Vec::new();
        for m in ac.find_overlapping_iter(ft) {
            let pat = &self.folded[m.pattern().as_usize()];
            if !is_boundary(ft, m.start(), pat, true) || !is_boundary(ft, m.end(), pat, false) {
                continue;
            }
            let span = folded.original_span(m.start(), m.end());
            out.push(Finding::keyword(&self.words[m.pattern().as_usize()], span, source));
        }
        out
    }
}

/// A match edge is a word boundary unless a word character of the pattern touches a word character of the
/// text.
fn is_boundary(text: &str, at: usize, pat: &str, start: bool) -> bool {
    let pat_edge = if start { pat.chars().next() } else { pat.chars().next_back() };
    if !pat_edge.is_some_and(is_word) {
        return true;
    }
    let neighbour = if start { text[..at].chars().next_back() } else { text[at..].chars().next() };
    !neighbour.is_some_and(is_word)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kw(words: &[&str]) -> KeywordMatcher {
        KeywordMatcher::new(&words.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn matches_case_and_accent_insensitively_with_original_spans() {
        let m = kw(&["confidencial", "Project Falcon"]);
        let text = "Esto es CONFIDÉNCIAL y el   project\n falcon sigue.";
        let f = m.scan(text, "message");
        assert_eq!(f.len(), 2);
        assert_eq!(&text[f[0].span.clone().unwrap()], "CONFIDÉNCIAL");
        assert_eq!(&text[f[1].span.clone().unwrap()], "project\n falcon");
        assert_eq!(f[1].kind, veil_core::FindingKind::Keyword("Project Falcon".into()));
    }

    #[test]
    fn whole_words_only() {
        let m = kw(&["confidential"]);
        assert!(m.scan("confidentiality matters", "m").is_empty());
        assert!(m.scan("nonconfidential", "m").is_empty());
        assert_eq!(m.scan("(confidential).", "m").len(), 1);
        assert_eq!(m.scan("confidential_doc", "m").len(), 0);
    }

    #[test]
    fn keywords_with_symbols_match_inside_text() {
        let m = kw(&["#internal", "acme-42"]);
        assert_eq!(m.scan("tag:#internal now", "m").len(), 1);
        assert_eq!(m.scan("ref ACME-42!", "m").len(), 1);
    }

    #[test]
    fn empty_and_duplicate_keywords_are_ignored() {
        let m = kw(&["", "  ", "Secret", "secret"]);
        assert_eq!(m.words, vec!["Secret"]);
        assert!(kw(&[]).scan("anything", "m").is_empty());
    }

    #[test]
    fn ligatures_and_wide_characters_fold() {
        let m = kw(&["office"]);
        assert_eq!(m.scan("the ｏｆｆｉｃｅ plan", "m").len(), 1);
        let text = "the oﬃce plan";
        let f = m.scan(text, "m");
        assert_eq!(&text[f[0].span.clone().unwrap()], "oﬃce");
    }
}
