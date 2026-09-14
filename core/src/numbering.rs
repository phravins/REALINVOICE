//! Document numbering: `RI-YYYY-NNNN` for invoices, `CN-YYYY-NNNN` for credit notes,
//! each sequential within an Indian financial year.
//!
//! The financial year runs 1 April to 31 March, and `YYYY` is the year it *starts* in:
//! 2026-09-11 and 2027-02-11 both sit in FY 2026 and share one counter, which resets to
//! 0001 on 2027-04-01.
//!
//! The two series are **separate counters**, as GST expects: an invoice and a credit note
//! raised the same afternoon are `RI-2026-0007` and `CN-2026-0001`. They share the
//! formatting and the financial-year rule so a change to either lands in one place.

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

/// The prefix on an invoice number.
pub const INVOICE_PREFIX: &str = "RI";

/// The prefix on a credit note number. A different series, not a variant of the same one.
pub const CREDIT_NOTE_PREFIX: &str = "CN";

/// Formats a sequence number for a financial year.
pub fn format_invoice_no(fy: i32, sequence: u32) -> String {
    format_document_no(INVOICE_PREFIX, fy, sequence)
}

/// Formats a credit note number.
pub fn format_credit_note_no(fy: i32, sequence: u32) -> String {
    format_document_no(CREDIT_NOTE_PREFIX, fy, sequence)
}

/// `PREFIX-YYYY-NNNN`.
pub fn format_document_no(prefix: &str, fy: i32, sequence: u32) -> String {
    format!("{prefix}-{fy}-{sequence:04}")
}

/// Pulls the sequence back out of an invoice number, for continuing a series.
/// Returns `None` for anything that isn't one of ours or isn't in `fy`.
pub fn parse_sequence(invoice_no: &str, fy: i32) -> Option<u32> {
    parse_document_sequence(INVOICE_PREFIX, invoice_no, fy)
}

/// As [`parse_sequence`], for any series. The prefix must match: a credit note number
/// handed to the invoice series is not a number with the wrong year, it is the wrong
/// document, and treating it as one would merge two counters that must stay apart.
pub fn parse_document_sequence(prefix: &str, number: &str, fy: i32) -> Option<u32> {
    let rest = number.strip_prefix(prefix)?.strip_prefix('-')?;
    let (year, seq) = rest.split_once('-')?;
    if year.parse::<i32>().ok()? != fy {
        return None;
    }
    seq.parse::<u32>().ok()
}

/// The next number in `fy`, given the highest already issued in it.
pub fn next_invoice_no(fy: i32, highest_existing: Option<&str>) -> Result<String> {
    next_document_no(INVOICE_PREFIX, fy, highest_existing)
}

/// The next credit note number in `fy`.
pub fn next_credit_note_no(fy: i32, highest_existing: Option<&str>) -> Result<String> {
    next_document_no(CREDIT_NOTE_PREFIX, fy, highest_existing)
}

/// The next number in a series.
pub fn next_document_no(prefix: &str, fy: i32, highest_existing: Option<&str>) -> Result<String> {
    let last = match highest_existing {
        Some(no) => parse_document_sequence(prefix, no, fy)
            .ok_or_else(|| CoreError::Invalid(format!("unparseable document number: {no}")))?,
        None => 0,
    };
    let next = last
        .checked_add(1)
        .ok_or_else(|| CoreError::Invalid(format!("{prefix} sequence exhausted for FY {fy}")))?;
    Ok(format_document_no(prefix, fy, next))
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

    #[test]
    fn credit_notes_run_their_own_series() {
        // Separate counters, as GST expects: the first credit note of a year is 0001
        // however many invoices came before it.
        assert_eq!(next_credit_note_no(2026, None).unwrap(), "CN-2026-0001");
        assert_eq!(next_credit_note_no(2026, Some("CN-2026-0007")).unwrap(), "CN-2026-0008");
        assert_eq!(format_credit_note_no(2026, 42), "CN-2026-0042");

        // And the series do not read each other's numbers. An invoice number offered to
        // the credit note series is the wrong document, not a number to continue from.
        assert!(parse_document_sequence("CN", "RI-2026-0007", 2026).is_none());
        assert!(parse_document_sequence("RI", "CN-2026-0007", 2026).is_none());
        assert!(next_credit_note_no(2026, Some("RI-2026-0007")).is_err());
        assert!(next_invoice_no(2026, Some("CN-2026-0007")).is_err());

        // The year still has to match, in either series.
        assert!(parse_document_sequence("CN", "CN-2025-0007", 2026).is_none());
    }
}
