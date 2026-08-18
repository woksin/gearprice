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
//!
//! # What is still on the shelf
//!
//! Reverb retired its sold-price endpoint, so there is no record of what anything went
//! for. There is, however, the date every live listing went up — and a live market is a
//! survivorship sample. Listings priced where the market clears leave, because they sell;
//! listings priced above it stay, and pile up. So the age of what is still for sale is
//! evidence about which asking prices are being paid, from the opposite direction:
//!
//! ```text
//! Fender American Professional II Stratocaster, used
//!   cheapest fifth   $900-$1,300     median 28 days listed
//!   dearest fifth    $1,640          median 1,985 days listed
//! ```
//!
//! Those $1,640 listings have been up for five and a half years. A percentile can say
//! that $1,640 is the ninetieth; only the dates can say that nobody is paying it.
//!
//! It is an inference and it is reported as one. A dealer's standing inventory listing
//! ages without meaning anything, and a relisted item resets its own clock. The signal
//! survives both because it is a median over a band, not a claim about any one listing.

use anyhow::Result;
use chrono::{DateTime, Utc};

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

/// How many price windows to sample a counted market across.
///
/// Reading only the cheapest page would describe the cheapest page. Ages and condition
/// vary most between the bottom of a market and the top, which is exactly what a single
/// sorted page cannot see.
const SAMPLE_WINDOWS: usize = 5;

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

/// What one condition grade costs on this market.
#[derive(Clone, Debug)]
pub struct GradePrice {
    pub grade: String,
    pub listings: u32,
    pub median: Money,
}

/// Reverb's condition grades, best first.
///
/// Ordering the table this way rather than by popularity is what lets a reader see
/// whether condition actually tracks price on a given model. Often it does not: on the
/// American Professional II Stratocaster the `Good` examples ask more than the `Mint`
/// ones. Sorted by how many listings each grade has, that reads as an arbitrary list;
/// sorted by grade, it reads as the finding it is.
const GRADE_ORDER: [&str; 10] = [
    "brand-new",
    "mint-inventory",
    "mint",
    "b-stock",
    "excellent",
    "very-good",
    "good",
    "fair",
    "poor",
    "non-functioning",
];

/// What to call a grade in the table.
///
/// Reverb reports `mint` and `mint-inventory` — a private seller's mint example and a
/// dealer's unsold new-old stock — under the same display name, which puts two rows
/// called "Mint" next to each other and makes the ladder unreadable.
fn grade_name(slug: &str, display: String) -> String {
    match slug {
        "mint-inventory" => "Mint (dealer stock)".to_string(),
        _ if display.is_empty() => slug.to_string(),
        _ => display,
    }
}

/// The rank of a grade named however Reverb spelled it — slug or display name.
pub fn grade_rank_of(name: &str) -> usize {
    grade_rank(&name.to_lowercase().replace(' ', "-"))
}

fn grade_rank(slug: &str) -> usize {
    GRADE_ORDER
        .iter()
        .position(|known| *known == slug)
        .unwrap_or(GRADE_ORDER.len())
}

/// One live listing, reduced to what the report reasons about.
#[derive(Clone, Copy, Debug)]
pub struct Observation {
    pub price: Money,
    /// Days on the market, where Reverb gave a usable publication date.
    pub days_listed: Option<i64>,
    pub offers: bool,
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
    /// What each condition grade is going for, commonest grade first.
    pub grades: Vec<GradePrice>,
    pub method: Method,
    /// Listings kept for display, cheapest first.
    pub sample: Vec<Listing>,
    /// Every listing the run actually read, reduced. Complete when the market was
    /// enumerated; spread across the price range when it was counted.
    pub observations: Vec<Observation>,
    /// Whether [`Market::observations`] covers the whole market or a sample of it.
    pub observations_complete: bool,
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

    /// Median days on the market for listings priced within a band.
    ///
    /// `None` when too few listings in that band carry a usable date to say anything —
    /// a median of two listings is an anecdote.
    pub fn days_listed_between(&self, from: Option<Money>, to: Option<Money>) -> Option<i64> {
        const ENOUGH: usize = 4;
        let mut ages: Vec<i64> = self
            .observations
            .iter()
            .filter(|observation| from.is_none_or(|from| observation.price >= from))
            .filter(|observation| to.is_none_or(|to| observation.price < to))
            .filter_map(|observation| observation.days_listed)
            .collect();
        if ages.len() < ENOUGH {
            return None;
        }
        ages.sort_unstable();
        Some(ages[ages.len() / 2])
    }

