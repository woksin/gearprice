//! The two questions a price answers, and how each is decided.
//!
//! **Is this listing a good price?** is a question about one model's own market. The
//! answer is a band: where the asking price falls among the prices everyone else is
//! asking for the same thing.
//!
//! **What class of gear is this?** is a question about the market around it. The answer
//! is a segment: where this model's median sits among everything else in its category.
//! A $2,300 Les Paul is expensive for a guitar and unremarkable for a Les Paul, and those
//! are different facts.
//!
//! Both are percentile positions, which is deliberate. Fixed price thresholds go stale,
//! do not survive a currency change, and mean nothing across categories — the same $800
//! is a boutique pedal and an entry-level electric.

use anyhow::Result;
use serde::Serialize;

use crate::market::Market;
use crate::money::Money;
use crate::query::Search;
use crate::reverb::Client;
use crate::sold::Sold;

/// Where an asking price sits within its own model's market.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    Steal,
    Low,
    Fair,
    High,
    Premium,
}

impl Band {
    pub const ALL: [Band; 5] = [
        Band::Steal,
        Band::Low,
        Band::Fair,
        Band::High,
        Band::Premium,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Steal => "steal",
            Self::Low => "low",
            Self::Fair => "fair",
            Self::High => "high",
            Self::Premium => "premium",
        }
    }

    /// The percentile range this band covers.
    pub fn bounds(self) -> (f64, f64) {
        match self {
            Self::Steal => (0.00, 0.10),
            Self::Low => (0.10, 0.25),
            Self::Fair => (0.25, 0.75),
            Self::High => (0.75, 0.90),
            Self::Premium => (0.90, 1.00),
        }
    }

    /// What it means to land in this band, said in terms of whichever prices drew it.
    pub fn verdict(self, basis: Basis) -> &'static str {
        match (self, basis) {
            (Self::Steal, Basis::Sold) => "below almost anything anyone has paid",
            (Self::Low, Basis::Sold) => "under three quarters of recent sales",
            (Self::Fair, Basis::Sold) => "the middle half of what people paid",
            (Self::High, Basis::Sold) => "over three quarters of recent sales",
            (Self::Premium, Basis::Sold) => "above almost anything anyone has paid",
            (Self::Steal, Basis::Asking) => "below almost every other asking price",
            (Self::Low, Basis::Asking) => "cheaper than three quarters of the market",
            (Self::Fair, Basis::Asking) => "in the middle half — the going rate",
            (Self::High, Basis::Asking) => "dearer than three quarters of the market",
            (Self::Premium, Basis::Asking) => "above almost every other asking price",
        }
    }
}

/// What the bands are cut from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Prices people paid. The right answer where there is enough sold history.
    Sold,
    /// Prices people are asking, which run well above what they get.
    Asking,
}

impl Basis {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sold => "what people paid",
            Self::Asking => "what sellers are asking",
        }
    }
}

/// The price boundaries between bands for one market.
#[derive(Clone, Debug, Serialize)]
pub struct Bands {
    pub basis: Basis,
    pub steal_below: Money,
    pub low_below: Money,
    pub fair_below: Money,
    pub high_below: Money,
}

impl Bands {
    /// Bands cut from what people actually paid.
    ///
    /// This is the one to prefer wherever it can be had: asking prices run 16% to 46%
    /// above sold depending on the model, so bands drawn from them answer a different
    /// question than the one anyone is asking.
    pub fn of_sold(sold: &Sold) -> Option<Self> {
        if !sold.carries_bands() {
            return None;
        }
        Some(Self {
            basis: Basis::Sold,
            steal_below: sold.percentile(0.10)?,
            low_below: sold.percentile(0.25)?,
            fair_below: sold.percentile(0.75)?,
            high_below: sold.percentile(0.90)?,
        })
    }

    /// The band boundaries for a live market, or `None` when there is no market to draw
    /// them from. An empty search has percentiles, all of them zero; bands built on those
    /// would put every asking price in `premium` against a market of nothing.
    pub fn of(market: &Market) -> Option<Self> {
        if market.is_empty() {
            return None;
        }
        Some(Self {
            basis: Basis::Asking,
            steal_below: market.percentile(0.10)?,
            low_below: market.percentile(0.25)?,
            fair_below: market.percentile(0.75)?,
            high_below: market.percentile(0.90)?,
        })
    }

