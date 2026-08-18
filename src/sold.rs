//! What gear actually sold for.
//!
//! Reverb retired its public Price Guide, and gearprice spent its first four commits
//! concluding from that `403` that sold prices were gone. They were not: the data moved
//! to a per-model sub-resource that is still public, still free, and reports both what
//! each item was listed at and what somebody paid for it.
//!
//! It matters more than a missing feature. Asking prices run well above sold — 16% on a
//! Les Paul Standard '60s, 21% on an American Professional II Stratocaster, 46% on a
//! Marshall JMP Major — so a report anchored to asking prices is not a slightly optimistic
//! answer to "what does this cost", it is an answer to a different question.
//!
//! # How much history to read
//!
//! Recency matters: the Major sold for about $2,895 in 2014 and $4,100 in 2026, so a
//! twelve-year median describes nothing anyone can buy today. But a fixed *time* window
//! is wrong too, because sale density varies by three orders of magnitude between models.
//! The newest fifty Les Paul Standard sales span about six weeks; the Marshall Major's
//! entire recorded history is thirty-nine sales over twelve years.
//!
//! So this reads the newest N sales rather than a fixed period, and reports the span they
//! turned out to cover. A busy model gets six recent weeks, a rare one gets everything
//! there is, and the reader is told which of those they are looking at.

use anyhow::Result;

use crate::money::Money;
use crate::quantile::REPORT_PERCENTILES;
use crate::reverb::{Client, MAX_PER_PAGE, Sale};

/// How many recent sales to read before stopping. Two pages.
pub const TARGET_SAMPLE: u32 = 100;

/// Never read more than this, however many the model has.
const MAX_PAGES: u32 = 6;

/// Below this many sales, quote the figures but do not let them carry percentiles.
pub const ENOUGH_FOR_BANDS: usize = 8;

/// A record reaching further back than this needs a recent figure quoted beside it.
const LONG_HISTORY_DAYS: i64 = 730;

/// How far back "recently" reaches when a record is long.
const RECENT_WINDOW_DAYS: i64 = 365;

/// Fewer recent sales than this and the recent median is an anecdote, not a figure.
const ENOUGH_RECENT: usize = 3;

/// One sale, reduced to what a price report reasons about.
#[derive(Clone, Debug)]
pub struct Transaction {
    pub date: String,
    pub condition: String,
    pub asked: Option<Money>,
    pub paid: Money,
}

impl Transaction {
    /// What the buyer got off the asking price, as a fraction. Negative is a discount.
    pub fn discount(&self) -> Option<f64> {
        let asked = self.asked?;
        (asked.minor > 0).then(|| (self.paid.minor - asked.minor) as f64 / asked.minor as f64)
    }
}

/// A model's recent sold history.
#[derive(Clone, Debug)]
pub struct Sold {
    pub currency: String,
    /// How many sales Reverb has on record, which may be far more than were read.
    pub recorded: u32,
    pub sales: Vec<Transaction>,
    /// Fraction to price paid, ascending.
    pub percentiles: Vec<(f64, Money)>,
    /// Oldest and newest sale actually read.
    pub covering: Option<(String, String)>,
    /// Median of what buyers got off asking, and how many sales carried both prices.
    pub typical_discount: Option<f64>,
    pub discounts_seen: usize,
    /// How many of those went at or above the asking price.
    pub at_or_above_ask: usize,
    /// What each condition grade sold for, best grade first.
    pub grades: Vec<SoldGrade>,
    /// The median of the last year's sales, quoted only when the record runs back far
    /// enough that the overall median is describing a different market.
    ///
    /// The Marshall Major is the case that demands this: thirty-nine sales spanning 2014
    /// to 2026 have a median of $2,900, while the last of them went for $4,100. Quoting
    /// the twelve-year figure alone to somebody buying this week would be useless.
    pub recent_median: Option<Money>,
    pub recent_sales: usize,
}

#[derive(Clone, Debug)]
pub struct SoldGrade {
    pub grade: String,
    pub sales: u32,
    pub median: Money,
}

