//! GST split and invoice totals.
//!
//! One rule drives everything: compare the buyer's place of supply against the seller's
//! home state. Same state is an intra-state sale, and the item's GST rate is split evenly
//! into CGST and SGST. Different states is an inter-state sale, and the whole rate is
//! charged as IGST.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::models::DiscountType;

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

/// Totals an invoice with no discounts. Tax is split per line and then summed, so lines
/// at different GST rates on one invoice each get their own correct split.
///
/// This is [`compute_discounted_totals`] with nothing to discount, rather than a second
/// implementation of the same arithmetic — an undiscounted bill and a discounted one
/// cannot drift apart, because there is only one of them. It cannot fail: every
/// validation in that function is about a discount, and there are none here.
pub fn compute_totals(
    lines: &[TaxableLine],
    home_state: &str,
    place_of_supply: &str,
) -> InvoiceTotals {
    let plain: Vec<DiscountedLine> =
        lines.iter().map(|l| DiscountedLine::plain(l.qty, l.rate, l.tax_rate)).collect();

    let totals =
        compute_discounted_totals(&plain, DiscountType::None, 0.0, home_state, place_of_supply)
            .expect("an invoice with no discounts has nothing that can fail validation");

    InvoiceTotals {
        subtotal: totals.subtotal,
        cgst: totals.cgst,
        sgst: totals.sgst,
        igst: totals.igst,
        grand_total: totals.grand_total,
    }
}

// ------------------------------------------------------------------ discounts

/// One line as billed, before any of the maths has been done to it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DiscountedLine {
    pub qty: f64,
    /// The price-list-resolved rate. Discounts come off this, never the other way round.
    pub rate: f64,
    pub tax_rate: f64,
    pub discount_type: DiscountType,
    pub discount_value: f64,
}

impl DiscountedLine {
    pub fn plain(qty: f64, rate: f64, tax_rate: f64) -> Self {
        DiscountedLine {
            qty,
            rate,
            tax_rate,
            discount_type: DiscountType::None,
            discount_value: 0.0,
        }
    }

    /// `qty * rate`, before any discount. The "what it would have cost" figure.
    pub fn line_gross(&self) -> f64 {
        round_money(self.qty * self.rate)
    }
}

/// What one line came to once every discount had been taken off it. Every field here is
/// stored on the invoice line as-is; nothing recomputes them afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PricedLine {
    pub line_gross: f64,
    /// This line's own discount, in rupees.
    pub discount_amount: f64,
    /// This line's share of the invoice-level discount, in rupees.
    pub invoice_discount_share: f64,
    /// `line_gross - discount_amount - invoice_discount_share`. What tax was charged on.
    pub taxable_value: f64,
    pub tax_rate: f64,
    pub tax: TaxSplit,
}

/// An invoice's totals with the discount working shown.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiscountedTotals {
    /// Sum of every line's `qty * rate`, before any discount.
    pub pre_discount_subtotal: f64,
    /// Line-level discounts only.
    pub line_discount_total: f64,
    /// The invoice-level discount, in rupees, however it was expressed.
    pub invoice_discount_amount: f64,
    /// Both of the above. The single figure the summary panel shows.
    pub discount_amount: f64,
    /// The taxable value: what is left after every discount, and what tax was charged on.
    pub subtotal: f64,
    pub cgst: f64,
    pub sgst: f64,
    pub igst: f64,
    pub grand_total: f64,
    pub lines: Vec<PricedLine>,
}

/// Rupees off, from a type and a value, applied to `base`.
///
/// Refuses a discount larger than what it is being taken off. A bill cannot go negative,
/// and a shop that means to give something away enters its price as zero rather than
/// discounting it by more than it costs.
fn discount_amount(kind: DiscountType, value: f64, base: f64, what: &str) -> Result<f64> {
    if matches!(kind, DiscountType::None) {
        return Ok(0.0);
    }
    if !value.is_finite() || value < 0.0 {
        return Err(CoreError::Invalid(format!("{what} discount must be 0 or more")));
    }

    let amount = match kind {
        DiscountType::None => 0.0,
        DiscountType::Percentage => {
            if value > 100.0 {
                return Err(CoreError::Invalid(format!(
                    "{what} discount cannot be more than 100%, got {value}%"
                )));
            }
            round_money(base * value / 100.0)
        }
        DiscountType::Flat => round_money(value),
    };

    if amount > base + MONEY_EPSILON {
        return Err(CoreError::Invalid(format!(
            "{what} discount of {amount:.2} is more than the {base:.2} it comes off"
        )));
    }
    Ok(amount.min(base))
}

