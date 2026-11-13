//! Money is stored as whole cents in an `i64` so sums never drift the way floats do.

use crate::error::{invalid, Result};

/// Parse an amount the way banks write them in CSV files.
///
/// Accepts `12.34`, `-12.34`, `$1,234.56`, `-$5`, `($5.00)` (accounting negative),
/// `12.34-` (trailing minus) and `+3`. Returns cents.
pub fn parse_amount(raw: &str) -> Result<i64> {
    let mut s: String = raw
        .trim()
        .chars()
        .filter(|c| !matches!(c, '$' | ',' | ' ' | '\u{a0}'))
        .collect();
    if s.is_empty() {
        return invalid(format!("empty amount"));
    }
    let mut negative = false;
    if s.starts_with('(') && s.ends_with(')') {
        negative = true;
        s = s[1..s.len() - 1].to_string();
    }
    if let Some(rest) = s.strip_prefix('-') {
        negative = !negative;
        s = rest.to_string();
    } else if let Some(rest) = s.strip_prefix('+') {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_suffix('-') {
        negative = !negative;
        s = rest.to_string();
    }
    // A minus may also sit after a stripped "$" e.g. "$-5.00".
    if let Some(rest) = s.strip_prefix('-') {
        negative = !negative;
        s = rest.to_string();
    }

    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s.as_str(), ""),
    };
    if whole.is_empty() && frac.is_empty() {
        return invalid(format!("not an amount: {raw:?}"));
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return invalid(format!("not an amount: {raw:?}"));
    }
    if frac.len() > 2 && frac[2..].chars().any(|c| c != '0') {
        return invalid(format!("more than two decimal places: {raw:?}"));
    }
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| crate::Error::Invalid(format!("amount too large: {raw:?}")))?
    };
    let frac_cents: i64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>().unwrap() * 10,
        _ => frac[..2].parse().unwrap(),
    };
    let cents = whole
        .checked_mul(100)
        .and_then(|c| c.checked_add(frac_cents))
        .ok_or_else(|| crate::Error::Invalid(format!("amount too large: {raw:?}")))?;
    Ok(if negative { -cents } else { cents })
}

/// Format cents as a plain decimal string, e.g. `-1234` → `"-12.34"`.
pub fn format_cents(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_bank_formats() {
        assert_eq!(parse_amount("12.34").unwrap(), 1234);
        assert_eq!(parse_amount("-12.34").unwrap(), -1234);
        assert_eq!(parse_amount("$1,234.56").unwrap(), 123456);
        assert_eq!(parse_amount("-$5").unwrap(), -500);
        assert_eq!(parse_amount("$-5.00").unwrap(), -500);
        assert_eq!(parse_amount("(5.00)").unwrap(), -500);
        assert_eq!(parse_amount("12.5").unwrap(), 1250);
        assert_eq!(parse_amount("7.50-").unwrap(), -750);
        assert_eq!(parse_amount("+3").unwrap(), 300);
        assert_eq!(parse_amount(".99").unwrap(), 99);
        assert_eq!(parse_amount("1.2300").unwrap(), 123);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_amount("").is_err());
        assert!(parse_amount("abc").is_err());
        assert!(parse_amount("1.234").is_err());
        assert!(parse_amount("-").is_err());
    }

    #[test]
    fn formats() {
        assert_eq!(format_cents(-1234), "-12.34");
        assert_eq!(format_cents(5), "0.05");
        assert_eq!(format_cents(0), "0.00");
    }
}
