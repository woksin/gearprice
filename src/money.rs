//! Money as integer minor units, plus the formatting the tables need.
//!
//! Reverb reports every price twice: as a decimal string and as an integer count of
//! minor units. gearprice keeps the integer, so summing, sorting and comparing prices
//! never goes through binary floating point.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Currencies whose minor unit is the whole unit — no cents to divide by.
const ZERO_DECIMAL: [&str; 8] = ["JPY", "KRW", "CLP", "ISK", "VND", "XAF", "XOF", "XPF"];

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Money {
    /// Minor units — cents for USD, øre for NOK, whole yen for JPY.
    pub minor: i64,
}

impl Money {
    pub const ZERO: Self = Self { minor: 0 };

    pub fn from_minor(minor: i64) -> Self {
        Self { minor }
    }

    pub fn from_major(major: f64, currency: &str) -> Self {
        Self {
            minor: (major * f64::from(minor_units(currency))).round() as i64,
        }
    }

    pub fn major(self, currency: &str) -> f64 {
        self.minor as f64 / f64::from(minor_units(currency))
    }

    /// Rounded up to the next whole major unit — the form Reverb's `price_min` and
    /// `price_max` parameters take.
    pub fn major_ceil(self, currency: &str) -> i64 {
        let divisor = i64::from(minor_units(currency));
        self.minor.div_euclid(divisor) + i64::from(self.minor.rem_euclid(divisor) != 0)
    }

    pub fn major_floor(self, currency: &str) -> i64 {
        self.minor.div_euclid(i64::from(minor_units(currency)))
    }
}

pub fn minor_units(currency: &str) -> u32 {
    if ZERO_DECIMAL.contains(&currency) {
        1
    } else {
        100
    }
}

pub fn symbol(currency: &str) -> &str {
    match currency {
        "USD" | "CAD" | "AUD" | "NZD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        _ => "",
    }
}

/// `$1,750` — the compact form used in tables, with decimals dropped above a hundred
/// units where they are noise rather than information.
pub fn format(amount: Money, currency: &str) -> String {
    let major = amount.major(currency);
    // Decimals earn their place only on small amounts: cents on a $40,000 guitar are
    // noise, cents on a $9.95 patch cable are the price.
    let decimals = if major.abs() < 100.0 && minor_units(currency) != 1 {
        2
    } else {
        0
    };
    let symbol = symbol(currency);
    let rendered = group_thousands(major, decimals);
    if symbol.is_empty() {
        format!("{rendered} {currency}")
    } else {
        format!("{symbol}{rendered}")
    }
}

/// `1,750.00` — digits only, for CSV and for callers supplying their own currency label.
pub fn group_thousands(value: f64, decimals: usize) -> String {
    let rendered = format!("{:.*}", decimals, value.abs());
    let (whole, fraction) = match rendered.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (rendered.as_str(), None),
    };
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3 + 4);
    if value < 0.0 {
        grouped.push('-');
    }
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if let Some(fraction) = fraction {
        grouped.push('.');
        grouped.push_str(fraction);
    }
    grouped
}

impl fmt::Display for Money {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.minor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_decimal_currencies_keep_whole_units() {
        assert_eq!(1, minor_units("JPY"));
        assert_eq!(100, minor_units("NOK"));
        assert_eq!(120_000.0, Money::from_minor(120_000).major("JPY"));
        assert_eq!(1_200.0, Money::from_minor(120_000).major("NOK"));
    }

    #[test]
    fn major_rounding_brackets_the_amount_from_both_sides() {
        let amount = Money::from_minor(175_050);
        assert_eq!(1_751, amount.major_ceil("USD"));
        assert_eq!(1_750, amount.major_floor("USD"));
        // An exact amount is its own ceiling — no phantom extra unit.
        assert_eq!(1_750, Money::from_minor(175_000).major_ceil("USD"));
    }

    #[test]
    fn formatting_groups_thousands_and_drops_noise_decimals() {
        assert_eq!("$1,750", format(Money::from_minor(175_000), "USD"));
        assert_eq!("$99.95", format(Money::from_minor(9_995), "USD"));
        assert_eq!("24,913 NOK", format(Money::from_minor(2_491_286), "NOK"));
        assert_eq!("€2,180", format(Money::from_minor(218_000), "EUR"));
    }
}