    /// The price range a band covers, open-ended at the two extremes.
    pub fn range(&self, band: Band) -> (Option<Money>, Option<Money>) {
        match band {
            Band::Steal => (None, Some(self.steal_below)),
            Band::Low => (Some(self.steal_below), Some(self.low_below)),
            Band::Fair => (Some(self.low_below), Some(self.fair_below)),
            Band::High => (Some(self.fair_below), Some(self.high_below)),
            Band::Premium => (Some(self.high_below), None),
        }
    }

    /// Which band an asking price falls in.
    ///
    /// Boundaries belong to the band above, so a price exactly on the p25 line is `fair`
    /// rather than `low` — the same rule the percentiles themselves use.
    pub fn band_for(&self, price: Money) -> Band {
        match price {
            value if value < self.steal_below => Band::Steal,
            value if value < self.low_below => Band::Low,
            value if value < self.fair_below => Band::Fair,
            value if value < self.high_below => Band::High,
            _ => Band::Premium,
        }
    }
}

/// The class of gear a model belongs to, judged against its category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Segment {
    Entry,
    Mid,
    Pro,
    Premium,
    Boutique,
}

impl Segment {
    pub const ALL: [Segment; 5] = [
        Segment::Entry,
        Segment::Mid,
        Segment::Pro,
        Segment::Premium,
        Segment::Boutique,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Mid => "mid",
            Self::Pro => "pro",
            Self::Premium => "premium",
            Self::Boutique => "boutique",
        }
    }

    /// The percentile range of the category this segment covers.
    pub fn bounds(self) -> (f64, f64) {
        match self {
            Self::Entry => (0.00, 0.20),
            Self::Mid => (0.20, 0.50),
            Self::Pro => (0.50, 0.80),
            Self::Premium => (0.80, 0.95),
            Self::Boutique => (0.95, 1.00),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Entry => "cheaper than four fifths of its category",
            Self::Mid => "below the category median",
            Self::Pro => "above the category median",
            Self::Premium => "dearer than four fifths of its category",
            Self::Boutique => "in the dearest twentieth of its category",
        }
    }

    pub fn from_percentile(percentile: f64) -> Self {
        match percentile {
            value if value < 0.20 => Self::Entry,
            value if value < 0.50 => Self::Mid,
            value if value < 0.80 => Self::Pro,
            value if value < 0.95 => Self::Premium,
            _ => Self::Boutique,
        }
    }
}

/// Where a price sits in a wider market.
#[derive(Clone, Debug, Serialize)]
pub struct Placement {
    pub peer_group: String,
    pub peer_total: u32,
    pub percentile: f64,
    pub segment: Segment,
}

/// Places `price` within `peers` — two counting requests, whatever the size of the
/// category.
///
/// Returns `None` when the peer group is empty, which is the honest answer: a position
/// among nothing is not a position.
pub fn place(
    client: &Client,
    peers: &Search,
    peer_label: &str,
    price: Money,
) -> Result<Option<Placement>> {
    let peer_total = client.count(peers)?;
    if peer_total == 0 {
        return Ok(None);
    }
    let at_or_below = client.count(&peers.between(None, Some(price)))?;
    let percentile = f64::from(at_or_below) / f64::from(peer_total);
    Ok(Some(Placement {
        peer_group: peer_label.to_string(),
        peer_total,
        percentile,
        segment: Segment::from_percentile(percentile),
    }))
}

/// The price boundaries between segments of a category, so a reader can see what the
/// classes actually mean in money.
pub fn segment_ladder(market: &Market) -> Vec<(Segment, Option<Money>, Option<Money>)> {
    Segment::ALL
        .into_iter()
        .map(|segment| {
            let (low, high) = segment.bounds();
            (
                segment,
                (low > 0.0).then(|| market.percentile(low)).flatten(),
                (high < 1.0).then(|| market.percentile(high)).flatten(),
            )
        })
        .collect()
}

