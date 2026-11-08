use serde::{Deserialize, Serialize};
use std::ops::Range;

/// The four built-in AI checks (layer 3). Each is a yes/no question asked of the content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckId {
    Secrets,
    PersonalInfo,
    Confidential,
    SourceCode,
}

impl CheckId {
    pub const ALL: [CheckId; 4] =
        [CheckId::Secrets, CheckId::PersonalInfo, CheckId::Confidential, CheckId::SourceCode];

    pub fn key(&self) -> &'static str {
        match self {
            CheckId::Secrets => "secrets",
            CheckId::PersonalInfo => "personal_info",
            CheckId::Confidential => "confidential",
            CheckId::SourceCode => "source_code",
        }
    }

    /// Short label shown to the user.
    pub fn label(&self) -> &'static str {
        match self {
            CheckId::Secrets => "passwords, API keys or secrets",
            CheckId::PersonalInfo => "personal information",
            CheckId::Confidential => "confidential business information",
            CheckId::SourceCode => "proprietary source code",
        }
    }

    pub fn from_key(key: &str) -> Option<CheckId> {
        CheckId::ALL.into_iter().find(|c| c.key() == key)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "layer", content = "what", rename_all = "snake_case")]
pub enum FindingKind {
    /// Layer 1: a user keyword or phrase.
    Keyword(String),
    /// Layer 2: a personal-data pattern, e.g. `EMAIL_ADDRESS`, `CREDIT_CARD`, `ES_DNI`.
    Pii(String),
    /// Layer 3: an AI check crossed its threshold.
    Check(CheckId),
}

/// One detection. Spans index into the scanned text (byte offsets) when known.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    #[serde(skip)]
    pub span: Option<Range<usize>>,
    pub score: f32,
    /// Where it was found: "message", a file name, or "image".
    pub source: String,
}

impl Finding {
    pub fn keyword(word: &str, span: Range<usize>, source: &str) -> Self {
        Finding { kind: FindingKind::Keyword(word.to_string()), span: Some(span), score: 1.0, source: source.into() }
    }

    pub fn pii(entity: &str, span: Range<usize>, score: f32, source: &str) -> Self {
        Finding { kind: FindingKind::Pii(entity.to_string()), span: Some(span), score, source: source.into() }
    }

    pub fn check(check: CheckId, score: f32, source: &str) -> Self {
        Finding { kind: FindingKind::Check(check), span: None, score, source: source.into() }
    }

    /// Detection type for the metadata-only activity log (never the matched value).
    pub fn type_label(&self) -> String {
        match &self.kind {
            FindingKind::Keyword(_) => "keyword".into(),
            FindingKind::Pii(entity) => entity.to_ascii_lowercase(),
            FindingKind::Check(c) => format!("ai:{}", c.key()),
        }
    }

    /// Phrase used in popups, e.g. `the keyword “confidential”`, `an email address`.
    pub fn describe(&self) -> String {
        match &self.kind {
            FindingKind::Keyword(word) => format!("the keyword “{word}”"),
            FindingKind::Pii(entity) => describe_pii(entity),
            FindingKind::Check(c) => c.label().to_string(),
        }
    }
}

fn describe_pii(entity: &str) -> String {
    let e = entity.to_ascii_uppercase();
    let text = match e.as_str() {
        "EMAIL_ADDRESS" => "an email address",
        "PHONE_NUMBER" | "UK_PHONE_NUMBER" | "UK_MOBILE_NUMBER" => "a phone number",
        "CREDIT_CARD" => "a payment card number",
        "IBAN" | "IBAN_CODE" => "a bank account number (IBAN)",
        "US_SSN" => "a US Social Security number",
        "UK_NINO" => "a UK National Insurance number",
        "UK_NHS" => "an NHS number",
        "IP_ADDRESS" => "an IP address",
        "PRIVATE_KEY" => "a private key",
        "JWT_TOKEN" => "an access token",
        "GENERIC_SECRET" => "a secret or password",
        e if e.ends_with("_API_KEY") || e.ends_with("_ACCESS_KEY") || e.ends_with("_TOKEN") => "an API key or token",
        "ES_DNI" | "ES_NIE" => "a Spanish ID number (DNI/NIE)",
        "DE_TAX_ID" => "a German tax ID",
        "FR_NIR" => "a French social security number",
        "IT_FISCAL_CODE" => "an Italian fiscal code",
        "NL_BSN" => "a Dutch citizen service number (BSN)",
        "EU_VAT" => "an EU VAT number",
        "IN_AADHAAR" => "an Aadhaar number",
        "IN_PAN" => "an Indian PAN",
        "PK_CNIC" => "a Pakistani CNIC",
        "AU_TFN" => "an Australian tax file number",
        "SG_NRIC" => "a Singapore NRIC/FIN",
        _ => return format!("personal data ({})", entity.to_ascii_lowercase().replace('_', " ")),
    };
    text.to_string()
}

/// A readable summary of findings for a popup, e.g. “an email address and the keyword “confidential””.
pub fn summarize(findings: &[Finding]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for f in findings {
        let d = f.describe();
        if !parts.contains(&d) {
            parts.push(d);
        }
    }
    match parts.len() {
        0 => "sensitive content".into(),
        1 => parts.remove(0),
        2 => format!("{} and {}", parts[0], parts[1]),
        n => format!("{}, and {} more", parts[..2].join(", "), n - 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_are_deduplicated_and_readable() {
        let f = vec![
            Finding::pii("EMAIL_ADDRESS", 0..5, 0.9, "message"),
            Finding::pii("EMAIL_ADDRESS", 9..14, 0.9, "message"),
            Finding::keyword("confidential", 20..32, "message"),
        ];
        assert_eq!(summarize(&f), "an email address and the keyword “confidential”");
    }

    #[test]
    fn log_labels_never_contain_values() {
        let f = Finding::keyword("project falcon", 0..14, "message");
        assert_eq!(f.type_label(), "keyword");
        assert_eq!(Finding::check(CheckId::SourceCode, 0.9, "x").type_label(), "ai:source_code");
    }

    #[test]
    fn check_keys_round_trip() {
        for c in CheckId::ALL {
            assert_eq!(CheckId::from_key(c.key()), Some(c));
        }
    }
}
