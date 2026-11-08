//! Check-digit validators for national identifiers. Each takes the matched text (separators allowed) and
//! returns whether it is a valid identifier.

fn digits(s: &str) -> Vec<u32> {
    s.chars().filter_map(|c| c.to_digit(10)).collect()
}

fn alnum_upper(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

const DNI_LETTERS: &[u8; 23] = b"TRWAGMYFPDXBNJZSQVHLCKE";

/// Spanish DNI: 8 digits + letter (mod 23).
pub fn es_dni(s: &str) -> bool {
    let v = alnum_upper(s);
    if v.len() != 9 {
        return false;
    }
    let (num, letter) = v.split_at(8);
    let Ok(n) = num.parse::<u32>() else { return false };
    letter.as_bytes()[0] == DNI_LETTERS[(n % 23) as usize]
}

/// Spanish NIE: X/Y/Z + 7 digits + letter (X=0, Y=1, Z=2, then mod 23).
pub fn es_nie(s: &str) -> bool {
    let v = alnum_upper(s);
    if v.len() != 9 {
        return false;
    }
    let prefix = match v.as_bytes()[0] {
        b'X' => '0',
        b'Y' => '1',
        b'Z' => '2',
        _ => return false,
    };
    es_dni(&format!("{prefix}{}", &v[1..]))
}

/// German tax identification number (Steuerliche Identifikationsnummer): 11 digits, no leading zero,
/// ISO 7064 MOD 11,10 check digit, and in the first ten digits exactly one digit repeats (2 or 3 times).
pub fn de_tax_id(s: &str) -> bool {
    let d = digits(s);
    if d.len() != 11 || d[0] == 0 {
        return false;
    }
    let mut counts = [0u8; 10];
    for &x in &d[..10] {
        counts[x as usize] += 1;
    }
    let repeated: Vec<u8> = counts.iter().copied().filter(|&c| c > 1).collect();
    if repeated.len() != 1 || !(2..=3).contains(&repeated[0]) {
        return false;
    }
    let mut product = 10;
    for &x in &d[..10] {
        let mut sum = (x + product) % 10;
        if sum == 0 {
            sum = 10;
        }
        product = (sum * 2) % 11;
    }
    let check = (11 - product) % 10;
    check == d[10]
}

/// French social security number (NIR): 13 characters + 2-digit key = 97 − (number mod 97).
/// Corsica departments 2A/2B are replaced by 19/18 for the computation.
pub fn fr_nir(s: &str) -> bool {
    let v = alnum_upper(s);
    if v.len() != 15 || !matches!(v.as_bytes()[0], b'1' | b'2' | b'3' | b'4' | b'7' | b'8') {
        return false;
    }
    let (body, key) = v.split_at(13);
    let body = body.replacen("2A", "19", 1).replacen("2B", "18", 1);
    if !body.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let (Ok(n), Ok(k)) = (body.parse::<u64>(), key.parse::<u64>()) else { return false };
    // Month: 01–12, or 20–99 for unknown/special registrations.
    let month: u32 = v[3..5].parse().unwrap_or(0);
    if !((1..=12).contains(&month) || (20..=99).contains(&month)) {
        return false;
    }
    k == 97 - (n % 97)
}

/// Italian fiscal code (codice fiscale): 16 characters with a check letter. Omocodia letters accepted.
pub fn it_fiscal_code(s: &str) -> bool {
    let v = alnum_upper(s);
    if v.len() != 16 {
        return false;
    }
    const ODD: [u32; 36] = [
        1, 0, 5, 7, 9, 13, 15, 17, 19, 21, // 0-9
        1, 0, 5, 7, 9, 13, 15, 17, 19, 21, 2, 4, 18, 20, 11, 3, 6, 8, 12, 14, 16, 10, 22, 25, 24, 23, // A-Z
    ];
    let value = |c: u8| -> Option<usize> {
        match c {
            b'0'..=b'9' => Some((c - b'0') as usize),
            b'A'..=b'Z' => Some((c - b'A') as usize + 10),
            _ => None,
        }
    };
    let bytes = v.as_bytes();
    let mut sum = 0;
    for (i, &c) in bytes[..15].iter().enumerate() {
        let Some(idx) = value(c) else { return false };
        sum += if i % 2 == 0 {
            ODD[idx]
        } else if idx < 10 {
            idx as u32
        } else {
            (idx - 10) as u32
        };
    }
    bytes[15] == b'A' + (sum % 26) as u8
}

/// Dutch citizen service number (BSN): 9 digits, "11-proof" with the last weight −1.
pub fn nl_bsn(s: &str) -> bool {
    let d = digits(s);
    if d.len() != 9 || d.iter().all(|&x| x == 0) {
        return false;
    }
    let mut sum: i64 = 0;
    for (i, &x) in d.iter().enumerate() {
        let w = if i == 8 { -1 } else { 9 - i as i64 };
        sum += w * x as i64;
    }
    sum % 11 == 0
}

const VERHOEFF_D: [[u8; 10]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    [1, 2, 3, 4, 0, 6, 7, 8, 9, 5],
    [2, 3, 4, 0, 1, 7, 8, 9, 5, 6],
    [3, 4, 0, 1, 2, 8, 9, 5, 6, 7],
    [4, 0, 1, 2, 3, 9, 5, 6, 7, 8],
    [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
    [6, 5, 9, 8, 7, 1, 0, 4, 3, 2],
    [7, 6, 5, 9, 8, 2, 1, 0, 4, 3],
    [8, 7, 6, 5, 9, 3, 2, 1, 0, 4],
    [9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
];
const VERHOEFF_P: [[u8; 10]; 8] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    [1, 5, 7, 6, 2, 8, 3, 0, 9, 4],
    [5, 8, 0, 3, 7, 9, 6, 1, 4, 2],
    [8, 9, 1, 6, 0, 4, 3, 5, 2, 7],
    [9, 4, 5, 3, 1, 2, 8, 7, 6, 0],
    [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
    [2, 7, 9, 3, 8, 0, 6, 4, 1, 5],
    [7, 0, 4, 6, 9, 1, 3, 2, 5, 8],
];
const VERHOEFF_INV: [u8; 10] = [0, 4, 3, 2, 1, 5, 6, 7, 8, 9];

/// Verhoeff checksum over a digit string (check digit included).
pub fn verhoeff(d: &[u32]) -> bool {
    let mut c = 0u8;
    for (i, &x) in d.iter().rev().enumerate() {
        c = VERHOEFF_D[c as usize][VERHOEFF_P[i % 8][x as usize] as usize];
    }
    c == 0
}

/// The Verhoeff check digit to append to `d`.
pub fn verhoeff_digit(d: &[u32]) -> u32 {
    let mut c = 0u8;
    for (i, &x) in d.iter().rev().enumerate() {
        c = VERHOEFF_D[c as usize][VERHOEFF_P[(i + 1) % 8][x as usize] as usize];
    }
    VERHOEFF_INV[c as usize] as u32
}

/// Indian Aadhaar: 12 digits, first digit 2–9, Verhoeff check digit.
pub fn in_aadhaar(s: &str) -> bool {
    let d = digits(s);
    d.len() == 12 && d[0] >= 2 && verhoeff(&d)
}

/// Australian tax file number: 8 or 9 digits, weighted sum divisible by 11.
pub fn au_tfn(s: &str) -> bool {
    let d = digits(s);
    let weights: &[u32] = match d.len() {
        9 => &[1, 4, 3, 7, 5, 8, 6, 9, 10],
        8 => &[10, 7, 8, 4, 6, 3, 5, 1],
        _ => return false,
    };
    let sum: u32 = d.iter().zip(weights).map(|(a, b)| a * b).sum();
    sum != 0 && sum % 11 == 0
}

/// Singapore NRIC/FIN: prefix S/T/F/G (+ M, format only) + 7 digits + check letter.
pub fn sg_nric(s: &str) -> bool {
    let v = alnum_upper(s);
    if v.len() != 9 {
        return false;
    }
    let b = v.as_bytes();
    let d = digits(&v[1..8]);
    if d.len() != 7 || !b[8].is_ascii_uppercase() {
        return false;
    }
    const W: [u32; 7] = [2, 7, 6, 5, 4, 3, 2];
    let mut sum: u32 = d.iter().zip(W).map(|(a, w)| a * w).sum();
    let table: &[u8; 11] = match b[0] {
        b'S' => b"JZIHGFEDCBA",
        b'T' => {
            sum += 4;
            b"JZIHGFEDCBA"
        }
        b'F' => b"XWUTRQPNMLK",
        b'G' => {
            sum += 4;
            b"XWUTRQPNMLK"
        }
        // M-series FINs (2022+): format accepted, checksum not verified.
        b'M' => return true,
        _ => return false,
    };
    b[8] == table[(sum % 11) as usize]
}

/// Pakistani CNIC: 13 digits, first digit 1–7 (province code).
pub fn pk_cnic(s: &str) -> bool {
    let d = digits(s);
    d.len() == 13 && (1..=7).contains(&d[0])
}

/// Indian PAN: AAAAA9999A with a valid holder-type letter in position 4.
pub fn in_pan(s: &str) -> bool {
    let v = alnum_upper(s);
    let b = v.as_bytes();
    v.len() == 10
        && b[..5].iter().all(|c| c.is_ascii_uppercase())
        && b"ABCFGHJLPT".contains(&b[3])
        && b[5..9].iter().all(|c| c.is_ascii_digit())
        && b[9].is_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_ids() {
        assert!(es_dni("12345678Z"));
        assert!(es_dni("12345678-z"));
        assert!(!es_dni("12345678A"));
        assert!(es_nie("X1234567L"));
        assert!(es_nie("Y-1234567-X") == es_dni("11234567X"));
        assert!(!es_nie("X1234567A"));
        assert!(!es_nie("A1234567L"));
    }

    #[test]
    fn german_tax_id() {
        assert!(de_tax_id("86095742719"));
        assert!(de_tax_id("86 095 742 719"));
        assert!(!de_tax_id("86095742718"), "wrong check digit");
        assert!(!de_tax_id("06095742719"), "leading zero");
        assert!(!de_tax_id("12345678903"), "no repeated digit");
    }

    fn nir_with_key(body: &str) -> String {
        let n: u64 = body.replacen("2A", "19", 1).replacen("2B", "18", 1).parse().unwrap();
        format!("{body}{:02}", 97 - n % 97)
    }

    #[test]
    fn french_nir() {
        let valid = nir_with_key("1840275123456");
        assert!(fr_nir(&valid), "{valid}");
        let corsica = nir_with_key("284022A123456");
        assert!(fr_nir(&corsica), "{corsica}");
        let mut wrong = valid.clone();
        wrong.replace_range(13..15, if &valid[13..15] == "01" { "02" } else { "01" });
        assert!(!fr_nir(&wrong));
        assert!(!fr_nir("5840275123456 12"), "bad sex digit");
    }

    #[test]
    fn italian_fiscal_code() {
        assert!(it_fiscal_code("RSSMRA85T10A562S"));
        assert!(it_fiscal_code("rssmra85t10a562s"));
        assert!(!it_fiscal_code("RSSMRA85T10A562T"));
        assert!(!it_fiscal_code("RSSMRA85T10A562"));
    }

    #[test]
    fn dutch_bsn() {
        assert!(nl_bsn("111222333"));
        assert!(nl_bsn("1112.22.333"));
        assert!(!nl_bsn("111222334"));
        assert!(!nl_bsn("000000000"));
    }

    #[test]
    fn verhoeff_and_aadhaar() {
        assert!(verhoeff(&[2, 3, 6, 3]));
        assert!(!verhoeff(&[2, 3, 6, 4]));
        assert_eq!(verhoeff_digit(&[2, 3, 6]), 3);
        let body = [2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4];
        let check = verhoeff_digit(&body);
        let s: String = body.iter().chain(std::iter::once(&check)).map(|d| char::from_digit(*d, 10).unwrap()).collect();
        assert!(in_aadhaar(&s), "{s}");
        let spaced = format!("{} {} {}", &s[..4], &s[4..8], &s[8..]);
        assert!(in_aadhaar(&spaced));
        let bad_check = (check + 1) % 10;
        let bad: String =
            body.iter().chain(std::iter::once(&bad_check)).map(|d| char::from_digit(*d, 10).unwrap()).collect();
        assert!(!in_aadhaar(&bad));
        assert!(!in_aadhaar("123412341234"), "cannot start with 1");
    }

    #[test]
    fn australian_tfn() {
        assert!(au_tfn("123 456 782"));
        assert!(!au_tfn("123456789"));
        assert!(!au_tfn("1234567"));
    }

    #[test]
    fn singapore_nric() {
        assert!(sg_nric("S1234567D"));
        assert!(!sg_nric("S1234567A"));
        // T series adds 4: 106 + 4 = 110 → rem 0 → 'J'.
        assert!(sg_nric("T1234567J"));
        // F series: rem 7 → 'N'; G adds 4 → rem 0 → 'X'.
        assert!(sg_nric("F1234567N"));
        assert!(sg_nric("G1234567X"));
        assert!(!sg_nric("A1234567D"));
    }

    #[test]
    fn pakistan_cnic_and_india_pan() {
        assert!(pk_cnic("35202-1234567-1"));
        assert!(!pk_cnic("95202-1234567-1"));
        assert!(!pk_cnic("3520-1234567-1"));
        assert!(in_pan("ABCPE1234F"));
        assert!(!in_pan("ABCXE1234F"), "invalid holder type");
        assert!(!in_pan("ABCPE12345"));
    }
}
