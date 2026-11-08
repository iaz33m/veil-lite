//! "Redact & send": replace detected spans with masks.

use crate::findings::{Finding, FindingKind};

/// True when every finding points at a span of the text (whole-text AI verdicts cannot be redacted).
pub fn can_redact(findings: &[Finding]) -> bool {
    !findings.is_empty() && findings.iter().all(|f| f.span.is_some())
}

/// Replace each finding's span with a mask. Overlapping spans are merged; the widest mask wins.
pub fn redact(text: &str, findings: &[Finding]) -> String {
    let mut spans: Vec<(usize, usize, String)> = findings
        .iter()
        .filter_map(|f| {
            let span = f.span.clone()?;
            let (start, end) = (floor_boundary(text, span.start), ceil_boundary(text, span.end));
            (start < end).then(|| (start, end, mask_for(f, &text[start..end])))
        })
        .collect();
    spans.sort_by_key(|s| (s.0, std::cmp::Reverse(s.1)));

    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for (start, end, mask) in spans {
        if start < pos {
            // Overlaps the previous span: extend the cut if needed, keep the earlier mask.
            pos = pos.max(end);
            continue;
        }
        out.push_str(&text[pos..start]);
        out.push_str(&mask);
        pos = end;
    }
    out.push_str(&text[pos.min(text.len())..]);
    out
}

fn mask_for(f: &Finding, value: &str) -> String {
    match &f.kind {
        FindingKind::Keyword(_) => "[REDACTED]".into(),
        FindingKind::Pii(entity) => {
            let e = entity.to_ascii_uppercase();
            if e == "CREDIT_CARD" || e == "IBAN" || e == "IBAN_CODE" {
                let digits: String = value.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                let tail: String = digits.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
                format!("****{tail}")
            } else {
                format!("[{e}]")
            }
        }
        FindingKind::Check(c) => format!("[{}]", c.key().to_ascii_uppercase()),
    }
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::{CheckId, Finding};

    #[test]
    fn masks_pii_and_keywords() {
        let text = "mail bob@corp.com, card 4111 1111 1111 1111, confidential plan";
        let f = vec![
            Finding::pii("EMAIL_ADDRESS", 5..17, 0.9, "m"),
            Finding::pii("CREDIT_CARD", 24..43, 0.9, "m"),
            Finding::keyword("confidential", 45..57, "m"),
        ];
        assert_eq!(redact(text, &f), "mail [EMAIL_ADDRESS], card ****1111, [REDACTED] plan");
    }

    #[test]
    fn overlapping_spans_are_merged() {
        let text = "token abcdef123456 end";
        let f = vec![Finding::pii("GENERIC_SECRET", 6..18, 0.8, "m"), Finding::keyword("abcdef", 6..12, "m")];
        assert_eq!(redact(text, &f), "token [GENERIC_SECRET] end");
    }

    #[test]
    fn multibyte_text_is_safe() {
        let text = "José confidencial ünïcode";
        let f = vec![Finding::keyword("confidencial", 6..18, "m")];
        let out = redact(text, &f);
        assert!(out.starts_with("José [REDACTED]"));
    }

    #[test]
    fn whole_text_verdicts_cannot_be_redacted() {
        assert!(!can_redact(&[Finding::check(CheckId::SourceCode, 0.9, "m")]));
        assert!(can_redact(&[Finding::keyword("x", 0..1, "m")]));
        assert!(!can_redact(&[]));
    }
}
