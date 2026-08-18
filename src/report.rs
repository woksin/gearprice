//! The canonical shape of everything gearprice reports.
//!
//! Table, JSON and CSV are three renderings of one structure rather than three
//! independent formatters, so a field can never appear in one and quietly go missing
//! from another. Reverb's own response types stop here: what leaves the tool is a schema
//! gearprice owns and can keep stable.

use serde::Serialize;

use crate::classify::{Band, Bands, Basis, Placement, Segment};
use crate::history::{Movement, Snapshot};
use crate::market::Market;
use crate::money::Money;
use crate::reverb::{CatalogueModel, Listing, Usage};
use crate::sold::Sold;

/// Said plainly and attached to every report: these are prices people are *asking*, not
/// prices anything *sold* for, and asking runs 16–46% above sold depending on the model.
///
/// Reverb's Price Guide endpoint is retired, but sold history is still public at
/// `/api/comparison_shopping_pages/{id}/transactions`. Reporting sold as the primary
/// number is the next piece of work — see `docs/PLAN.md`. Until it lands this caveat has
/// to carry the whole weight of the difference.
pub const SOURCE: &str = "live Reverb listings — asking prices, not sold prices";

/// The same caveat, short enough for a table header.
pub const SOURCE_SHORT: &str = "asking prices, not sold prices";

#[derive(Debug, Serialize)]
pub struct PriceReport {
    pub query: String,
    pub source: &'static str,
    pub generated_at: String,
    pub currency: String,
    pub condition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSummary>,
    /// What the query was corrected to, when correcting it is what found the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interpreted_as: Option<String>,
    /// Models that fit the query about as well as the one priced.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<AlternativeModel>,
    pub market: MarketSummary,
    /// What the model has actually sold for, where Reverb has a record of it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sold: Option<SoldSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bands: Option<BandTable>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<ClassSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asking: Option<AskingVerdict>,
    /// The destination the market was narrowed to, when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_to: Option<String>,
    pub listings: Vec<ListingSummary>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Serialize)]
pub struct ModelSummary {
    pub id: u64,
    pub title: String,
    pub brand: String,
    pub category: String,
    pub url: String,
    pub used_listings: u32,
    pub new_listings: u32,
    /// How many catalogue products the model covers — the finishes and runs a search is
    /// pinned to.
    pub products: usize,
}

impl ModelSummary {
    pub fn of(model: &CatalogueModel) -> Self {
        Self {
            id: model.id,
            title: model.title.clone(),
            brand: model.brand_name().to_string(),
            category: model.root_category_slug.clone(),
            url: model.web_url(),
            used_listings: model.used_total,
            new_listings: model.new_total,
            products: model.product_ids().len(),
        }
    }
}

/// A model the query fits about as well as the one that was priced.
#[derive(Debug, Serialize)]
pub struct AlternativeModel {
    pub id: u64,
    pub title: String,
    pub used_listings: u32,
    pub new_listings: u32,
}

