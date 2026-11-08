//! Layer 2: personal data and secrets by pattern + validator.
//!
//! * `redact-core` (Apache-2.0) supplies universal, US and UK recognizers and ~45 secret/token types.
//! * Veil adds EU and APAC national identifiers with real check-digit validation ([`validators`]).
//!
//! Identifiers that are only digits (e.g. a German tax ID or a Dutch BSN) score below the default
//! threshold unless a context word ("BSN", "Steuer-ID", "TFN", …) appears just before them, which keeps
//! order numbers and phone numbers from triggering prompts.
//!
//! NOTICE: some EU/APAC patterns were adapted from pii-vault (MIT License).

pub mod validators;

use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;
use veil_core::Finding;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Region {
    Universal,
    Us,
    Uk,
    Eu,
    Apac,
}

impl Region {
    pub fn parse(s: &str) -> Option<Region> {
        match s.trim().to_ascii_lowercase().as_str() {
            "universal" => Some(Region::Universal),
            "us" => Some(Region::Us),
            "uk" => Some(Region::Uk),
            "eu" => Some(Region::Eu),
            "apac" => Some(Region::Apac),
            _ => None,
        }
    }
}

/// Region of a redact-core entity, or `None` for types Veil ignores as too noisy for a chat context.
pub fn redact_entity_region(entity: &str) -> Option<Region> {
    match entity {
        "DATE_TIME" | "URL" | "DOMAIN_NAME" | "AGE" | "ISBN" | "US_ZIP_CODE" | "UK_POSTCODE" | "GUID" | "PO_BOX"
        | "PERSON" | "LOCATION" | "ORGANIZATION" | "MAC_ADDRESS" | "MD5_HASH" | "SHA1_HASH" | "SHA256_HASH" => None,
        e if e.starts_with("US_") || e == "MEDICAL_LICENSE" || e == "MEDICAL_RECORD_NUMBER" => Some(Region::Us),
        e if e.starts_with("UK_") => Some(Region::Uk),
        _ => Some(Region::Universal),
    }
}

struct Recognizer {
    entity: &'static str,
    region: Region,
    re: Regex,
    validate: fn(&str) -> bool,
    score: f32,
    context: &'static [&'static str],
    context_score: f32,
}

fn always(_: &str) -> bool {
    true
}