impl Sold {
    /// No sold history — because the model has none, or because reading it failed.
    ///
    /// Distinct from a price of zero in every way that matters: the percentiles are
    /// empty rather than zeroed, so nothing downstream can mistake silence for cheapness.
    pub fn none(currency: &str) -> Self {
        Self {
            currency: currency.to_string(),
            recorded: 0,
            sales: Vec::new(),
            percentiles: Vec::new(),
            covering: None,
            typical_discount: None,
            discounts_seen: 0,
            at_or_above_ask: 0,
            grades: Vec::new(),
            recent_median: None,
            recent_sales: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sales.is_empty()
    }

    /// Whether there are enough sales to cut into bands without inventing precision.
    pub fn carries_bands(&self) -> bool {
        self.sales.len() >= ENOUGH_FOR_BANDS
    }

    pub fn percentile(&self, fraction: f64) -> Option<Money> {
        self.percentiles
            .iter()
            .find(|(wanted, _)| (wanted - fraction).abs() < f64::EPSILON)
            .map(|(_, price)| *price)
    }

    pub fn median(&self) -> Option<Money> {
        self.percentile(0.50)
    }
}

/// Reads a model's recent sold history.
///
/// A model with no recorded sales is not an error and not a price of zero — it is a model
/// nobody has sold through Reverb, which is itself worth reporting.
pub fn read(client: &Client, model_id: u64, sample: u32) -> Result<Sold> {
    let currency = client.currency().to_string();
    let wanted = sample.max(1);
    let mut sales: Vec<Transaction> = Vec::new();
    let mut recorded = 0;

    for page in 1..=MAX_PAGES {
        let found = client.transactions(model_id, page, MAX_PER_PAGE)?;
        recorded = found.total;
        let before = sales.len();
        sales.extend(found.transactions.iter().filter_map(convert));
        let exhausted = sales.len() == before || page >= found.total_pages.max(1);
        if sales.len() as u32 >= wanted || exhausted {
            break;
        }
    }
    sales.truncate(wanted as usize);

    let mut paid: Vec<Money> = sales.iter().map(|sale| sale.paid).collect();
    paid.sort_unstable();
    let percentiles = REPORT_PERCENTILES
        .iter()
        .map(|fraction| (*fraction, nearest_rank(&paid, *fraction)))
        .collect();

    let mut discounts: Vec<f64> = sales.iter().filter_map(Transaction::discount).collect();
    discounts.sort_by(f64::total_cmp);
    let at_or_above_ask = discounts.iter().filter(|value| **value >= 0.0).count();
    let typical_discount = (!discounts.is_empty()).then(|| discounts[discounts.len() / 2]);

    let covering = sales
        .last()
        .zip(sales.first())
        .map(|(oldest, newest)| (oldest.date.clone(), newest.date.clone()));

    let (recent_median, recent_sales) = recent(&sales);

    Ok(Sold {
        grades: grade_medians(&sales),
        recent_median,
        recent_sales,
        discounts_seen: discounts.len(),
        currency,
        recorded,
        sales,
        percentiles,
        covering,
        typical_discount,
        at_or_above_ask,
    })
}

/// The median of the last year of sales, when the record is long enough for the overall
/// median to be describing an older market than the one somebody is buying in.
fn recent(sales: &[Transaction]) -> (Option<Money>, usize) {
    let (Some(newest), Some(oldest)) = (sales.first(), sales.last()) else {
        return (None, 0);
    };
    let Some(span) = days_between(&oldest.date, &newest.date) else {
        return (None, 0);
    };
    if span < LONG_HISTORY_DAYS {
        return (None, 0);
    }
    let mut recent: Vec<Money> = sales
        .iter()
        .filter(|sale| {
            days_between(&sale.date, &newest.date).is_some_and(|age| age <= RECENT_WINDOW_DAYS)
        })
        .map(|sale| sale.paid)
        .collect();
    if recent.len() < ENOUGH_RECENT {
        return (None, recent.len());
    }
    recent.sort_unstable();
    let count = recent.len();
    (Some(recent[count / 2]), count)
}

/// Days between two `YYYY-MM-DD` dates, without pulling in date parsing for the purpose.
fn days_between(from: &str, to: &str) -> Option<i64> {
    Some(day_number(to)? - day_number(from)?)
}

/// A day count from a `YYYY-MM-DD` date, by the usual civil-date arithmetic.
fn day_number(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.get(..2)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn convert(sale: &Sale) -> Option<Transaction> {
    let paid = sale.price_final.as_ref()?;
    // A sale with no final price is a record of nothing.
    (paid.amount_cents > 0).then(|| Transaction {
        date: sale.date.clone(),
        condition: sale.condition.clone(),
        asked: sale
            .price_ask
            .as_ref()
            .map(|price| Money::from_minor(price.amount_cents))
            .filter(|asked| asked.minor > 0),
        paid: Money::from_minor(paid.amount_cents),
    })
}

/// What each condition grade sold for.
///
/// Worth having even where the asking-price version is not: on the Marshall Major the
/// asking prices put `Good` above `Mint`, while what people actually paid runs in order —
/// $2,700, $2,986, $3,225. Sold prices know what condition is worth; sellers guess.
fn grade_medians(sales: &[Transaction]) -> Vec<SoldGrade> {
    let mut by_grade: Vec<(String, Vec<Money>)> = Vec::new();
    for sale in sales {
        if sale.condition.is_empty() {
            continue;
        }
        match by_grade
            .iter_mut()
            .find(|(grade, _)| grade.eq_ignore_ascii_case(&sale.condition))
        {
            Some((_, prices)) => prices.push(sale.paid),
            None => by_grade.push((sale.condition.clone(), vec![sale.paid])),
        }
    }
    let mut grades: Vec<(usize, SoldGrade)> = by_grade
        .into_iter()
        .map(|(grade, mut prices)| {
            prices.sort_unstable();
            (
                crate::market::grade_rank_of(&grade),
                SoldGrade {
                    sales: prices.len() as u32,
                    median: prices[prices.len() / 2],
                    grade,
                },
            )
        })
        .collect();
    grades.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.grade.cmp(&right.1.grade))
    });
    grades.into_iter().map(|(_, grade)| grade).collect()
}