impl AlternativeModel {
    pub fn of(model: &CatalogueModel) -> Self {
        Self {
            id: model.id,
            title: model.title.clone(),
            used_listings: model.used_total,
            new_listings: model.new_total,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct MarketSummary {
    pub listings: u32,
    pub method: String,
    pub cheapest: f64,
    pub dearest: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    pub percentiles: Vec<PercentilePoint>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub grades: Vec<GradeCount>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub histogram: Vec<HistogramBand>,
    /// Share of sellers who accept offers, so a reader knows how firm these prices are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sellers_taking_offers: Option<f64>,
    /// How many listings the ages and condition figures were read from, and whether that
    /// is the whole market or a sample across it.
    pub listings_read: usize,
    pub listings_read_is_complete: bool,
}

impl MarketSummary {
    pub fn of(market: &Market) -> Self {
        let currency = market.currency.as_str();
        Self {
            listings: market.total,
            method: market.method.label(),
            cheapest: market.low.major(currency),
            dearest: market.high.major(currency),
            mean: market.mean.map(|value| value.major(currency)),
            percentiles: market
                .percentiles
                .iter()
                .map(|(fraction, price)| PercentilePoint {
                    percentile: (fraction * 100.0).round() as u32,
                    price: price.major(currency),
                })
                .collect(),
            grades: market
                .grades
                .iter()
                .map(|grade| GradeCount {
                    grade: grade.grade.clone(),
                    listings: grade.listings,
                    median: grade.median.major(currency),
                    against_median: market
                        .median()
                        .map(|overall| grade.median.major(currency) - overall.major(currency)),
                })
                .collect(),
            histogram: market
                .histogram
                .iter()
                .map(|(from, to, count)| HistogramBand {
                    from: from.major(currency),
                    to: to.major(currency),
                    listings: *count,
                })
                .collect(),
            sellers_taking_offers: market.offers_share(),
            listings_read: market.observations.len(),
            listings_read_is_complete: market.observations_complete,
        }
    }
}

/// What people paid, as against what sellers want.
#[derive(Debug, Serialize)]
pub struct SoldSummary {
    /// Sales Reverb has on record for this model.
    pub recorded: u32,
    /// How many of them this report read.
    pub read: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covering: Option<String>,
    pub percentiles: Vec<PercentilePoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median: Option<f64>,
    /// The median of the last year, where the record runs back far enough that the
    /// overall median describes an older market.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_median: Option<f64>,
    pub recent_sales: usize,
    /// How far the asking median sits above the sold median, as a fraction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asking_premium: Option<f64>,
    /// Where the asking median falls among prices people actually paid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asking_percentile: Option<u32>,
    /// Median of what buyers got off the asking price. Negative is a discount.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typical_discount: Option<f64>,
    pub sales_with_both_prices: usize,
    pub sold_at_or_above_ask: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub grades: Vec<GradeCount>,
}

impl SoldSummary {
    pub fn of(sold: &Sold, market: &Market) -> Self {
        let currency = sold.currency.as_str();
        let median = sold.median().map(|price| price.major(currency));
        let asking = market.median().map(|price| price.major(currency));
        Self {
            recorded: sold.recorded,
            read: sold.sales.len(),
            covering: sold
                .covering
                .as_ref()
                .map(|(oldest, newest)| format!("{oldest} to {newest}")),
            percentiles: sold
                .percentiles
                .iter()
                .map(|(fraction, price)| PercentilePoint {
                    percentile: (fraction * 100.0).round() as u32,
                    price: price.major(currency),
                })
                .collect(),
            median,
            recent_median: sold.recent_median.map(|price| price.major(currency)),
            recent_sales: sold.recent_sales,
            asking_premium: median
                .zip(asking)
                .filter(|(sold, _)| *sold > 0.0)
                .map(|(sold, asking)| (asking - sold) / sold),
            asking_percentile: asking.map(|asking| {
                let below = sold
                    .sales
                    .iter()
                    .filter(|sale| sale.paid.major(currency) <= asking)
                    .count();
                if sold.sales.is_empty() {
                    0
                } else {
                    ((below as f64 / sold.sales.len() as f64) * 100.0).round() as u32
                }
            }),
            typical_discount: sold.typical_discount,
            sales_with_both_prices: sold.discounts_seen,
            sold_at_or_above_ask: sold.at_or_above_ask,
            grades: sold
                .grades
                .iter()
                .map(|grade| GradeCount {
                    grade: grade.grade.clone(),
                    listings: grade.sales,
                    median: grade.median.major(currency),
                    against_median: median.map(|overall| grade.median.major(currency) - overall),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PercentilePoint {
    pub percentile: u32,
    pub price: f64,
}

#[derive(Debug, Serialize)]
pub struct GradeCount {
    pub grade: String,
    pub listings: u32,
    /// The median asking price for this grade.
    pub median: f64,
    /// How that compares to the market's median across all grades.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub against_median: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct HistogramBand {
    pub from: f64,
    pub to: f64,
    pub listings: u32,
}

#[derive(Debug, Serialize)]
pub struct BandTable {
    /// Whether these bands were cut from what people paid or what sellers ask.
    pub basis: Basis,
    pub bands: Vec<BandRow>,
}

impl BandTable {
    pub fn of(bands: &Bands, market: &Market) -> Self {
        let currency = market.currency.as_str();
        Self {
            basis: bands.basis,
            bands: Band::ALL
                .into_iter()
                .map(|band| {
                    let (from, to) = bands.range(band);
                    let (low, high) = band.bounds();
                    BandRow {
                        band: band.name().to_string(),
                        from: from.map(|price| price.major(currency)),
                        to: to.map(|price| price.major(currency)),
                        share: high - low,
                        // The asking price at the same percentile, so a reader holding a
                        // listing can see which sold band it corresponds to.
                        asking_from: (bands.basis == Basis::Sold)
                            .then(|| market.percentile(low))
                            .flatten()
                            .map(|price| price.major(currency)),
                        asking_to: (bands.basis == Basis::Sold)
                            .then(|| market.percentile(high))
                            .flatten()
                            .map(|price| price.major(currency)),
                        days_listed: market.days_listed_between(from, to),
                        verdict: band.verdict(bands.basis).to_string(),
                    }
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BandRow {
    pub band: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
    pub share: f64,
    /// The asking price at the same percentile, when the bands were cut from sold prices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asking_from: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asking_to: Option<f64>,
    /// Median days the listings in this band have been on the market. The signal that
    /// separates a price people pay from a price people ignore.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days_listed: Option<i64>,
    pub verdict: String,
}

#[derive(Debug, Serialize)]
pub struct ClassSummary {
    pub segment: Segment,
    pub description: String,
    pub percentile: u32,
    pub peer_group: String,
    pub peer_listings: u32,
}

impl ClassSummary {
    pub fn of(placement: &Placement) -> Self {
        Self {
            segment: placement.segment,
            description: placement.segment.description().to_string(),
            percentile: (placement.percentile * 100.0).round() as u32,
            peer_group: placement.peer_group.clone(),
            peer_listings: placement.peer_total,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AskingVerdict {
    pub price: f64,
    pub band: Band,
    /// Whether the verdict is against what people paid or what sellers ask.
    pub basis: Basis,
    pub verdict: String,
    /// Difference from the median: negative is below it.
    pub against_median: f64,
}

#[derive(Debug, Serialize)]
pub struct ListingSummary {
    pub id: u64,
    pub price: f64,
    /// What the seller charges to send it to the chosen destination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shipping: Option<f64>,
    /// Price plus shipping. Absent when the carrier prices it at checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered: Option<f64>,
    pub condition: String,
    pub title: String,
    pub shop: String,
    pub year: String,
    /// How long this one has been on the market. Shown per listing where a market is too
    /// thin for a per-band median to mean anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days_listed: Option<i64>,
    /// Auctions carry a current bid rather than an asking price.
    pub auction: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band: Option<Band>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl ListingSummary {
    pub fn of(
        listing: &Listing,
        currency: &str,
        bands: Option<&Bands>,
        destination: Option<&[String]>,
        model_title: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        let cost = destination.map(|chain| crate::shipping::cost(listing, chain));
        Self {
            id: listing.id,
            price: listing.amount().major(currency),
            shipping: cost
                .and_then(crate::shipping::Cost::amount)
                .map(|amount| amount.major(currency)),
            delivered: destination
                .and_then(|chain| crate::shipping::landed(listing, chain))
                .map(|amount| amount.major(currency)),
            condition: listing.condition.display_name.clone(),
            title: listing.describe(),
            shop: listing.shop_name.clone(),
            // Not Reverb's year field taken at face value: on some models it holds the
            // model number, and sellers paste production ranges into titles.
            year: crate::years::stated(&listing.title, &listing.year, model_title)
                .map(|(year, _)| year.to_string())
                .unwrap_or_default(),
            days_listed: listing.days_listed(now),
            auction: listing.auction,
            band: bands.map(|bands| bands.band_for(listing.amount())),
            url: listing.web_url().map(str::to_string),
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Diagnostics {
    pub requests: u32,
    pub cache_hits: u32,
    pub warnings: Vec<String>,
}

impl Diagnostics {
    pub fn from(usage: Usage, warnings: Vec<String>) -> Self {
        Self {
            requests: usage.requests,
            cache_hits: usage.cache_hits,
            warnings,
        }
    }
}

/// Models matching a search, for disambiguation.
#[derive(Debug, Serialize)]
pub struct ModelReport {
    pub query: String,
    pub source: &'static str,
    pub generated_at: String,
    pub currency: String,
    pub models: Vec<ModelRow>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Serialize)]
pub struct ModelRow {
    pub id: u64,
    pub title: String,
    pub brand: String,
    pub category: String,
    pub used_listings: u32,
    pub new_listings: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_from: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_from: Option<f64>,
    pub url: String,
}

impl ModelRow {
    pub fn of(model: &CatalogueModel, currency: &str) -> Self {
        let price = |value: &Option<crate::reverb::Price>| {
            value
                .as_ref()
                .map(|price| Money::from_minor(price.amount_cents).major(currency))
        };
        Self {
            id: model.id,
            title: model.title.clone(),
            brand: model.brand_name().to_string(),
            category: model.root_category_slug.clone(),
            used_listings: model.used_total,
            new_listings: model.new_total,
            used_from: price(&model.used_low_price),
            new_from: price(&model.new_low_price),
            url: model.web_url(),
        }
    }
}

/// The price classes of a whole category, in money.
#[derive(Debug, Serialize)]
pub struct ClassReport {
    pub category: String,
    pub source: &'static str,
    pub generated_at: String,
    pub currency: String,
    pub condition: String,
    pub listings: u32,
    pub method: String,
    pub classes: Vec<ClassRow>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Serialize)]
pub struct ClassRow {
    pub segment: Segment,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
    pub share: f64,
    pub description: String,
}

/// What a market has done since it was first recorded.
#[derive(Debug, Serialize)]
pub struct TrackReport {
    pub query: String,
    pub source: &'static str,
    pub generated_at: String,
    pub currency: String,
    pub condition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSummary>,
    pub recorded: bool,
    pub readings: Vec<Reading>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movement: Option<Movement>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Serialize)]
pub struct Reading {
    pub recorded_at: String,
    pub listings: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median: Option<f64>,
    /// What it was selling for at the time, where that was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_sold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_days_listed: Option<i64>,
}

impl Reading {
    pub fn of(snapshot: &Snapshot) -> Self {
        Self {
            recorded_at: snapshot
                .recorded_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            listings: snapshot.listings,
            median: snapshot.median(),
            median_sold: snapshot.median_sold,
            median_days_listed: snapshot.median_days_listed,
        }
    }
}

/// Everything the history knows about, for `track --list`.
#[derive(Debug, Serialize)]
pub struct TrackedReport {
    pub history: String,
    pub tracked: Vec<TrackedSubject>,
}

#[derive(Debug, Serialize)]
pub struct TrackedSubject {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<u64>,
    pub currency: String,
    pub condition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ships_to: Option<String>,
    pub readings: usize,
    pub last_recorded: String,
}

/// Live listings, band-labelled.
#[derive(Debug, Serialize)]
pub struct ListingReport {
    pub query: String,
    pub source: &'static str,
    pub generated_at: String,
    pub currency: String,
    pub condition: String,
    pub matched: u32,
    pub shown: usize,
    /// The destination prices are quoted delivered to, when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bands: Option<BandTable>,
    pub listings: Vec<ListingSummary>,
    pub diagnostics: Diagnostics,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::Method;

    fn market() -> Market {
        Market {
            currency: "USD".into(),
            total: 174,
            low: Money::from_minor(175_000),
            high: Money::from_minor(490_000),
            percentiles: vec![
                (0.05, Money::from_minor(160_000)),
                (0.10, Money::from_minor(175_000)),
                (0.25, Money::from_minor(209_000)),
                (0.50, Money::from_minor(227_500)),
                (0.75, Money::from_minor(245_000)),
                (0.90, Money::from_minor(288_000)),
                (0.95, Money::from_minor(320_000)),
            ],
            mean: Some(Money::from_minor(231_000)),
            grades: vec![
                crate::market::GradePrice {
                    grade: "Excellent".into(),
                    listings: 68,
                    median: Money::from_minor(229_000),
                },
                crate::market::GradePrice {
                    grade: "Very Good".into(),
                    listings: 40,
                    median: Money::from_minor(210_000),
                },
            ],
            method: Method::Enumerated { listings: 174 },
            sample: Vec::new(),
            observations: Vec::new(),
            observations_complete: true,
            histogram: vec![(Money::from_minor(175_000), Money::from_minor(200_000), 30)],
            warnings: Vec::new(),
        }
    }

    #[test]
    fn a_market_summary_carries_every_percentile_as_whole_units() {
        let summary = MarketSummary::of(&market());
        assert_eq!(174, summary.listings);
        assert_eq!(1_750.0, summary.cheapest);
        assert_eq!(Some(2_310.0), summary.mean);
        assert_eq!(7, summary.percentiles.len());
        assert_eq!(5, summary.percentiles[0].percentile);
        assert_eq!(2_275.0, summary.percentiles[3].price);
        assert_eq!(50, summary.percentiles[3].percentile);
    }

    #[test]
    fn band_rows_shade_the_whole_line_and_shares_add_to_one() {
        let market = market();
        let bands = Bands::of(&market).unwrap();
        let table = BandTable::of(&bands, &market);
        assert_eq!(5, table.bands.len());
        assert_eq!(None, table.bands[0].from);
        assert_eq!(None, table.bands[4].to);
        let share: f64 = table.bands.iter().map(|row| row.share).sum();
        assert!((share - 1.0).abs() < 1e-9, "shares summed to {share}");
    }

    #[test]
    fn the_json_schema_keeps_absent_values_out_rather_than_emitting_null() {
        let report = ClassSummary::of(&Placement {
            peer_group: "Electric Guitars".into(),
            peer_total: 46_231,
            percentile: 0.84,
            segment: Segment::Premium,
        });
        let encoded = serde_json::to_string(&report).unwrap();
        assert!(encoded.contains(r#""segment":"premium""#));
        assert!(encoded.contains(r#""percentile":84"#));
    }
}