    /// The share of sellers who will take an offer, so a reader knows how firm these
    /// asking prices are.
    pub fn offers_share(&self) -> Option<f64> {
        (!self.observations.is_empty()).then(|| {
            let accepting = self
                .observations
                .iter()
                .filter(|observation| observation.offers)
                .count();
            accepting as f64 / self.observations.len() as f64
        })
    }
}

/// The median asking price for each condition grade present.
///
/// Between a mint example and a beaten one lies the whole reason two listings for the
/// same model differ in price, and a single band across every grade hides it.
fn grade_prices(listings: &[Listing]) -> Vec<GradePrice> {
    let mut by_grade: Vec<(String, String, Vec<Money>)> = Vec::new();
    for listing in listings {
        let slug = listing.condition.slug.clone();
        match by_grade.iter_mut().find(|(known, _, _)| *known == slug) {
            Some((_, _, prices)) => prices.push(listing.amount()),
            None => by_grade.push((
                slug,
                listing.condition.display_name.clone(),
                vec![listing.amount()],
            )),
        }
    }
    let mut grades: Vec<(usize, GradePrice)> = by_grade
        .into_iter()
        .map(|(slug, display, mut prices)| {
            prices.sort_unstable();
            (
                grade_rank(&slug),
                GradePrice {
                    listings: prices.len() as u32,
                    median: prices[prices.len() / 2],
                    grade: grade_name(&slug, display),
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

fn observe(listings: &[Listing], now: DateTime<Utc>) -> Vec<Observation> {
    listings
        .iter()
        .map(|listing| Observation {
            price: listing.amount(),
            days_listed: listing.days_listed(now),
            offers: listing.offers_enabled,
        })
        .collect()
}

/// Below this many listings, a pinned search is thin enough to be worth widening.
///
/// Pinning to a catalogue model is right for common gear — a text search for `Boss DS-1`
/// returns the pedal, a T-shirt, a knob set and a page of service notes, and the junk
/// lands in the cheap band. It is wrong for rare gear, because sellers of rare things
/// often do not attach their listing to the catalogue entry at all. Measured on a Marshall
/// Major: the pin found six listings and a widened search found nine.
pub const WIDEN_BELOW: u32 = 25;

/// Builds a market from a set of listings already in hand.
///
/// Used when the listings came from more than one search and had to be merged, which the
/// counting path cannot express as a single query.
pub fn from_listings(listings: Vec<Listing>, currency: String, condition: Condition) -> Market {
    let total = listings.len() as u32;
    let mut prices: Vec<Money> = listings.iter().map(Listing::amount).collect();
    prices.sort_unstable();
    let percentiles: Vec<(f64, Money)> = REPORT_PERCENTILES
        .iter()
        .map(|fraction| (*fraction, nearest_rank(&prices, *fraction)))
        .collect();
    let mean = (!prices.is_empty()).then(|| {
        let sum: i128 = prices.iter().map(|price| i128::from(price.minor)).sum();
        Money::from_minor((sum / i128::from(total.max(1))) as i64)
    });
    let histogram = even_histogram(&prices, &percentiles);
    let observations = observe(&listings, Utc::now());
    let warnings = unfiltered_warning(&listings, condition)
        .into_iter()
        .collect();
    let mut sample = listings;
    sample.sort_by_key(Listing::amount);
    Market {
        low: prices.first().copied().unwrap_or(Money::ZERO),
        high: prices.last().copied().unwrap_or(Money::ZERO),
        grades: grade_prices(&sample),
        currency,
        total,
        percentiles,
        mean,
        method: Method::Enumerated { listings: total },
        observations,
        observations_complete: true,
        sample,
        histogram,
        warnings,
    }
}

/// Measures the market for `search`, reporting the standard set of percentiles.
pub fn measure(client: &Client, search: &Search) -> Result<Market> {
    measure_at(client, search, &REPORT_PERCENTILES)
}

/// Measures the market for `search` at the given percentiles.
pub fn measure_at(client: &Client, search: &Search, percentiles: &[f64]) -> Result<Market> {
    let currency = client.currency().to_string();
    // Both ends of the market at once, which also settles how many there are: a listing
    // page reports the total for its search, so counting separately would be a third
    // round trip for a number already in hand.
    let ends = crate::parallel::each(&[Sort::PriceAscending, Sort::PriceDescending], |sort| {
        edge_listing(client, search, *sort)
    })?;
    let (cheapest, total) = ends[0].clone();
    let dearest = ends[1].0.clone();

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
            observations: Vec::new(),
            observations_complete: true,
            histogram: Vec::new(),
            warnings: Vec::new(),
        });
    }

    if total <= ENUMERATION_LIMIT {
        enumerate(client, search, total, currency, percentiles)
    } else {
        count(
            client,
            search,
            total,
            currency,
            percentiles,
            cheapest,
            dearest,
        )
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
    let numbers: Vec<u32> = (1..=pages).collect();
    let fetched: Vec<Listing> = crate::parallel::each(&numbers, |page| {
        client
            .listings(&ordered, *page, MAX_PER_PAGE)
            .map(|result| result.listings)
    })?
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

    let grades = grade_prices(&fetched);

    let low = prices.first().copied().unwrap_or(Money::ZERO);
    let high = prices.last().copied().unwrap_or(Money::ZERO);
    let warnings = unfiltered_warning(&fetched, search.condition)
        .into_iter()
        .collect();
    let observations = observe(&fetched, Utc::now());
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
        observations,
        observations_complete: true,
        histogram,
        warnings,
    })
}

/// Measures the distribution by counting, for markets past the paging cap.
#[allow(clippy::too_many_arguments)]
fn count(
    client: &Client,
    search: &Search,
    total: u32,
    currency: String,
    wanted: &[f64],
    cheapest: Option<Listing>,
    dearest: Option<Listing>,
) -> Result<Market> {
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

    // Sampled across the price range rather than off the front. The cheapest fifty
    // listings describe the cheapest fifty listings, and every question worth asking of
    // them — how long has this been sitting, what condition is it — differs most between
    // the bottom of a market and the top.
    let edges: Vec<Money> = sketch
        .quantiles
        .iter()
        .map(|(_, price)| Money::from_major(*price as f64, &currency))
        .collect();
    let windows = sample_windows(low, high, &edges);
    let sampled: Vec<Vec<Listing>> = crate::parallel::each(&windows, |(from, to)| {
        client
            .listings(
                &Search {
                    sort: Sort::PriceAscending,
                    ..search.between(Some(*from), Some(*to))
                },
                1,
                SAMPLE_SIZE,
            )
            .map(|page| page.listings)
    })?;
    let spread: Vec<Listing> = sampled.into_iter().flatten().collect();
    let observations = observe(&spread, Utc::now());
    let mut sample = spread;
    sample.sort_by_key(Listing::amount);
    sample.truncate(SAMPLE_SIZE as usize);

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
        grades: grade_prices(&sample),
        method: Method::Counted { probes },
        warnings: unfiltered_warning(&sample, search.condition)
            .into_iter()
            .collect(),
        sample,
        observations,
        observations_complete: false,
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

/// Price windows to sample a counted market across, from the percentiles already known.
fn sample_windows(low: Money, high: Money, edges: &[Money]) -> Vec<(Money, Money)> {
    let mut cuts: Vec<Money> = vec![low];
    if !edges.is_empty() {
        // Spread the chosen cuts evenly through the percentiles rather than taking the
        // first few, so the windows span the market instead of crowding its cheap end.
        for step in 1..SAMPLE_WINDOWS {
            let index = step * edges.len() / SAMPLE_WINDOWS;
            cuts.push(edges[index.min(edges.len() - 1)]);
        }
    }
    cuts.push(high);
    cuts.dedup();
    cuts.windows(2).map(|pair| (pair[0], pair[1])).collect()
}

/// One listing from one end of a market, and how many there are altogether.
///
/// The count comes back with the listing rather than being asked for separately. Every
/// listing page carries the total for its search, so a count is a page of one that throws
/// the listing away — and on a large category that discarded request is a serial round
/// trip of well over a second, taken before anything else can start.
fn edge_listing(client: &Client, search: &Search, sort: Sort) -> Result<(Option<Listing>, u32)> {
    let ordered = Search {
        sort,
        ..search.clone()
    };
    let page = client.listings(&ordered, 1, 1)?;
    Ok((page.listings.into_iter().next(), page.total))
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

    fn listing_at(price: i64, grade: &str) -> Listing {
        let slug = grade.to_lowercase().replace(' ', "-");
        serde_json::from_str(&format!(
            "{{\"id\":1,\"condition\":{{\"slug\":\"{slug}\",\"display_name\":\"{grade}\"}},
              \"price\":{{\"amount_cents\":{price}}}}}"
        ))
        .unwrap()
    }

    #[test]
    fn each_condition_grade_reports_its_own_median() {
        let listings = vec![
            listing_at(260_000, "Mint"),
            listing_at(229_000, "Excellent"),
            listing_at(231_000, "Excellent"),
            listing_at(235_000, "Excellent"),
            listing_at(186_000, "Good"),
        ];
        let grades = grade_prices(&listings);
        // Best grade first, so a reader can see at a glance whether condition tracks price
        // on this model — which, often enough, it does not.
        assert_eq!(
            vec!["Mint", "Excellent", "Good"],
            grades.iter().map(|g| g.grade.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(3, grades[1].listings);
        assert_eq!(money(2_310), grades[1].median);
        assert_eq!(money(2_600), grades[0].median);
        assert!(grade_prices(&[]).is_empty());

        // Reverb calls a dealer's unsold stock "Mint" too; two rows of that name in one
        // ladder read as a bug.
        let both = vec![
            listing_at(260_000, "Mint"),
            listing_at(255_000, "Mint"),
            serde_json::from_str::<Listing>(
                "{\"id\":2,\"condition\":{\"slug\":\"mint-inventory\",\"display_name\":\"Mint\"},
                  \"price\":{\"amount_cents\":270000}}",
            )
            .unwrap(),
        ];
        let priced = grade_prices(&both);
        let named: Vec<&str> = priced.iter().map(|grade| grade.grade.as_str()).collect();
        assert_eq!(vec!["Mint (dealer stock)", "Mint"], named);
    }

    #[test]
    fn a_bands_typical_age_needs_enough_listings_to_be_a_median() {
        let observation = |price: i64, days: Option<i64>| Observation {
            price: money(price),
            days_listed: days,
            offers: false,
        };
        let mut market = empty_market();
        market.observations = vec![
            observation(1_000, Some(10)),
            observation(1_100, Some(20)),
            observation(1_200, Some(30)),
            observation(1_300, Some(400)),
            observation(9_000, Some(900)),
        ];
        // Four listings in the window, so a median is worth quoting. Even counts take the
        // upper middle, the same convention the percentiles use.
        assert_eq!(
            Some(30),
            market.days_listed_between(Some(money(1_000)), Some(money(2_000)))
        );
        // One is not.
        assert_eq!(None, market.days_listed_between(Some(money(5_000)), None));
        // Listings with no date are left out rather than counted as new.
        market.observations.push(observation(1_150, None));
        assert_eq!(
            Some(30),
            market.days_listed_between(Some(money(1_000)), Some(money(2_000)))
        );
    }

    #[test]
    fn the_share_taking_offers_is_reported_only_when_something_was_read() {
        let mut market = empty_market();
        assert_eq!(None, market.offers_share());
        market.observations = vec![
            Observation {
                price: money(1),
                days_listed: None,
                offers: true,
            },
            Observation {
                price: money(2),
                days_listed: None,
                offers: true,
            },
            Observation {
                price: money(3),
                days_listed: None,
                offers: false,
            },
            Observation {
                price: money(4),
                days_listed: None,
                offers: false,
            },
        ];
        assert_eq!(Some(0.5), market.offers_share());
    }

    fn empty_market() -> Market {
        Market {
            currency: "USD".into(),
            total: 0,
            low: Money::ZERO,
            high: Money::ZERO,
            percentiles: Vec::new(),
            mean: None,
            grades: Vec::new(),
            method: Method::Counted { probes: 0 },
            sample: Vec::new(),
            observations: Vec::new(),
            observations_complete: false,
            histogram: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn sampling_windows_span_the_market_rather_than_crowding_its_cheap_end() {
        let edges: Vec<Money> = [1_600, 1_750, 2_090, 2_275, 2_450, 2_880, 3_200]
            .iter()
            .map(|major| money(*major))
            .collect();
        let windows = sample_windows(money(1_500), money(4_900), &edges);
        assert_eq!(SAMPLE_WINDOWS, windows.len());
        // Contiguous, and covering the whole range from cheapest to dearest.
        assert_eq!(money(1_500), windows[0].0);
        assert_eq!(money(4_900), windows[windows.len() - 1].1);
        for pair in windows.windows(2) {
            assert_eq!(pair[0].1, pair[1].0);
        }
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
