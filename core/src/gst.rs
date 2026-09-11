//! GST split and invoice totals.
//!
//! One rule drives everything: compare the buyer's place of supply against the seller's
//! home state. Same state is an intra-state sale, and the item's GST rate is split evenly
//! into CGST and SGST. Different states is an inter-state sale, and the whole rate is
//! charged as IGST.

use serde::{Deserialize, Serialize};

/// Seller's home state until a settings table exists (stage 2). Tamil Nadu.
pub const DEFAULT_HOME_STATE: &str = "TN";

/// Rounds money to paise, half away from zero, the way a printed invoice rounds.
pub fn round_money(amount: f64) -> f64 {
    (amount * 100.0).round() / 100.0
}

/// True when the sale is intra-state and therefore CGST + SGST rather than IGST.
/// Comparison is case- and whitespace-insensitive so "tn", "TN " and "TN" agree.
pub fn is_intra_state(home_state: &str, place_of_supply: &str) -> bool {
    home_state.trim().eq_ignore_ascii_case(place_of_supply.trim())
}

/// The three GST buckets. Exactly one of (cgst, sgst) or igst is ever non-zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TaxSplit {
    pub cgst: f64,
    pub sgst: f64,
    pub igst: f64,
}

impl TaxSplit {
    pub fn total(&self) -> f64 {
        round_money(self.cgst + self.sgst + self.igst)
    }
}

/// A line reduced to just what the tax maths needs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TaxableLine {
    pub qty: f64,
    pub rate: f64,
    /// Total GST percentage for the item, e.g. 18.0.
    pub tax_rate: f64,
}

impl TaxableLine {
    /// Taxable value of the line, before GST.
    pub fn line_total(&self) -> f64 {
        round_money(self.qty * self.rate)
    }
}

/// Splits the tax on one taxable value.
pub fn split_line_tax(taxable_value: f64, tax_rate: f64, intra_state: bool) -> TaxSplit {
    let tax = taxable_value * tax_rate / 100.0;
    if intra_state {
        let half = round_money(tax / 2.0);
        TaxSplit { cgst: half, sgst: half, igst: 0.0 }
    } else {
        TaxSplit { cgst: 0.0, sgst: 0.0, igst: round_money(tax) }
    }
}

/// Everything that goes on the totals block of an invoice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct InvoiceTotals {
    pub subtotal: f64,
    pub cgst: f64,
    pub sgst: f64,
    pub igst: f64,
    pub grand_total: f64,
}

/// Totals an invoice. Tax is split per line and then summed, so lines at different GST
/// rates on one invoice each get their own correct split.
pub fn compute_totals(
    lines: &[TaxableLine],
    home_state: &str,
    place_of_supply: &str,
) -> InvoiceTotals {
    let intra_state = is_intra_state(home_state, place_of_supply);

    let mut totals = InvoiceTotals::default();
    for line in lines {
        let taxable = line.line_total();
        let split = split_line_tax(taxable, line.tax_rate, intra_state);
        totals.subtotal += taxable;
        totals.cgst += split.cgst;
        totals.sgst += split.sgst;
        totals.igst += split.igst;
    }

    totals.subtotal = round_money(totals.subtotal);
    totals.cgst = round_money(totals.cgst);
    totals.sgst = round_money(totals.sgst);
    totals.igst = round_money(totals.igst);
    totals.grand_total = round_money(totals.subtotal + totals.cgst + totals.sgst + totals.igst);
    totals
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(qty: f64, rate: f64, tax_rate: f64) -> TaxableLine {
        TaxableLine { qty, rate, tax_rate }
    }

    /// The worked example from the spec: a Tamil Nadu seller billing a Tamil Nadu buyer
    /// 1,05,000.00 at 18% pays 9,450 CGST + 9,450 SGST for a 1,23,900.00 grand total.
    #[test]
    fn intra_state_tamil_nadu_splits_into_cgst_and_sgst() {
        let totals = compute_totals(&[line(1.0, 105_000.0, 18.0)], "TN", "TN");

        assert_eq!(totals.subtotal, 105_000.00);
        assert_eq!(totals.cgst, 9_450.00);
        assert_eq!(totals.sgst, 9_450.00);
        assert_eq!(totals.igst, 0.0);
        assert_eq!(totals.grand_total, 123_900.00);
    }

    #[test]
    fn inter_state_charges_igst_at_the_full_rate() {
        let totals = compute_totals(&[line(1.0, 105_000.0, 18.0)], "TN", "KA");

        assert_eq!(totals.subtotal, 105_000.00);
        assert_eq!(totals.cgst, 0.0);
        assert_eq!(totals.sgst, 0.0);
        assert_eq!(totals.igst, 18_900.00);
        assert_eq!(totals.grand_total, 123_900.00);
    }

    /// Same money either way — only the buckets it lands in change.
    #[test]
    fn intra_and_inter_state_grand_totals_agree() {
        let lines = [line(3.0, 1_250.0, 12.0), line(2.0, 480.0, 5.0)];
        let intra = compute_totals(&lines, "TN", "TN");
        let inter = compute_totals(&lines, "TN", "MH");

        assert_eq!(intra.grand_total, inter.grand_total);
        assert_eq!(intra.cgst + intra.sgst, inter.igst);
    }

    #[test]
    fn mixed_rate_lines_are_taxed_per_line() {
        // 2 x 1,000 @ 18% -> 180 CGST + 180 SGST; 1 x 500 @ 5% -> 12.50 each.
        let totals = compute_totals(&[line(2.0, 1_000.0, 18.0), line(1.0, 500.0, 5.0)], "TN", "TN");

        assert_eq!(totals.subtotal, 2_500.00);
        assert_eq!(totals.cgst, 192.50);
        assert_eq!(totals.sgst, 192.50);
        assert_eq!(totals.grand_total, 2_885.00);
    }

    #[test]
    fn state_comparison_ignores_case_and_padding() {
        assert!(is_intra_state("TN", "tn"));
        assert!(is_intra_state("TN", " TN "));
        assert!(!is_intra_state("TN", "KA"));
    }

    #[test]
    fn line_totals_and_tax_round_to_paise() {
        // 3 x 33.335 = 100.005 -> 100.01, and 18% of that halves to 9.00 each.
        let l = line(3.0, 33.335, 18.0);
        assert_eq!(l.line_total(), 100.01);

        let split = split_line_tax(l.line_total(), l.tax_rate, true);
        assert_eq!(split.cgst, 9.00);
        assert_eq!(split.sgst, 9.00);
    }

    #[test]
    fn zero_rated_lines_add_no_tax() {
        let totals = compute_totals(&[line(4.0, 25.0, 0.0)], "TN", "TN");
        assert_eq!(totals.subtotal, 100.00);
        assert_eq!(totals.grand_total, 100.00);
        assert_eq!(totals.cgst, 0.0);
    }

    #[test]
    fn an_empty_invoice_totals_zero() {
        assert_eq!(compute_totals(&[], "TN", "TN"), InvoiceTotals::default());
    }
}