fn veil_recognizers() -> &'static [Recognizer] {
    static R: OnceLock<Vec<Recognizer>> = OnceLock::new();
    R.get_or_init(|| {
        use validators::*;
        use Region::*;
        let r = |entity, region, pattern: &str, validate, score, context, context_score| Recognizer {
            entity,
            region,
            re: Regex::new(pattern).expect("recognizer pattern"),
            validate,
            score,
            context,
            context_score,
        };
        vec![
            // ---- EU ----
            r("ES_DNI", Eu, r"\b\d{8}-?[A-Za-z]\b", es_dni as fn(&str) -> bool, 0.85, &["dni", "nif", "documento"][..], 0.95),
            r("ES_NIE", Eu, r"\b[XYZxyz]-?\d{7}-?[A-Za-z]\b", es_nie, 0.85, &["nie", "extranjero"], 0.95),
            r(
                "DE_TAX_ID",
                Eu,
                r"\b\d{2} ?\d{3} ?\d{3} ?\d{3}\b",
                de_tax_id,
                0.4,
                &["steuer", "idnr", "identifikationsnummer", "tax id", "tin"],
                0.85,
            ),
            r(
                "FR_NIR",
                Eu,
                r"\b[1-478] ?\d{2} ?\d{2} ?(?:\d{2}|2[ABab]) ?\d{3} ?\d{3} ?\d{2}\b",
                fr_nir,
                0.75,
                &["sécurité sociale", "securite sociale", "nir", "insee", "sécu", "social security"],
                0.95,
            ),
            r(
                "IT_FISCAL_CODE",
                Eu,
                r"\b[A-Za-z]{6}[0-9LMNPQRSTUVlmnpqrstuv]{2}[ABCDEHLMPRSTabcdehlmprst][0-9LMNPQRSTUVlmnpqrstuv]{2}[A-Za-z][0-9LMNPQRSTUVlmnpqrstuv]{3}[A-Za-z]\b",
                it_fiscal_code,
                0.9,
                &["codice fiscale", "fiscal code", "cf"],
                0.95,
            ),
            r(
                "NL_BSN",
                Eu,
                r"\b\d{4}\.?\d{2}\.?\d{3}\b",
                nl_bsn,
                0.4,
                &["bsn", "burgerservicenummer", "sofinummer", "citizen service"],
                0.85,
            ),
            r(
                "EU_VAT",
                Eu,
                r"\b(?:ATU\d{8}|BE[01]\d{9}|BG\d{9,10}|CY\d{8}[A-Z]|CZ\d{8,10}|DE\d{9}|DK\d{8}|EE\d{9}|EL\d{9}|ES[0-9A-Z]\d{7}[0-9A-Z]|FI\d{8}|FR[0-9A-HJ-NP-Z]{2}\d{9}|HR\d{11}|HU\d{8}|IE\d{7}[A-W][A-I]?|IT\d{11}|LT(?:\d{9}|\d{12})|LU\d{8}|LV\d{11}|MT\d{8}|NL\d{9}B\d{2}|PL\d{10}|PT\d{9}|RO\d{6,10}|SE\d{12}|SI\d{8}|SK\d{10})\b",
                always,
                0.6,
                &["vat", "ust", "umsatzsteuer", "tva", "iva", "btw", "moms", "mwst", "partita"],
                0.9,
            ),
            // ---- APAC ----
            r(
                "IN_AADHAAR",
                Apac,
                r"\b[2-9]\d{3} \d{4} \d{4}\b",
                in_aadhaar,
                0.65,
                &["aadhaar", "aadhar", "uidai", "आधार"],
                0.9,
            ),
            r("IN_AADHAAR", Apac, r"\b[2-9]\d{11}\b", in_aadhaar, 0.4, &["aadhaar", "aadhar", "uidai", "आधार"], 0.9),
            r(
                "IN_PAN",
                Apac,
                r"\b[A-Za-z]{3}[ABCFGHJLPTabcfghjlpt][A-Za-z]\d{4}[A-Za-z]\b",
                in_pan,
                0.7,
                &["pan", "permanent account", "income tax"],
                0.9,
            ),
            r(
                "PK_CNIC",
                Apac,
                r"\b[1-7]\d{4}-\d{7}-\d\b",
                pk_cnic,
                0.85,
                &["cnic", "nic", "nadra", "identity card", "shanakhti"],
                0.95,
            ),
            r("PK_CNIC", Apac, r"\b[1-7]\d{12}\b", pk_cnic, 0.4, &["cnic", "nic", "nadra", "identity card"], 0.85),
            r("AU_TFN", Apac, r"\b\d{3} ?\d{3} ?\d{2,3}\b", au_tfn, 0.4, &["tfn", "tax file"], 0.9),
            r("SG_NRIC", Apac, r"\b[STFGMstfgm]\d{7}[A-Za-z]\b", sg_nric, 0.85, &["nric", "fin", "ic no", "identity"], 0.95),
        ]
    })
}

fn has_context(text: &str, start: usize, words: &[&str]) -> bool {
    if words.is_empty() {
        return false;
    }
    let mut from = start.saturating_sub(48);
    while !text.is_char_boundary(from) {
        from -= 1;
    }
    let window = text[from..start].to_lowercase();
    words.iter().any(|w| window.contains(w))
}

pub struct PiiDetector {
    engine: redact_core::AnalyzerEngine,
}

impl Default for PiiDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl PiiDetector {
    pub fn new() -> Self {
        PiiDetector { engine: redact_core::AnalyzerEngine::new() }
    }

    /// Scan `text` for the given regions. Findings below `min_score` are dropped.
    pub fn scan(&self, text: &str, regions: &HashSet<Region>, min_score: f32, source: &str) -> Vec<Finding> {
        let mut out: Vec<Finding> = Vec::new();
        let mut push = |entity: &str, start: usize, end: usize, score: f32| {
            if score < min_score || start >= end {
                return;
            }
            let dup = out.iter().any(|f| {
                f.span.as_ref() == Some(&(start..end))
                    && matches!(&f.kind, veil_core::FindingKind::Pii(e) if e == entity)
            });
            if !dup {
                out.push(Finding::pii(entity, start..end, score, source));
            }
        };

        if regions.iter().any(|r| matches!(r, Region::Universal | Region::Us | Region::Uk)) {
            match self.engine.analyze(text, None) {
                Ok(result) => {
                    for d in result.detected_entities {
                        let entity = d.entity_type.as_str();
                        let Some(region) = redact_entity_region(entity) else { continue };
                        if !regions.contains(&region) || d.end > text.len() {
                            continue;
                        }
                        let entity = if entity == "IBAN_CODE" { "IBAN" } else { entity };
                        push(entity, d.start, d.end, d.score);
                    }
                }
                Err(e) => tracing::warn!("pattern analysis failed: {e}"),
            }
        }

        for r in veil_recognizers() {
            if !regions.contains(&r.region) {
                continue;
            }
            for m in r.re.find_iter(text) {
                if !(r.validate)(m.as_str()) {
                    continue;
                }
                let score = if has_context(text, m.start(), r.context) { r.context_score } else { r.score };
                push(r.entity, m.start(), m.end(), score);
            }
        }
        out.sort_by_key(|f| f.span.as_ref().map(|s| s.start).unwrap_or(0));
        out
    }
}

