//! The canonical shape of everything gearprice reports.
//!
//! Table, JSON and CSV are three renderings of one structure rather than three
//! independent formatters, so a field can never appear in one and quietly go missing
//! from another. Reverb's own response types stop here: what leaves the tool is a schema
//! gearprice owns and can keep stable.

use serde::Serialize;

use crate::classify::{Band, Bands, Placement, Segment};
use crate::market::Market;
use crate::money::Money;
use crate::reverb::{CatalogueModel, Listing, Usage};

/// Said plainly and attached to every report: these are prices people are *asking*, not
/// prices anything *sold* for. Reverb retired its public sold-price endpoint, and a tool
/// that blurred the two would be worse than useless for deciding what to pay.
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
    pub bands: Vec<BandRow>,
}

impl BandTable {
    pub fn of(bands: &Bands, market: &Market) -> Self {
        let currency = market.currency.as_str();
        Self {
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
                        days_listed: market.days_listed_between(from, to),
                        verdict: band.verdict().to_string(),
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
            year: listing.year.clone(),
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
