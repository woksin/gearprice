//! Turning a search into a picture of what that gear costs right now.
//!
//! There are two honest ways to describe a market, and which one applies depends on how
//! big it is. A market small enough to enumerate is enumerated: every listing is read, so
//! the mean, the condition mix and the listings themselves are all exact and available.
//! A market too big for that — Reverb hands over at most 2,500 listings for any search —
//! is measured instead, by counting, which [`crate::quantile`] does without the cap.
//!
//! Both paths produce the same percentiles. Only the extras differ, and the report says
//! which path it took so a reader is never guessing.

use anyhow::Result;
use rayon::prelude::*;

use crate::money::Money;
use crate::quantile::{self, REPORT_PERCENTILES};
use crate::query::{Condition, Search, Sort};
use crate::reverb::{Client, Listing, MAX_ENUMERABLE, MAX_PER_PAGE};

/// Above this many listings, counting beats downloading. Twelve pages is a few seconds
/// and a few megabytes; the alternative at the paging cap is fifty pages for information
/// the counting path gets in about the same number of far smaller requests.
const ENUMERATION_LIMIT: u32 = 600;

/// How many listings to keep for display when the market was measured rather than read.
const SAMPLE_SIZE: u32 = MAX_PER_PAGE;

/// Bars in the shape-of-the-market histogram.
const HISTOGRAM_BANDS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    /// Every matching listing was read.
    Enumerated { listings: u32 },
    /// The distribution was measured by counting, without transferring the listings.
    Counted { probes: u32 },
}