/// The same nearest-rank rule the live-market percentiles use, so the two are comparable.
fn nearest_rank(sorted: &[Money], fraction: f64) -> Money {
    if sorted.is_empty() {
        return Money::ZERO;
    }
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sale(date: &str, condition: &str, asked: Option<i64>, paid: i64) -> Transaction {
        Transaction {
            date: date.into(),
            condition: condition.into(),
            asked: asked.map(Money::from_minor),
            paid: Money::from_minor(paid),
        }
    }

    /// The Marshall Major's real record, trimmed to the sales that matter here.
    fn major() -> Sold {
        let sales = vec![
            sale("2026-01-14", "Good", Some(500_000), 410_000),
            sale("2025-07-22", "Excellent", Some(544_500), 544_500),
            sale("2025-06-22", "Good", Some(389_500), 300_000),
            sale("2025-01-28", "Very Good", Some(350_000), 335_000),
            sale("2024-12-07", "Excellent", Some(499_900), 556_600),
            sale("2023-06-25", "Good", Some(250_000), 250_000),
            sale("2023-01-22", "Good", Some(360_000), 330_000),
            sale("2022-12-17", "Good", Some(400_000), 400_000),
            sale("2022-09-07", "Good", Some(300_000), 270_000),
        ];
        let mut paid: Vec<Money> = sales.iter().map(|sale| sale.paid).collect();
        paid.sort_unstable();
        let mut discounts: Vec<f64> = sales.iter().filter_map(Transaction::discount).collect();
        discounts.sort_by(f64::total_cmp);
        Sold {
            currency: "USD".into(),
            recorded: 39,
            percentiles: REPORT_PERCENTILES
                .iter()
                .map(|fraction| (*fraction, nearest_rank(&paid, *fraction)))
                .collect(),
            covering: Some(("2022-09-07".into(), "2026-01-14".into())),
            typical_discount: Some(discounts[discounts.len() / 2]),
            at_or_above_ask: discounts.iter().filter(|value| **value >= 0.0).count(),
            discounts_seen: discounts.len(),
            grades: grade_medians(&sales),
            recent_median: recent(&sales).0,
            recent_sales: recent(&sales).1,
            sales,
        }
    }

    #[test]
    fn a_discount_is_what_the_buyer_got_off_and_a_premium_is_positive() {
        // Asked $5,000, paid $4,100.
        assert!(
            (sale("x", "Good", Some(500_000), 410_000)
                .discount()
                .unwrap()
                + 0.18)
                .abs()
                < 1e-9
        );
        // Asked $4,999, paid $5,566 — it happens, and the number should not hide it.
        assert!(
            sale("x", "Excellent", Some(499_900), 556_600)
                .discount()
                .unwrap()
                > 0.0
        );
        // No asking price recorded is not a discount of zero.
        assert_eq!(None, sale("x", "Good", None, 410_000).discount());
        assert_eq!(None, sale("x", "Good", Some(0), 410_000).discount());
    }

    #[test]
    fn the_typical_discount_is_a_median_and_the_spread_is_reported_beside_it() {
        let sold = major();
        // A few per cent off the middle sale, while the spread runs from 23% off to 11%
        // over. That is why this is quoted as a median with the spread beside it: four of
        // these nine went at or above the asking price.
        let discount = sold.typical_discount.unwrap();
        assert!(
            (-0.06..=-0.03).contains(&discount),
            "discount was {discount}"
        );
        assert_eq!(4, sold.at_or_above_ask);
        assert_eq!(9, sold.discounts_seen);
    }

    #[test]
    fn what_people_paid_orders_the_condition_grades_that_asking_prices_scrambled() {
        // The finding this exists for: on this model the asking prices put Good above
        // Mint, while what buyers actually paid runs in grade order.
        let grades = major().grades;
        assert_eq!(
            vec!["Excellent", "Very Good", "Good"],
            grades
                .iter()
                .map(|grade| grade.grade.as_str())
                .collect::<Vec<_>>()
        );
        let excellent = grades[0].median.minor;
        let good = grades[2].median.minor;
        assert!(
            excellent > good,
            "excellent {excellent} should beat good {good}"
        );
    }

    #[test]
    fn percentiles_follow_the_same_rule_as_the_live_market_so_the_two_compare() {
        let sold = major();
        assert_eq!(Some(Money::from_minor(335_000)), sold.median());
        assert_eq!(Some(Money::from_minor(250_000)), sold.percentile(0.05));
        assert!(sold.carries_bands());
    }

    #[test]
    fn a_long_record_quotes_the_last_year_beside_the_lot() {
        // The Marshall Major's problem: twelve years of sales whose overall median
        // describes a market that has moved on. Newest first, as the API returns them.
        let sales = vec![
            sale("2026-01-14", "Good", Some(500_000), 410_000),
            sale("2025-07-22", "Excellent", Some(544_500), 544_500),
            sale("2025-06-22", "Good", Some(389_500), 300_000),
            sale("2025-01-28", "Very Good", Some(350_000), 335_000),
            sale("2015-06-11", "Good", Some(325_000), 260_000),
            sale("2015-03-08", "Good", Some(325_000), 270_000),
            sale("2014-08-30", "Very Good", Some(330_000), 289_500),
        ];
        let (median, count) = recent(&sales);
        // The last year of it: $4,100, $5,445, $3,000 and $3,350 — median $4,100, against
        // the $3,000 the whole twelve-year record medians out to.
        assert_eq!(4, count);
        assert_eq!(Some(Money::from_minor(410_000)), median);

        // A record that does not reach back far enough needs no such qualifier.
        let short = vec![
            sale("2026-08-18", "Good", Some(200_000), 180_000),
            sale("2026-07-01", "Good", Some(200_000), 190_000),
            sale("2026-06-03", "Good", Some(200_000), 185_000),
        ];
        assert_eq!((None, 0), recent(&short));
    }

    #[test]
    fn dates_subtract_the_way_a_calendar_does() {
        assert_eq!(Some(1), days_between("2026-08-17", "2026-08-18"));
        assert_eq!(Some(365), days_between("2025-08-18", "2026-08-18"));
        // Across a leap day, and across a century that is not a leap year.
        assert_eq!(Some(366), days_between("2024-01-01", "2025-01-01"));
        assert_eq!(Some(4_158), days_between("2014-08-27", "2026-01-14"));
        assert_eq!(None, days_between("not-a-date", "2026-08-18"));
        assert_eq!(None, day_number("2026-13-01"));
    }

    #[test]
    fn a_thin_record_is_reported_but_not_cut_into_bands() {
        let sales = vec![sale("2026-01-14", "Good", Some(500_000), 410_000)];
        let sold = Sold {
            currency: "USD".into(),
            recorded: 1,
            percentiles: vec![(0.50, Money::from_minor(410_000))],
            covering: Some(("2026-01-14".into(), "2026-01-14".into())),
            typical_discount: None,
            at_or_above_ask: 0,
            discounts_seen: 0,
            grades: grade_medians(&sales),
            recent_median: None,
            recent_sales: 0,
            sales,
        };
        assert!(!sold.is_empty());
        assert!(!sold.carries_bands(), "one sale cannot carry five bands");
    }

    #[test]
    fn a_model_nobody_has_sold_is_empty_rather_than_free() {
        let sold = Sold::none("USD");
        assert!(sold.is_empty());
        assert!(!sold.carries_bands());
        assert_eq!(None, sold.covering);
        // Not a percentile of zero: no number at all, so nothing can quote it as a price.
        assert_eq!(None, sold.median());
        assert!(sold.percentiles.is_empty());
    }
}