/// Tolerance for comparing two money figures that should be equal. Half a paisa: closer
/// than that and the difference cannot survive being rounded onto an invoice.
const MONEY_EPSILON: f64 = 0.005;

/// Totals an invoice with discounts, in the order GST requires.
///
/// Per line: `qty * rate`, less that line's own discount, less its share of the
/// invoice-level discount — and *then* tax, on what is left. Tax is never computed on a
/// price the customer was not charged.
///
/// # Why the invoice-level discount is apportioned across lines
///
/// It would be simpler to take the invoice discount off the subtotal as one deduction and
/// tax the result. That only works if every line on the bill carries the same GST rate,
/// and this catalogue ships items at 5%, 18% and 28% — a single discounted subtotal has no
/// one rate to be taxed at, so the simple version would have to pick one and would be
/// wrong on every mixed-rate bill.
///
/// So the discount is split across the lines in proportion to what each contributes to the
/// subtotal *after* line discounts — which is the amount the invoice discount is actually
/// being calculated on — and each line is then taxed at its own rate on its own reduced
/// base. The share is stored per line, so the tax on any line can be re-derived from that
/// line alone by an auditor who never sees this function.
///
/// Rounding each share to paise leaves a residue of a paisa or two, which is put on the
/// last line that has anything to discount. The shares therefore sum to exactly the
/// invoice discount, rather than to a figure that is nearly it: an invoice whose parts do
/// not add up to its total is worse than one whose last line absorbs a paisa.
pub fn compute_discounted_totals(
    lines: &[DiscountedLine],
    invoice_discount_type: DiscountType,
    invoice_discount_value: f64,
    home_state: &str,
    place_of_supply: &str,
) -> Result<DiscountedTotals> {
    let intra_state = is_intra_state(home_state, place_of_supply);

    // (a) and (b): gross, then each line's own discount.
    let mut gross = Vec::with_capacity(lines.len());
    let mut own_discount = Vec::with_capacity(lines.len());
    let mut after_line_discount = Vec::with_capacity(lines.len());

    for line in lines {
        let line_gross = line.line_gross();
        let discount =
            discount_amount(line.discount_type, line.discount_value, line_gross, "a line")?;
        gross.push(line_gross);
        own_discount.push(discount);
        after_line_discount.push(round_money(line_gross - discount));
    }

    // (e) and (f): the invoice discount comes off the already-line-discounted subtotal,
    // not off the original.
    let pre_discount_subtotal = round_money(gross.iter().sum::<f64>());
    let line_discount_total = round_money(own_discount.iter().sum::<f64>());
    let after_line_total = round_money(after_line_discount.iter().sum::<f64>());
    let invoice_discount = discount_amount(
        invoice_discount_type,
        invoice_discount_value,
        after_line_total,
        "an invoice",
    )?;

    let shares = apportion(invoice_discount, &after_line_discount, after_line_total);

    // (c), (d) and (g): tax per line on what is left, then sum.
    let mut priced = Vec::with_capacity(lines.len());
    let mut totals = DiscountedTotals {
        pre_discount_subtotal,
        line_discount_total,
        invoice_discount_amount: invoice_discount,
        discount_amount: round_money(line_discount_total + invoice_discount),
        ..Default::default()
    };

    for (index, line) in lines.iter().enumerate() {
        let share = shares[index];
        let taxable_value = round_money(after_line_discount[index] - share);
        let tax = split_line_tax(taxable_value, line.tax_rate, intra_state);

        totals.subtotal += taxable_value;
        totals.cgst += tax.cgst;
        totals.sgst += tax.sgst;
        totals.igst += tax.igst;

        priced.push(PricedLine {
            line_gross: gross[index],
            discount_amount: own_discount[index],
            invoice_discount_share: share,
            taxable_value,
            tax_rate: line.tax_rate,
            tax,
        });
    }

    totals.subtotal = round_money(totals.subtotal);
    totals.cgst = round_money(totals.cgst);
    totals.sgst = round_money(totals.sgst);
    totals.igst = round_money(totals.igst);
    totals.grand_total = round_money(totals.subtotal + totals.cgst + totals.sgst + totals.igst);
    totals.lines = priced;
    Ok(totals)
}

