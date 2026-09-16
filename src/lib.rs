// Money and quantities are fixed-point integers (cents, thousandths of a
// unit) rather than f64. An invoice that silently drifts by half a cent
// because of float rounding is a bug someone in accounting will find before
// we do.

use std::fmt;
use std::str::FromStr;

/// An amount of money, stored as integer cents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Money(i64);

impl Money {
    pub fn from_cents(cents: i64) -> Self {
        Money(cents)
    }

    pub fn cents(self) -> i64 {
        self.0
    }

    pub fn checked_add(self, other: Money) -> Option<Money> {
        self.0.checked_add(other.0).map(Money)
    }

    pub fn checked_sub(self, other: Money) -> Option<Money> {
        self.0.checked_sub(other.0).map(Money)
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:02}", abs / 100, abs % 100)
    }
}

impl FromStr for Money {
    type Err = ParseMoneyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_fixed_point(s, 2)
            .map(Money)
            .map_err(|()| ParseMoneyError(s.to_string()))
    }
}

/// A quantity of units, stored as thousandths (three decimal places), which
/// is enough for things like 2.5 hours or 0.125 kg without pulling in a
/// decimal library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Quantity(i64);

impl Quantity {
    pub fn from_thousandths(thousandths: i64) -> Self {
        Quantity(thousandths)
    }

    pub fn as_thousandths(self) -> i64 {
        self.0
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:03}", abs / 1000, abs % 1000)
    }
}

impl FromStr for Quantity {
    type Err = ParseQuantityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_fixed_point(s, 3)
            .map(Quantity)
            .map_err(|()| ParseQuantityError(s.to_string()))
    }
}

/// One priced line on an invoice: some quantity of a thing, a unit price,
/// a discount, and a tax rate. `discount_bps` and `tax_bps` are basis
/// points (1/100 of a percent), so 825 means 8.25%.
#[derive(Debug, Clone)]
pub struct LineItem {
    pub description: String,
    pub quantity: Quantity,
    pub unit_price: Money,
    pub discount_bps: u32,
    pub tax_bps: u32,
}

/// The result of pricing a `LineItem`: subtotal, the discount taken off it,
/// tax charged on what's left, and the final total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineTotals {
    pub subtotal: Money,
    pub discount: Money,
    pub tax: Money,
    pub total: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineItemError {
    Overflow,
    InvalidDiscount(u32),
}

impl fmt::Display for LineItemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LineItemError::Overflow => write!(f, "arithmetic overflow while computing line totals"),
            LineItemError::InvalidDiscount(bps) => {
                write!(f, "discount_bps {bps} exceeds 10000 (100%)")
            }
        }
    }
}

impl std::error::Error for LineItemError {}

impl LineItem {
    /// Computes subtotal, discount, tax, and total.
    ///
    /// Discount is taken off the subtotal first, then tax is applied to
    /// what's left; each step rounds to the nearest cent independently,
    /// rounding exact halves away from zero. That means a return (negative
    /// quantity) rounds its subtotal down in magnitude the same way a sale
    /// rounds up, rather than always rounding toward positive infinity.
    pub fn totals(&self) -> Result<LineTotals, LineItemError> {
        if self.discount_bps > 10_000 {
            return Err(LineItemError::InvalidDiscount(self.discount_bps));
        }

        let subtotal = proportional(self.unit_price.cents(), self.quantity.as_thousandths(), 1000)?;
        let discount = proportional(subtotal.cents(), self.discount_bps as i64, 10_000)?;
        let after_discount = subtotal.checked_sub(discount).ok_or(LineItemError::Overflow)?;
        let tax = proportional(after_discount.cents(), self.tax_bps as i64, 10_000)?;
        let total = after_discount.checked_add(tax).ok_or(LineItemError::Overflow)?;

        Ok(LineTotals { subtotal, discount, tax, total })
    }
}

/// Computes `round_half_away_from_zero(amount_cents * multiplier / denominator)`
/// as a `Money`, widening to i128 so the intermediate product can't overflow
/// before rounding, then narrowing back and reporting overflow if the
/// result doesn't fit in a cent count we can actually represent.
fn proportional(amount_cents: i64, multiplier: i64, denominator: i64) -> Result<Money, LineItemError> {
    let product = (amount_cents as i128) * (multiplier as i128);
    let rounded = round_half_away_from_zero(product, denominator as i128);
    let cents = i64::try_from(rounded).map_err(|_| LineItemError::Overflow)?;
    Ok(Money::from_cents(cents))
}

fn round_half_away_from_zero(numerator: i128, denominator: i128) -> i128 {
    let half = denominator / 2;
    if numerator >= 0 {
        (numerator + half) / denominator
    } else {
        -((-numerator + half) / denominator)
    }
}