/// Parse region names, ignoring unknown ones.
pub fn parse_regions(names: &[String]) -> HashSet<Region> {
    names.iter().filter_map(|n| Region::parse(n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> HashSet<Region> {
        [Region::Universal, Region::Us, Region::Uk, Region::Eu, Region::Apac].into_iter().collect()
    }

    fn entities(text: &str, regions: &HashSet<Region>) -> Vec<String> {
        PiiDetector::new()
            .scan(text, regions, 0.5, "m")
            .into_iter()
            .map(|f| match f.kind {
                veil_core::FindingKind::Pii(e) => e,
                _ => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn universal_and_secret_types_are_found() {
        let text = "mail jane@example.com, card 4111 1111 1111 1111, key AKIAIOSFODNN7EXAMPLE";
        let e = entities(text, &all());
        for want in ["EMAIL_ADDRESS", "CREDIT_CARD", "AWS_ACCESS_KEY"] {
            assert!(e.contains(&want.to_string()), "{want} missing from {e:?}");
        }
    }

    #[test]
    fn noisy_types_are_dropped() {
        let e = entities("see https://example.com on 2024-01-02, aged 42", &all());
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn regions_filter_results() {
        let text = "SSN 078-05-1120 and DNI 12345678Z";
        let us: HashSet<Region> = [Region::Us].into_iter().collect();
        assert_eq!(entities(text, &us), vec!["US_SSN"]);
        let eu: HashSet<Region> = [Region::Eu].into_iter().collect();
        assert_eq!(entities(text, &eu), vec!["ES_DNI"]);
    }

    #[test]
    fn eu_identifiers() {
        let e = entities("NIE X1234567L, CF RSSMRA85T10A562S, VAT DE123456789", &all());
        for want in ["ES_NIE", "IT_FISCAL_CODE", "EU_VAT"] {
            assert!(e.contains(&want.to_string()), "{want} missing from {e:?}");
        }
        assert!(!entities("ref 12345678A", &all()).contains(&"ES_DNI".to_string()), "bad check letter");
    }

    #[test]
    fn digit_only_ids_need_context() {
        let eu: HashSet<Region> = [Region::Eu].into_iter().collect();
        assert!(entities("order 111222333 shipped", &eu).is_empty());
        assert_eq!(entities("mijn BSN is 111222333", &eu), vec!["NL_BSN"]);
        assert_eq!(entities("Steuer-ID: 86095742719", &eu), vec!["DE_TAX_ID"]);
        let apac: HashSet<Region> = [Region::Apac].into_iter().collect();
        assert!(entities("invoice 123456782", &apac).is_empty());
        assert_eq!(entities("my TFN is 123 456 782", &apac), vec!["AU_TFN"]);
    }

    #[test]
    fn apac_identifiers() {
        let apac: HashSet<Region> = [Region::Apac].into_iter().collect();
        let e = entities("CNIC 35202-1234567-1, PAN ABCPE1234F, NRIC S1234567D", &apac);
        assert_eq!(e, vec!["PK_CNIC", "IN_PAN", "SG_NRIC"]);
        assert!(entities("NRIC S1234567A", &apac).is_empty());
    }

    #[test]
    fn spans_point_at_the_value() {
        let text = "DNI: 12345678Z ok";
        let f = PiiDetector::new().scan(text, &all(), 0.5, "m");
        let dni = f.iter().find(|f| f.kind == veil_core::FindingKind::Pii("ES_DNI".into())).unwrap();
        assert_eq!(&text[dni.span.clone().unwrap()], "12345678Z");
    }

    #[test]
    fn ordinary_chat_has_no_findings() {
        let samples = [
            "Can you summarise this meeting for me? We agreed to ship on Friday.",
            "Write a haiku about autumn leaves falling in the park",
            "The build 2024.10.3 failed with exit code 137 after 45 minutes",
            "Translate 'good morning' into Spanish and French please",
            "fn main() { let x = vec![1, 2, 3]; println!(\"{:?}\", x); }",
        ];
        for s in samples {
            let e = entities(s, &all());
            assert!(e.is_empty(), "false positive in {s:?}: {e:?}");
        }
    }
}