/// Splits `amount` across `weights` in proportion, to the paisa, summing to exactly
/// `amount`. The rounding residue lands on the last weight that is not zero.
fn apportion(amount: f64, weights: &[f64], total_weight: f64) -> Vec<f64> {
    if amount <= 0.0 || total_weight <= 0.0 {
        return vec![0.0; weights.len()];
    }

    let mut shares: Vec<f64> =
        weights.iter().map(|w| round_money(amount * w / total_weight)).collect();

    let residue = round_money(amount - shares.iter().sum::<f64>());
    if residue != 0.0 {
        if let Some(last) = weights.iter().rposition(|w| *w > 0.0) {
            shares[last] = round_money(shares[last] + residue);
        }
    }
    shares
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

    // ------------------------------------------------------------- discounts

    fn disc(qty: f64, rate: f64, tax_rate: f64, kind: DiscountType, value: f64) -> DiscountedLine {
        DiscountedLine { qty, rate, tax_rate, discount_type: kind, discount_value: value }
    }

    /// The worked example, hand-calculated before the code was written.
    ///
    /// ```text
    ///   L1  1 x 45,000.00 @ 18%, no discount     gross 45,000.00
    ///   L2  5 x 12,000.00 @ 18%, 10% off         gross 60,000.00, less 6,000.00 = 54,000.00
    ///   invoice: flat 1,000.00 off
    ///
    ///   pre-discount subtotal   45,000.00 + 60,000.00           = 105,000.00
    ///   after line discounts    45,000.00 + 54,000.00           =  99,000.00
    ///   invoice discount apportioned over that 99,000.00:
    ///       L1  1,000.00 x 45,000/99,000 = 454.5454...          =     454.55
    ///       L2  1,000.00 x 54,000/99,000 = 545.4545...          =     545.45
    ///                                                   (sums to 1,000.00 exactly)
    ///   taxable   L1  45,000.00 - 454.55                        =  44,545.45
    ///             L2  54,000.00 - 545.45                        =  53,454.55
    ///                                                       total =  98,000.00
    ///   tax at 18% intra-state, 9% each side, per line:
    ///             L1  44,545.45 x 9% = 4,009.0905              ->  4,009.09
    ///             L2  53,454.55 x 9% = 4,810.9095              ->  4,810.91
    ///                                              CGST = SGST =   8,820.00
    ///   grand total  98,000.00 + 8,820.00 + 8,820.00            = 115,640.00
    /// ```
    #[test]
    fn the_worked_discount_example_matches_the_hand_calculation() {
        let totals = compute_discounted_totals(
            &[
                disc(1.0, 45_000.0, 18.0, DiscountType::None, 0.0),
                disc(5.0, 12_000.0, 18.0, DiscountType::Percentage, 10.0),
            ],
            DiscountType::Flat,
            1_000.0,
            "TN",
            "TN",
        )
        .unwrap();

        assert_eq!(totals.pre_discount_subtotal, 105_000.00);
        assert_eq!(totals.line_discount_total, 6_000.00);
        assert_eq!(totals.invoice_discount_amount, 1_000.00);
        assert_eq!(totals.discount_amount, 7_000.00);
        assert_eq!(totals.subtotal, 98_000.00);
        assert_eq!(totals.cgst, 8_820.00);
        assert_eq!(totals.sgst, 8_820.00);
        assert_eq!(totals.igst, 0.0);
        assert_eq!(totals.grand_total, 115_640.00);

        // And the per-line working the invoice will store.
        assert_eq!(totals.lines[0].line_gross, 45_000.00);
        assert_eq!(totals.lines[0].discount_amount, 0.0);
        assert_eq!(totals.lines[0].invoice_discount_share, 454.55);
        assert_eq!(totals.lines[0].taxable_value, 44_545.45);
        assert_eq!(totals.lines[0].tax.cgst, 4_009.09);

        assert_eq!(totals.lines[1].line_gross, 60_000.00);
        assert_eq!(totals.lines[1].discount_amount, 6_000.00);
        assert_eq!(totals.lines[1].invoice_discount_share, 545.45);
        assert_eq!(totals.lines[1].taxable_value, 53_454.55);
        assert_eq!(totals.lines[1].tax.cgst, 4_810.91);
    }

    /// Mixed GST rates, a flat line discount and a percentage invoice discount, also hand
    /// calculated. This is the case a single subtotal-level deduction gets wrong: there is
    /// no one rate to tax 69,479.16 at.
    ///
    /// ```text
    ///   L1  3 x    410.00 @ 28%, 7.5% off   1,230.00 -    92.25 =  1,137.75
    ///   L2  7 x  4,800.00 @  5%, flat 333  33,600.00 -   333.00 = 33,267.00
    ///   L3  1 x 45,000.00 @ 18%, none      45,000.00           = 45,000.00
    ///   pre-discount subtotal                                   = 79,830.00
    ///   after line discounts                                    = 79,404.75
    ///   invoice 12.5% of 79,404.75 = 9,925.59375               ->  9,925.59
    ///       L1  9,925.59 x  1,137.75/79,404.75 =   142.2186    ->    142.22
    ///       L2  9,925.59 x 33,267.00/79,404.75 = 4,158.3676    ->  4,158.37
    ///       L3  9,925.59 x 45,000.00/79,404.75 = 5,625.0035    ->  5,625.00
    ///   taxable   L1     995.53   L2  29,108.63   L3  39,375.00 = 69,479.16
    ///   tax       L1  @28%:    995.53 x 14%   =   139.3742     ->    139.37
    ///             L2  @ 5%: 29,108.63 x 2.5%  =   727.7158     ->    727.72
    ///             L3  @18%: 39,375.00 x 9%    = 3,543.75       ->  3,543.75
    ///                                              CGST = SGST =  4,410.84
    ///   grand total  69,479.16 + 4,410.84 + 4,410.84           = 78,300.84
    /// ```
    #[test]
    fn mixed_tax_rates_are_each_discounted_and_taxed_on_their_own_base() {
        let totals = compute_discounted_totals(
            &[
                disc(3.0, 410.0, 28.0, DiscountType::Percentage, 7.5),
                disc(7.0, 4_800.0, 5.0, DiscountType::Flat, 333.0),
                disc(1.0, 45_000.0, 18.0, DiscountType::None, 0.0),
            ],
            DiscountType::Percentage,
            12.5,
            "TN",
            "TN",
        )
        .unwrap();

        assert_eq!(totals.pre_discount_subtotal, 79_830.00);
        assert_eq!(totals.line_discount_total, 425.25);
        assert_eq!(totals.invoice_discount_amount, 9_925.59);
        assert_eq!(totals.discount_amount, 10_350.84);
        assert_eq!(totals.subtotal, 69_479.16);
        assert_eq!(totals.cgst, 4_410.84);
        assert_eq!(totals.sgst, 4_410.84);
        assert_eq!(totals.grand_total, 78_300.84);

        let shares: Vec<f64> = totals.lines.iter().map(|l| l.invoice_discount_share).collect();
        assert_eq!(shares, vec![142.22, 4_158.37, 5_625.00]);
        let taxables: Vec<f64> = totals.lines.iter().map(|l| l.taxable_value).collect();
        assert_eq!(taxables, vec![995.53, 29_108.63, 39_375.00]);
    }

    /// Whatever the rounding does to the individual shares, they add up to the discount.
    #[test]
    fn apportioned_shares_always_sum_to_the_invoice_discount() {
        // Three equal lines and 10.00 off: 3.333... each, which cannot be split evenly.
        let totals = compute_discounted_totals(
            &[
                disc(1.0, 100.0, 18.0, DiscountType::None, 0.0),
                disc(1.0, 100.0, 18.0, DiscountType::None, 0.0),
                disc(1.0, 100.0, 18.0, DiscountType::None, 0.0),
            ],
            DiscountType::Flat,
            10.0,
            "TN",
            "TN",
        )
        .unwrap();

        let shares: Vec<f64> = totals.lines.iter().map(|l| l.invoice_discount_share).collect();
        assert_eq!(shares, vec![3.33, 3.33, 3.34], "the residue lands on the last line");
        assert_eq!(round_money(shares.iter().sum::<f64>()), 10.00);
        assert_eq!(totals.subtotal, 290.00);
    }

    /// The invoice discount comes off the already-line-discounted subtotal, not the
    /// original. 10% of 900 is 90, not 10% of 1,000.
    #[test]
    fn the_invoice_discount_applies_after_line_discounts_not_before() {
        let totals = compute_discounted_totals(
            &[disc(1.0, 1_000.0, 18.0, DiscountType::Percentage, 10.0)],
            DiscountType::Percentage,
            10.0,
            "TN",
            "TN",
        )
        .unwrap();

        assert_eq!(totals.pre_discount_subtotal, 1_000.00);
        assert_eq!(totals.line_discount_total, 100.00);
        assert_eq!(totals.invoice_discount_amount, 90.00, "10% of 900, not of 1,000");
        assert_eq!(totals.subtotal, 810.00);
        assert_eq!(totals.cgst, 72.90);
        assert_eq!(totals.grand_total, 955.80);
    }

    /// Tax is charged on what was charged, never on the sticker price.
    #[test]
    fn tax_is_computed_on_the_discounted_value_not_the_gross() {
        let discounted = compute_discounted_totals(
            &[disc(1.0, 1_000.0, 18.0, DiscountType::Percentage, 50.0)],
            DiscountType::None,
            0.0,
            "TN",
            "TN",
        )
        .unwrap();
        let undiscounted = compute_totals(&[line(1.0, 1_000.0, 18.0)], "TN", "TN");

        assert_eq!(discounted.subtotal, 500.00);
        assert_eq!(discounted.cgst, 45.00, "9% of 500, not of 1,000");
        assert_eq!(undiscounted.cgst, 90.00);
        assert_eq!(discounted.grand_total, 590.00);
    }

    /// Inter-state discounting lands in the IGST bucket and comes to the same money.
    #[test]
    fn a_discounted_inter_state_bill_agrees_with_the_intra_state_one() {
        let lines = [
            disc(2.0, 1_250.0, 12.0, DiscountType::Percentage, 5.0),
            disc(3.0, 480.0, 5.0, DiscountType::Flat, 40.0),
        ];
        let intra =
            compute_discounted_totals(&lines, DiscountType::Flat, 100.0, "TN", "TN").unwrap();
        let inter =
            compute_discounted_totals(&lines, DiscountType::Flat, 100.0, "TN", "MH").unwrap();

        assert_eq!(intra.subtotal, inter.subtotal);
        assert_eq!(intra.grand_total, inter.grand_total);
        assert_eq!(round_money(intra.cgst + intra.sgst), inter.igst);
        assert_eq!(intra.igst, 0.0);
        assert_eq!(inter.cgst, 0.0);
    }

    #[test]
    fn a_discount_cannot_exceed_what_it_comes_off() {
        // More rupees off than the line costs.
        assert!(compute_discounted_totals(
            &[disc(1.0, 100.0, 18.0, DiscountType::Flat, 150.0)],
            DiscountType::None,
            0.0,
            "TN",
            "TN"
        )
        .is_err());

        // More than 100%.
        assert!(compute_discounted_totals(
            &[disc(1.0, 100.0, 18.0, DiscountType::Percentage, 120.0)],
            DiscountType::None,
            0.0,
            "TN",
            "TN"
        )
        .is_err());

        // Negative, which is a surcharge wearing a discount's clothes.
        assert!(compute_discounted_totals(
            &[disc(1.0, 100.0, 18.0, DiscountType::Flat, -10.0)],
            DiscountType::None,
            0.0,
            "TN",
            "TN"
        )
        .is_err());

        // And an invoice discount bigger than the already-discounted subtotal.
        assert!(compute_discounted_totals(
            &[disc(1.0, 100.0, 18.0, DiscountType::Percentage, 50.0)],
            DiscountType::Flat,
            60.0,
            "TN",
            "TN"
        )
        .is_err());
    }

    /// Everything free is allowed; everything free and then some is not.
    #[test]
    fn a_hundred_percent_discount_bills_and_taxes_nothing() {
        let totals = compute_discounted_totals(
            &[disc(1.0, 100.0, 18.0, DiscountType::Percentage, 100.0)],
            DiscountType::None,
            0.0,
            "TN",
            "TN",
        )
        .unwrap();

        assert_eq!(totals.pre_discount_subtotal, 100.00);
        assert_eq!(totals.discount_amount, 100.00);
        assert_eq!(totals.subtotal, 0.0);
        assert_eq!(totals.cgst, 0.0);
        assert_eq!(totals.grand_total, 0.0);
    }

    /// With no discounts at all the new path must agree with the old figures exactly.
    #[test]
    fn an_undiscounted_bill_totals_the_same_as_it_always_did() {
        let plain = [line(1.0, 45_000.0, 18.0), line(5.0, 12_000.0, 18.0)];
        let discounted: Vec<DiscountedLine> =
            plain.iter().map(|l| DiscountedLine::plain(l.qty, l.rate, l.tax_rate)).collect();

        let old = compute_totals(&plain, "TN", "TN");
        let new =
            compute_discounted_totals(&discounted, DiscountType::None, 0.0, "TN", "TN").unwrap();

        assert_eq!(old.subtotal, 105_000.00);
        assert_eq!(old.grand_total, 123_900.00);
        assert_eq!(new.subtotal, old.subtotal);
        assert_eq!(new.cgst, old.cgst);
        assert_eq!(new.grand_total, old.grand_total);
        assert_eq!(new.pre_discount_subtotal, new.subtotal, "nothing was taken off");
        assert_eq!(new.discount_amount, 0.0);
    }
}