/// Parses a decimal string like "12.34" or "-0.5" into an integer scaled by
/// `10^decimals`. Rejects more fractional digits than `decimals` instead of
/// truncating, so a quantity like "1.2345" fails loudly rather than quietly
/// losing precision.
fn parse_fixed_point(s: &str, decimals: usize) -> Result<i64, ()> {
    let s = s.trim();
    if s.is_empty() {
        return Err(());
    }

    let (negative, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };

    let mut parts = rest.splitn(2, '.');
    let whole = parts.next().unwrap();
    let frac = parts.next().unwrap_or("");

    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    if frac.len() > decimals || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }

    let whole_val: i64 = whole.parse().map_err(|_| ())?;
    let mut frac_val: i64 = if frac.is_empty() { 0 } else { frac.parse().map_err(|_| ())? };
    for _ in frac.len()..decimals {
        frac_val = frac_val.checked_mul(10).ok_or(())?;
    }

    let scale = 10i64.checked_pow(decimals as u32).ok_or(())?;
    let magnitude = whole_val
        .checked_mul(scale)
        .and_then(|v| v.checked_add(frac_val))
        .ok_or(())?;

    Ok(if negative { -magnitude } else { magnitude })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseMoneyError(String);

impl fmt::Display for ParseMoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid money value {:?} (expected e.g. \"12.34\" or \"-5\")", self.0)
    }
}

impl std::error::Error for ParseMoneyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseQuantityError(String);

impl fmt::Display for ParseQuantityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid quantity {:?} (expected e.g. \"2.5\", up to 3 decimal places)",
            self.0
        )
    }
}

impl std::error::Error for ParseQuantityError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row per awkward case we've hit or expect to hit: fractional
    /// quantities, credits (negative quantities), a discount that zeroes
    /// tax, rounding exactly on a half cent in both directions, and the
    /// two error paths (overflow, out-of-range discount).
    struct Case {
        name: &'static str,
        quantity: i64,
        unit_price: i64,
        discount_bps: u32,
        tax_bps: u32,
        expected: Result<(i64, i64, i64, i64), LineItemError>,
    }

    fn cases() -> Vec<Case> {
        vec![
            Case {
                name: "whole units, no discount or tax",
                quantity: 2000,
                unit_price: 500,
                discount_bps: 0,
                tax_bps: 0,
                expected: Ok((1000, 0, 0, 1000)),
            },
            Case {
                name: "fractional quantity rounds an exact half cent up",
                quantity: 500,
                unit_price: 3,
                discount_bps: 0,
                tax_bps: 0,
                expected: Ok((2, 0, 0, 2)),
            },
            Case {
                name: "negative quantity (a credit) rounds an exact half away from zero",
                quantity: -500,
                unit_price: 3,
                discount_bps: 0,
                tax_bps: 0,
                expected: Ok((-2, 0, 0, -2)),
            },
            Case {
                name: "zero quantity zeroes everything even with discount and tax set",
                quantity: 0,
                unit_price: 999,
                discount_bps: 500,
                tax_bps: 800,
                expected: Ok((0, 0, 0, 0)),
            },
            Case {
                name: "discount rounds an exact half away from zero",
                quantity: 1000,
                unit_price: 100,
                discount_bps: 150,
                tax_bps: 0,
                expected: Ok((100, 2, 0, 98)),
            },
            Case {
                name: "a 100% discount leaves nothing for tax to apply to",
                quantity: 1000,
                unit_price: 500,
                discount_bps: 10_000,
                tax_bps: 2000,
                expected: Ok((500, 500, 0, 0)),
            },
            Case {
                name: "discount then tax each round independently",
                quantity: 1000,
                unit_price: 999,
                discount_bps: 1000,
                tax_bps: 825,
                expected: Ok((999, 100, 74, 973)),
            },
            Case {
                name: "quantity times price overflows an i64 cent count",
                quantity: 2000,
                unit_price: i64::MAX,
                discount_bps: 0,
                tax_bps: 0,
                expected: Err(LineItemError::Overflow),
            },
            Case {
                name: "a discount above 100% is rejected outright",
                quantity: 1000,
                unit_price: 100,
                discount_bps: 10_001,
                tax_bps: 0,
                expected: Err(LineItemError::InvalidDiscount(10_001)),
            },
        ]
    }

    #[test]
    fn totals_table() {
        for case in cases() {
            let item = LineItem {
                description: case.name.to_string(),
                quantity: Quantity(case.quantity),
                unit_price: Money(case.unit_price),
                discount_bps: case.discount_bps,
                tax_bps: case.tax_bps,
            };

            let got = item
                .totals()
                .map(|t| (t.subtotal.cents(), t.discount.cents(), t.tax.cents(), t.total.cents()));

            assert_eq!(got, case.expected, "case: {}", case.name);
        }
    }

    #[test]
    fn money_parses_dollars_and_cents() {
        assert_eq!("12.34".parse::<Money>().unwrap(), Money(1234));
        assert_eq!("-5".parse::<Money>().unwrap(), Money(-500));
        assert_eq!("0.05".parse::<Money>().unwrap(), Money(5));
        assert!("1.234".parse::<Money>().is_err());
        assert!("abc".parse::<Money>().is_err());
    }

    #[test]
    fn quantity_parses_up_to_three_decimal_places() {
        assert_eq!("2.5".parse::<Quantity>().unwrap(), Quantity(2500));
        assert_eq!("-0.5".parse::<Quantity>().unwrap(), Quantity(-500));
        assert!("1.2345".parse::<Quantity>().is_err());
    }

    #[test]
    fn money_display_pads_single_digit_cents() {
        assert_eq!(Money(105).to_string(), "1.05");
        assert_eq!(Money(-105).to_string(), "-1.05");
    }
}
