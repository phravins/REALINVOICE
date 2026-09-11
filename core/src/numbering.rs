//! Invoice numbering: `RI-YYYY-NNNN`, sequential within an Indian financial year.
//!
//! The financial year runs 1 April to 31 March, and `YYYY` is the year it *starts* in:
//! 2026-09-11 and 2027-02-11 both sit in FY 2026 and share one counter, which resets to
//! 0001 on 2027-04-01.

use chrono::{Datelike, NaiveDate};

use crate::error::{CoreError, Result};

/// Financial year a date belongs to, named by its starting year.
pub fn financial_year(date: NaiveDate) -> i32 {
    if date.month() >= 4 {
        date.year()
    } else {
        date.year() - 1
    }
}

/// Formats a sequence number for a financial year.
pub fn format_invoice_no(fy: i32, sequence: u32) -> String {
    format!("RI-{fy}-{sequence:04}")
}

/// Pulls the sequence back out of an invoice number, for continuing a series.
/// Returns `None` for anything that isn't one of ours or isn't in `fy`.
pub fn parse_sequence(invoice_no: &str, fy: i32) -> Option<u32> {
    let rest = invoice_no.strip_prefix("RI-")?;
    let (year, seq) = rest.split_once('-')?;
    if year.parse::<i32>().ok()? != fy {
        return None;
    }
    seq.parse::<u32>().ok()
}

/// The next number in `fy`, given the highest already issued in it.
pub fn next_invoice_no(fy: i32, highest_existing: Option<&str>) -> Result<String> {
    let last = match highest_existing {
        Some(no) => parse_sequence(no, fy)
            .ok_or_else(|| CoreError::Invalid(format!("unparseable invoice number: {no}")))?,
        None => 0,
    };
    let next = last
        .checked_add(1)
        .ok_or_else(|| CoreError::Invalid(format!("invoice sequence exhausted for FY {fy}")))?;
    Ok(format_invoice_no(fy, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn financial_year_starts_in_april() {
        assert_eq!(financial_year(d(2026, 4, 1)), 2026);
        assert_eq!(financial_year(d(2026, 9, 11)), 2026);
        assert_eq!(financial_year(d(2027, 3, 31)), 2026);
        assert_eq!(financial_year(d(2027, 4, 1)), 2027);
        assert_eq!(financial_year(d(2026, 1, 15)), 2025);
    }

    #[test]
    fn numbers_are_zero_padded_to_four_digits() {
        assert_eq!(format_invoice_no(2026, 1), "RI-2026-0001");
        assert_eq!(format_invoice_no(2026, 42), "RI-2026-0042");
        assert_eq!(format_invoice_no(2026, 10_000), "RI-2026-10000");
    }

    #[test]
    fn the_series_continues_from_the_highest_issued() {
        assert_eq!(next_invoice_no(2026, None).unwrap(), "RI-2026-0001");
        assert_eq!(next_invoice_no(2026, Some("RI-2026-0001")).unwrap(), "RI-2026-0002");
        assert_eq!(next_invoice_no(2026, Some("RI-2026-0999")).unwrap(), "RI-2026-1000");
    }

    #[test]
    fn a_new_financial_year_restarts_at_one() {
        // Nothing issued in FY 2027 yet, even though FY 2026 got to 0500.
        assert_eq!(next_invoice_no(2027, None).unwrap(), "RI-2027-0001");
        assert_eq!(parse_sequence("RI-2026-0500", 2027), None);
    }

    #[test]
    fn foreign_numbers_are_rejected() {
        assert_eq!(parse_sequence("INV/2026/1", 2026), None);
        assert_eq!(parse_sequence("RI-2026-abcd", 2026), None);
        assert!(next_invoice_no(2026, Some("INV/2026/1")).is_err());
    }
}