/// The percentiles a segment ladder needs, which are not the ones a price report quotes.
pub const SEGMENT_PERCENTILES: [f64; 4] = [0.20, 0.50, 0.80, 0.95];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::Method;

    fn market_with(percentiles: &[(f64, i64)]) -> Market {
        Market {
            currency: "USD".into(),
            total: 100,
            low: Money::from_minor(percentiles[0].1),
            high: Money::from_minor(percentiles[percentiles.len() - 1].1),
            percentiles: percentiles
                .iter()
                .map(|(fraction, minor)| (*fraction, Money::from_minor(*minor)))
                .collect(),
            mean: None,
            grades: Vec::new(),
            method: Method::Counted { probes: 0 },
            sample: Vec::new(),
            observations: Vec::new(),
            observations_complete: true,
            histogram: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn sold_history() -> Sold {
        // Nine real sales, so the bands are allowed to be cut from them.
        let paid = [
            160_000, 168_000, 175_000, 180_000, 186_800, 195_000, 205_000, 220_000, 240_000,
        ];
        let sales: Vec<crate::sold::Transaction> = paid
            .iter()
            .map(|minor| crate::sold::Transaction {
                date: "2026-08-01".into(),
                condition: "Excellent".into(),
                asked: Some(Money::from_minor(minor + 20_000)),
                paid: Money::from_minor(*minor),
            })
            .collect();
        let sorted: Vec<Money> = paid.iter().map(|minor| Money::from_minor(*minor)).collect();
        Sold {
            currency: "USD".into(),
            recorded: 312,
            percentiles: crate::quantile::REPORT_PERCENTILES
                .iter()
                .map(|fraction| {
                    let rank = (fraction * sorted.len() as f64).ceil() as usize;
                    (*fraction, sorted[rank.clamp(1, sorted.len()) - 1])
                })
                .collect(),
            sales,
            covering: Some(("2026-06-03".into(), "2026-08-18".into())),
            typical_discount: Some(-0.09),
            at_or_above_ask: 3,
            discounts_seen: 9,
            grades: Vec::new(),
            recent_median: None,
            recent_sales: 0,
        }
    }

    fn les_paul_market() -> Market {
        market_with(&[
            (0.05, 160_000),
            (0.10, 175_000),
            (0.25, 209_000),
            (0.50, 227_500),
            (0.75, 245_000),
            (0.90, 288_000),
            (0.95, 320_000),
        ])
    }

    #[test]
    fn bands_prefer_what_people_paid_over_what_sellers_want() {
        let asking = Bands::of(&les_paul_market()).unwrap();
        let sold = Bands::of_sold(&sold_history()).unwrap();
        assert_eq!(Basis::Asking, asking.basis);
        assert_eq!(Basis::Sold, sold.basis);
        // The whole reason sold is preferred: every boundary sits lower.
        assert!(sold.fair_below < asking.fair_below);
        assert!(sold.steal_below < asking.steal_below);
    }

    #[test]
    fn a_record_too_thin_to_cut_into_five_bands_declines_to() {
        let mut thin = sold_history();
        thin.sales = Vec::new();
        thin.percentiles = vec![(0.50, Money::from_minor(410_000))];
        assert!(!thin.carries_bands());
        assert!(Bands::of_sold(&thin).is_none());
    }

    #[test]
    fn a_verdict_says_which_prices_it_is_talking_about() {
        assert_eq!(
            "the middle half of what people paid",
            Band::Fair.verdict(Basis::Sold)
        );
        assert_eq!(
            "in the middle half — the going rate",
            Band::Fair.verdict(Basis::Asking)
        );
    }

    #[test]
    fn bands_run_from_the_percentiles_of_the_model_itself() {
        let bands = Bands::of(&les_paul_market()).unwrap();
        assert_eq!(Money::from_minor(175_000), bands.steal_below);
        assert_eq!(Money::from_minor(288_000), bands.high_below);
    }

    #[test]
    fn an_asking_price_lands_in_the_band_the_percentiles_put_it_in() {
        let bands = Bands::of(&les_paul_market()).unwrap();
        assert_eq!(Band::Steal, bands.band_for(Money::from_minor(150_000)));
        assert_eq!(Band::Low, bands.band_for(Money::from_minor(190_000)));
        assert_eq!(Band::Fair, bands.band_for(Money::from_minor(227_500)));
        assert_eq!(Band::High, bands.band_for(Money::from_minor(260_000)));
        assert_eq!(Band::Premium, bands.band_for(Money::from_minor(400_000)));
    }

    #[test]
    fn a_price_exactly_on_a_boundary_belongs_to_the_band_above() {
        let bands = Bands::of(&les_paul_market()).unwrap();
        // p10 itself is not a steal: a tenth of the market is at or below it.
        assert_eq!(Band::Low, bands.band_for(bands.steal_below));
        assert_eq!(Band::Fair, bands.band_for(bands.low_below));
        assert_eq!(Band::Premium, bands.band_for(bands.high_below));
    }

    #[test]
    fn every_band_covers_a_range_and_the_extremes_stay_open_ended() {
        let bands = Bands::of(&les_paul_market()).unwrap();
        assert_eq!((None, Some(bands.steal_below)), bands.range(Band::Steal));
        assert_eq!((Some(bands.high_below), None), bands.range(Band::Premium));
        // The bands tile the line: each one starts where the last stopped.
        for pair in Band::ALL.windows(2) {
            assert_eq!(bands.range(pair[0]).1, bands.range(pair[1]).0);
        }
    }

    #[test]
    fn bands_are_unavailable_without_a_market_to_draw_them_from() {
        assert!(Bands::of(&market_with(&[(0.50, 100)])).is_none());
        let mut empty = les_paul_market();
        empty.total = 0;
        assert!(Bands::of(&empty).is_none());
    }

    #[test]
    fn segments_split_the_category_at_the_documented_percentiles() {
        assert_eq!(Segment::Entry, Segment::from_percentile(0.0));
        assert_eq!(Segment::Entry, Segment::from_percentile(0.199));
        assert_eq!(Segment::Mid, Segment::from_percentile(0.20));
        assert_eq!(Segment::Pro, Segment::from_percentile(0.50));
        // The worked example: a Les Paul Standard at the 84th percentile of its category.
        assert_eq!(Segment::Premium, Segment::from_percentile(0.84));
        assert_eq!(Segment::Boutique, Segment::from_percentile(0.99));
        assert_eq!(Segment::Boutique, Segment::from_percentile(1.0));
    }

    #[test]
    fn band_and_segment_boundaries_tile_their_range_without_a_gap() {
        for pair in Band::ALL.windows(2) {
            assert_eq!(pair[0].bounds().1, pair[1].bounds().0);
        }
        for pair in Segment::ALL.windows(2) {
            assert_eq!(pair[0].bounds().1, pair[1].bounds().0);
        }
        assert_eq!(0.0, Band::ALL[0].bounds().0);
        assert_eq!(1.0, Band::ALL[4].bounds().1);
        assert_eq!(0.0, Segment::ALL[0].bounds().0);
        assert_eq!(1.0, Segment::ALL[4].bounds().1);
    }

    #[test]
    fn the_band_a_percentile_names_matches_the_band_a_price_lands_in() {
        let market = les_paul_market();
        let bands = Bands::of(&market).unwrap();
        // The two ways of deciding a band must never disagree: the band whose percentile
        // range contains p must be the band the price at p lands in.
        for (fraction, price) in &market.percentiles {
            let by_percentile = Band::ALL
                .into_iter()
                .find(|band| {
                    let (low, high) = band.bounds();
                    *fraction >= low && *fraction < high
                })
                .unwrap_or(Band::Premium);
            let by_price = bands.band_for(*price);
            assert_eq!(
                by_percentile,
                by_price,
                "p{:.0} at {price:?} is {by_price:?} by price but {by_percentile:?} by percentile",
                fraction * 100.0
            );
        }
    }

    #[test]
    fn the_segment_ladder_leaves_both_ends_open() {
        let market = market_with(&[
            (0.20, 40_000),
            (0.50, 120_000),
            (0.80, 300_000),
            (0.95, 800_000),
        ]);
        let ladder = segment_ladder(&market);
        assert_eq!(5, ladder.len());
        assert_eq!(None, ladder[0].1);
        assert_eq!(Some(Money::from_minor(40_000)), ladder[0].2);
        assert_eq!(Some(Money::from_minor(800_000)), ladder[4].1);
        assert_eq!(None, ladder[4].2);
    }
}