impl Method {
    pub fn label(&self) -> String {
        match self {
            Self::Enumerated { listings } => {
                format!("read all {listings} listings")
            }
            Self::Counted { probes } => {
                format!("measured by counting, {probes} probes")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Market {
    pub currency: String,
    pub total: u32,
    pub low: Money,
    pub high: Money,
    /// Fraction to price, in ascending order of fraction.
    pub percentiles: Vec<(f64, Money)>,
    /// Only available when the market was enumerated.
    pub mean: Option<Money>,
    /// Condition grade to count. Only populated when the market was enumerated.
    pub grades: Vec<(String, u32)>,
    pub method: Method,
    /// Listings kept for display, cheapest first.
    pub sample: Vec<Listing>,
    /// Price band to how many listings fall in it.
    pub histogram: Vec<(Money, Money, u32)>,
    /// Anything noticed while measuring that the reader should know about.
    pub warnings: Vec<String>,
}

impl Market {
    pub fn percentile(&self, fraction: f64) -> Option<Money> {
        self.percentiles
            .iter()
            .find(|(wanted, _)| (wanted - fraction).abs() < f64::EPSILON)
            .map(|(_, price)| *price)
    }

    pub fn median(&self) -> Option<Money> {
        self.percentile(0.50)
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// Measures the market for `search`, reporting the standard set of percentiles.
pub fn measure(client: &Client, search: &Search) -> Result<Market> {
    measure_at(client, search, &REPORT_PERCENTILES)
}

/// Measures the market for `search` at the given percentiles.
pub fn measure_at(client: &Client, search: &Search, percentiles: &[f64]) -> Result<Market> {
    let currency = client.currency().to_string();
    let total = client.count(search)?;

    if total == 0 {
        return Ok(Market {
            currency,
            total: 0,
            low: Money::ZERO,
            high: Money::ZERO,
            percentiles: percentiles
                .iter()
                .map(|fraction| (*fraction, Money::ZERO))
                .collect(),
            mean: None,
            grades: Vec::new(),
            method: Method::Enumerated { listings: 0 },
            sample: Vec::new(),
            histogram: Vec::new(),
            warnings: Vec::new(),
        });
    }

    if total <= ENUMERATION_LIMIT {
        enumerate(client, search, total, currency, percentiles)
    } else {
        count(client, search, total, currency, percentiles)
    }
}

/// Reads every listing and computes the distribution directly.
fn enumerate(
    client: &Client,
    search: &Search,
    total: u32,
    currency: String,
    wanted: &[f64],
) -> Result<Market> {
    let ordered = Search {
        sort: Sort::PriceAscending,
        ..search.clone()
    };
    let pages = total.div_ceil(MAX_PER_PAGE).min(quantile_page_cap());
    let fetched: Vec<Listing> = (1..=pages)
        .into_par_iter()
        .map(|page| {
            client
                .listings(&ordered, page, MAX_PER_PAGE)
                .map(|result| result.listings)
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();

    let mut prices: Vec<Money> = fetched.iter().map(Listing::amount).collect();
    prices.sort_unstable();
    let read = prices.len() as u32;

    let percentiles: Vec<(f64, Money)> = wanted
        .iter()
        .map(|fraction| (*fraction, nearest_rank(&prices, *fraction)))
        .collect();
    let histogram = even_histogram(&prices, &percentiles);
    let mean = (!prices.is_empty()).then(|| {
        let sum: i128 = prices.iter().map(|price| i128::from(price.minor)).sum();
        Money::from_minor((sum / i128::from(read.max(1))) as i64)
    });

    let mut grades: Vec<(String, u32)> = Vec::new();
    for listing in &fetched {
        let grade = listing.condition.display_name.clone();
        match grades.iter_mut().find(|(name, _)| *name == grade) {
            Some((_, count)) => *count += 1,
            None => grades.push((grade, 1)),
        }
    }
    grades.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    let low = prices.first().copied().unwrap_or(Money::ZERO);
    let high = prices.last().copied().unwrap_or(Money::ZERO);
    let warnings = unfiltered_warning(&fetched, search.condition)
        .into_iter()
        .collect();
    let sample = fetched
        .into_iter()
        .take(SAMPLE_SIZE as usize)
        .collect::<Vec<_>>();

    Ok(Market {
        currency: currency.clone(),
        total,
        low,
        high,
        percentiles,
        mean,
        grades,
        method: Method::Enumerated { listings: read },
        sample,
        histogram,
        warnings,
    })
}

/// Measures the distribution by counting, for markets past the paging cap.
fn count(
    client: &Client,
    search: &Search,
    total: u32,
    currency: String,
    wanted: &[f64],
) -> Result<Market> {
    // The bracket comes from the market itself: one listing from each end, which is both
    // exact and two requests. Searching for the bounds would cost far more.
    let cheapest = edge_listing(client, search, Sort::PriceAscending)?;
    let dearest = edge_listing(client, search, Sort::PriceDescending)?;
    let low = cheapest
        .as_ref()
        .map(Listing::amount)
        .unwrap_or(Money::ZERO);
    let high = dearest.as_ref().map(Listing::amount).unwrap_or(Money::ZERO);

    let before = client.usage().requests;
    let sketch = {
        let probe = |price: quantile::MajorUnits| -> Result<u32> {
            client.count(&search.between(None, Some(Money::from_major(price as f64, &currency))))
        };
        quantile::sketch(
            &probe,
            total,
            low.major_floor(&currency),
            high.major_ceil(&currency),
            wanted,
        )?
    };
    let probes = client.usage().requests.saturating_sub(before);

    let percentiles = sketch
        .quantiles
        .iter()
        .map(|(fraction, price)| (*fraction, Money::from_major(*price as f64, &currency)))
        .collect();

    let sample = client
        .listings(
            &Search {
                sort: Sort::PriceAscending,
                ..search.clone()
            },
            1,
            SAMPLE_SIZE,
        )?
        .listings;

    // Clipped to the middle ninety per cent. One vintage outlier at ten times the going
    // rate would otherwise stretch the range so far that every real listing lands in the
    // first band and the shape of the market disappears.
    let inner_low = sketch.quantiles.first().map(|(_, price)| *price);
    let inner_high = sketch.quantiles.last().map(|(_, price)| *price);
    let histogram = match (inner_low, inner_high) {
        (Some(low), Some(high)) => sketch
            .histogram_between(low, high, HISTOGRAM_BANDS)
            .into_iter()
            .map(|(from, to, count)| {
                (
                    Money::from_major(from as f64, &currency),
                    Money::from_major(to as f64, &currency),
                    count,
                )
            })
            .collect(),
        _ => Vec::new(),
    };

    Ok(Market {
        currency,
        total,
        low,
        high,
        percentiles,
        // Counting cannot produce a mean: it never sees the individual prices, and a mean
        // inferred from the curve would be a guess dressed as a statistic.
        mean: None,
        grades: Vec::new(),
        method: Method::Counted { probes },
        warnings: unfiltered_warning(&sample, search.condition)
            .into_iter()
            .collect(),
        sample,
        histogram,
    })
}

/// Checks that the server actually applied the condition that was asked for.
///
/// Reverb ignores a filter value it does not recognise and answers 200 with the full
/// unfiltered set. The condition vocabulary in [`Condition`] is closed precisely so that
/// cannot happen — this is the belt to that braces, and it would catch Reverb changing
/// the vocabulary under a released binary.
fn unfiltered_warning(listings: &[Listing], condition: Condition) -> Option<String> {
    let stray = listings
        .iter()
        .filter(|listing| !condition.admits(&listing.condition.slug))
        .count();
    (stray > 0).then(|| {
        format!(
            "Reverb returned {stray} of {} sampled listings outside the requested condition \
             ({condition}) — it may have stopped honouring that filter, so treat these \
             numbers as covering more than {condition} gear",
            listings.len()
        )
    })
}

fn edge_listing(client: &Client, search: &Search, sort: Sort) -> Result<Option<Listing>> {
    let ordered = Search {
        sort,
        ..search.clone()
    };
    Ok(client.listings(&ordered, 1, 1)?.listings.into_iter().next())
}

/// The value at the nearest-rank position of `fraction`, matching how the counting path
/// defines a percentile so the two agree.
fn nearest_rank(sorted: &[Money], fraction: f64) -> Money {
    if sorted.is_empty() {
        return Money::ZERO;
    }
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Equal-width bands across the middle of the range.
///
/// Clipped to the first and last percentiles reported — the middle ninety per cent — for
/// the same reason the counting path clips: a single outlier ten times the going rate
/// would put every real listing in one band and hide the shape entirely.
fn even_histogram(sorted: &[Money], percentiles: &[(f64, Money)]) -> Vec<(Money, Money, u32)> {
    let (Some((_, low)), Some((_, high))) = (percentiles.first(), percentiles.last()) else {
        return Vec::new();
    };
    if sorted.is_empty() {
        return Vec::new();
    }
    if low >= high {
        return vec![(*low, *high, sorted.len() as u32)];
    }
    let span = high.minor - low.minor;
    let mut bands = Vec::with_capacity(HISTOGRAM_BANDS);
    for index in 0..HISTOGRAM_BANDS {
        let from = Money::from_minor(low.minor + span * index as i64 / HISTOGRAM_BANDS as i64);
        let to = Money::from_minor(low.minor + span * (index as i64 + 1) / HISTOGRAM_BANDS as i64);
        let last = index == HISTOGRAM_BANDS - 1;
        let count = sorted
            .iter()
            .filter(|price| **price >= from && (**price < to || (last && **price <= to)))
            .count() as u32;
        bands.push((from, to, count));
    }
    bands
}

/// Reverb stops paging here, so enumeration can never ask for more than this many pages.
fn quantile_page_cap() -> u32 {
    MAX_ENUMERABLE / MAX_PER_PAGE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(major: i64) -> Money {
        Money::from_minor(major * 100)
    }

    #[test]
    fn nearest_rank_agrees_with_the_counting_definition() {
        let sorted: Vec<Money> = (1..=100).map(money).collect();
        assert_eq!(money(1), nearest_rank(&sorted, 0.0));
        assert_eq!(money(25), nearest_rank(&sorted, 0.25));
        assert_eq!(money(50), nearest_rank(&sorted, 0.50));
        assert_eq!(money(100), nearest_rank(&sorted, 1.0));
        assert_eq!(Money::ZERO, nearest_rank(&[], 0.5));
    }

    #[test]
    fn the_histogram_accounts_for_every_listing_inside_its_range() {
        let sorted: Vec<Money> = (1..=100).map(money).collect();
        let bands = even_histogram(&sorted, &[(0.05, money(5)), (0.95, money(95))]);
        assert_eq!(8, bands.len());
        // Contiguous, and every listing between the two ends is counted exactly once.
        for pair in bands.windows(2) {
            assert_eq!(pair[0].1, pair[1].0);
        }
        assert_eq!(91, bands.iter().map(|(_, _, count)| count).sum::<u32>());
    }

    #[test]
    fn one_wild_outlier_no_longer_swallows_the_whole_histogram() {
        // Ninety-nine listings around two thousand, and one collector piece at fifty.
        let mut sorted: Vec<Money> = (0..99).map(|index| money(1_900 + index)).collect();
        sorted.push(money(50_000));
        sorted.sort_unstable();
        let bands = even_histogram(&sorted, &[(0.05, money(1_904)), (0.95, money(1_994))]);
        // The bars describe where listings actually are rather than one empty decade.
        let occupied = bands.iter().filter(|(_, _, count)| *count > 0).count();
        assert!(
            occupied >= 7,
            "only {occupied} of {} bands hold anything",
            bands.len()
        );
    }

    #[test]
    fn a_market_where_every_listing_costs_the_same_is_one_band() {
        let sorted = vec![money(1_999); 12];
        let bands = even_histogram(&sorted, &[(0.05, money(1_999)), (0.95, money(1_999))]);
        assert_eq!(vec![(money(1_999), money(1_999), 12)], bands);
    }

    #[test]
    fn an_empty_market_has_no_histogram() {
        assert!(even_histogram(&[], &[(0.05, money(1)), (0.95, money(2))]).is_empty());
        assert!(even_histogram(&[money(1)], &[]).is_empty());
    }
}
