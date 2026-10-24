//! PII masking for reviewers (Phase 5).
//!
//! Detects and masks personally identifiable information in free-text
//! fields so that supervisors can review task content without exposing
//! sensitive data.

use regex::Regex;
use std::sync::OnceLock;

/// Masks PII in a string value.
///
/// Replaces:
/// - Email addresses: `user@example.com` → `***@example.com`
/// - Phone numbers: `+1 555-123-4567` → `+1 ***-***-****`
/// - Credit card numbers: `1234 5678 9012 3456` → `**** **** **** 3456`
///
/// Non-matching text is passed through unchanged.
#[must_use]
pub fn mask_pii(value: &str) -> String {
    let result = mask_emails(value);
    let result = mask_phones(&result);
    mask_credit_cards(&result)
}

/// Masks email addresses, keeping the domain visible.
fn mask_emails(input: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}").unwrap()
    });
    re.replace_all(input, |caps: &regex::Captures| {
        let email = caps.get(0).unwrap().as_str();
        if let Some(at_pos) = email.find('@') {
            let domain = &email[at_pos..];
            format!("***{}", domain)
        } else {
            "***".to_owned()
        }
    })
    .to_string()
}

/// Masks phone numbers, keeping country code and last 4 digits.
fn mask_phones(input: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\+?\d[\d\s\-\.]{7,}\d").unwrap()
    });
    re.replace_all(input, |caps: &regex::Captures| {
        let phone = caps.get(0).unwrap().as_str();
        let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 4 {
            let last4 = &digits[digits.len() - 4..];
            let masked_len = digits.len().saturating_sub(4);
            let masked: String = std::iter::repeat('*').take(masked_len).collect();
            // Preserve the leading + if present
            let prefix = if phone.starts_with('+') { "+" } else { "" };
            format!("{}{}{}", prefix, masked, last4)
        } else {
            "***".to_owned()
        }
    })
    .to_string()
}

/// Masks credit card numbers, keeping last 4 digits.
fn mask_credit_cards(input: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\b\d{4}[\s\-]?\d{4}[\s\-]?\d{4}[\s\-]?\d{4}\b").unwrap()
    });
    re.replace_all(input, |caps: &regex::Captures| {
        let card = caps.get(0).unwrap().as_str();
        let digits: String = card.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 4 {
            let last4 = &digits[digits.len() - 4..];
            format!("**** **** **** {}", last4)
        } else {
            "****".to_owned()
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_email() {
        let result = mask_pii("Contact me at john.doe@example.com");
        assert!(!result.contains("john.doe"));
        assert!(result.contains("***@example.com"));
    }

    #[test]
    fn masks_phone() {
        let result = mask_pii("Call +1 555-123-4567");
        assert!(!result.contains("555-123-4567"));
        assert!(result.contains("***"));
    }

    #[test]
    fn masks_credit_card() {
        let result = mask_pii("Card 1234 5678 9012 3456");
        assert!(!result.contains("1234 5678 9012"));
        assert!(result.contains("3456"));
    }

    #[test]
    fn leaves_clean_text_unchanged() {
        let input = "Hello world, this is a test.";
        assert_eq!(mask_pii(input), input);
    }
}
