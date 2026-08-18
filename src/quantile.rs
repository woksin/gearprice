//! Exact percentiles over populations too large to download.
//!
//! Reverb will not hand over more than 2,500 listings for any one search, but it will
//! answer, cheaply and without transferring a single listing, "how many match?" — and it
//! applies price bounds to that count. So `count(price_max = P)` is the cumulative
//! distribution function of the market, evaluated at P, one HTTP request at a time.
//!
//! Given that, a percentile is a search rather than a download. For the p-th percentile
//! of n listings, the answer is the lowest price P where `count(P) >= ceil(p·n)`, and an
//! interpolating search finds it in a logarithmic number of probes.
//!
//! The percentiles are searched concurrently, sharing one memoised curve. That matters
//! more than it sounds: counting a category of a hundred thousand listings takes Reverb
//! well over a second per request, so seven searches in sequence is a minute and a half
//! of waiting, and the same seven in parallel is under half of that. The curve they leave
//! behind doubles as a histogram of the market.
//!
//! Percentiles come out at whole-major-unit resolution, which is the finest granularity
//! Reverb's price bounds accept. Within that, the answer is exact: it is the same number
//! a full enumeration would produce, from a few dozen requests, on a market of any size.

use std::collections::BTreeMap;
use std::sync::Mutex;

use anyhow::Result;
use rayon::prelude::*;

/// Probes are taken at whole major units, the granularity Reverb's price bounds accept.
pub type MajorUnits = i64;

/// Rungs in the seeding ladder. Enough to split the range for every percentile, few
/// enough to be one cheap parallel round trip.
const LADDER_STEPS: u32 = 8;

/// The percentiles every price report quotes, as fractions of the population.
pub const REPORT_PERCENTILES: [f64; 7] = [0.05, 0.10, 0.25, 0.50, 0.75, 0.90, 0.95];

#[derive(Clone, Debug)]
pub struct Sketch {
    /// Price to the number of listings at or below it — every probe the search took, in
    /// price order. A by-product of finding the percentiles, and enough to describe the
    /// shape of the market for free.
    pub curve: BTreeMap<MajorUnits, u32>,
    /// Requested fraction to the price at that percentile.
    pub quantiles: Vec<(f64, MajorUnits)>,
}

impl Sketch {
    #[cfg(test)]
    pub fn quantile(&self, fraction: f64) -> Option<MajorUnits> {
        self.quantiles
            .iter()
            .find(|(wanted, _)| (wanted - fraction).abs() < f64::EPSILON)
            .map(|(_, value)| *value)
    }

    /// A histogram of the market between two prices, built from probes already taken.
    ///
    /// The search leaves behind dozens of points on the curve, so the shape of the market
    /// is already paid for. This picks the points nearest to `bands` even divisions of
    /// the range and reads the counts between them — no further requests, and every count
    /// is one the server gave rather than an interpolation.
    pub fn histogram_between(
        &self,
        low: MajorUnits,
        high: MajorUnits,
        bands: usize,
    ) -> Vec<(MajorUnits, MajorUnits, u32)> {
        if high <= low || bands == 0 {
            return Vec::new();
        }
        let mut edges: Vec<MajorUnits> = (0..=bands)
            .filter_map(|step| {
                let target = low + (high - low) * step as MajorUnits / bands as MajorUnits;
                self.nearest_probe(target)
            })
            .collect();
        edges.dedup();
        edges
            .windows(2)
            .filter_map(|pair| {
                let from = self.curve.get(&pair[0])?;
                let to = self.curve.get(&pair[1])?;
                Some((pair[0], pair[1], to.saturating_sub(*from)))
            })
            .collect()
    }

    /// The probed price closest to `target`, since only probed prices have real counts.
    fn nearest_probe(&self, target: MajorUnits) -> Option<MajorUnits> {
        let below = self.curve.range(..=target).next_back().map(|(key, _)| *key);
        let above = self.curve.range(target..).next().map(|(key, _)| *key);
        match (below, above) {
            (Some(below), Some(above)) => Some(if target - below <= above - target {
                below
            } else {
                above
            }),
            (found, None) | (None, found) => found,
        }
    }
}

/// The nearest-rank position of a percentile in a population of `total`.
///
/// One-based, so rank 1 is the cheapest listing and rank `total` the dearest. A fraction
/// of zero still means "the first one", not "none of them".
fn rank_for(fraction: f64, total: u32) -> u32 {
    let rank = (fraction * f64::from(total)).ceil() as i64;
    rank.clamp(1, i64::from(total.max(1))) as u32
}

/// Builds a sketch of a price distribution from counting probes alone.
///
/// `count_at_or_below` must answer how many listings in the population are priced at or
/// below the given whole major unit, and is called from several threads at once. `low`
/// and `high` must bracket the population — the cheapest and dearest prices in it — which
/// a caller gets exactly from two sorted single-listing requests.
pub fn sketch(
    count_at_or_below: &(impl Fn(MajorUnits) -> Result<u32> + Sync),
    total: u32,
    low: MajorUnits,
    high: MajorUnits,
    wanted: &[f64],
) -> Result<Sketch> {
    if total == 0 {
        return Ok(Sketch {
            curve: BTreeMap::new(),
            quantiles: wanted.iter().map(|fraction| (*fraction, 0)).collect(),
        });
    }

    // The bracket is known without asking: nothing sits below the cheapest listing, and
    // everything sits at or below the dearest.
    let curve = Mutex::new(BTreeMap::from([(low.saturating_sub(1), 0), (high, total)]));

    // A log-spaced ladder first, taken in parallel — one round trip for all of it. This
    // exists purely to stop the searches below duplicating each other's work: without it
    // they all start from the same wide bracket and re-probe the same region, and the
    // request count runs about three quarters higher for no better answer. Log spacing
    // rather than even, because prices span orders of magnitude and even steps would put
    // every rung in the expensive tail.
    let rungs: Vec<MajorUnits> = (1..LADDER_STEPS)
        .map(|step| logarithmic_step(low, high, step, LADDER_STEPS))
        .filter(|price| *price > low && *price < high)
        .collect();
    rungs
        .par_iter()
        .map(|price| {
            let count = count_at_or_below(*price)?;
            locked(&curve).insert(*price, count);
            Ok(())
        })
        .collect::<Result<Vec<()>>>()?;

    // Each percentile then runs its own search, and every probe any of them takes lands
    // in the shared curve where the others can use it.
    let mut quantiles: Vec<(f64, MajorUnits)> = wanted
        .par_iter()
        .map(|fraction| {
            solve(count_at_or_below, &curve, total, low, high, *fraction)
                .map(|price| (*fraction, price))
        })
        .collect::<Result<Vec<_>>>()?;
    quantiles.sort_by(|left, right| left.0.total_cmp(&right.0));

    Ok(Sketch {
        curve: curve
            .into_inner()
            .unwrap_or_else(|error| error.into_inner()),
        quantiles,
    })
}

/// Finds one percentile, consulting and filling the shared curve as it goes.
fn solve(
    count_at_or_below: &impl Fn(MajorUnits) -> Result<u32>,
    curve: &Mutex<BTreeMap<MajorUnits, u32>>,
    total: u32,
    low: MajorUnits,
    high: MajorUnits,
    fraction: f64,
) -> Result<MajorUnits> {
    let rank = rank_for(fraction, total);
    let probe = |price: MajorUnits| -> Result<u32> {
        if let Some(known) = locked(curve).get(&price) {
            return Ok(*known);
        }
        let count = count_at_or_below(price)?;
        locked(curve).insert(price, count);
        Ok(count)
    };

    // Bracket from whatever is already known: the highest probe still short of the rank,
    // and the lowest probe that reaches it.
    let (mut below, mut at_or_above) = {
        let curve = locked(curve);
        (
            curve
                .iter()
                .filter(|(_, count)| **count < rank)
                .map(|(price, _)| *price)
                .next_back()
                .unwrap_or(low.saturating_sub(1)),
            curve
                .iter()
                .find(|(_, count)| **count >= rank)
                .map(|(price, _)| *price)
                .unwrap_or(high),
        )
    };

    // Converge fully rather than to a tolerance. Stopping early would report a price no
    // listing carries — $2,003 for a model where two hundred sellers all ask $1,999 —
    // which reads as precision while being wrong about the market.
    //
    // Interpolation, not plain bisection: the bracket comes with the counts at both ends,
    // so the position of the rank between them predicts the price far better than the
    // midpoint does. Every other step falls back to the midpoint, which keeps the
    // logarithmic worst case when a distribution defeats the guess.
    let mut step = 0_u32;
    while at_or_above - below > 1 {
        let (count_below, count_above) = {
            let curve = locked(curve);
            (
                curve.get(&below).copied().unwrap_or(0),
                curve.get(&at_or_above).copied().unwrap_or(total),
            )
        };
        let middle = if step.is_multiple_of(2) {
            interpolate(below, count_below, at_or_above, count_above, rank)
        } else {
            below + (at_or_above - below) / 2
        };
        if probe(middle)? >= rank {
            at_or_above = middle;
        } else {
            below = middle;
        }
        step += 1;
    }
    Ok(at_or_above)
}

/// A poisoned curve is still a correct curve — it only ever holds answers the server
/// gave — so a panicking sibling must not take the run down with it.
fn locked(
    curve: &Mutex<BTreeMap<MajorUnits, u32>>,
) -> std::sync::MutexGuard<'_, BTreeMap<MajorUnits, u32>> {
    curve.lock().unwrap_or_else(|error| error.into_inner())
}

/// The `step`-th of `steps` points from `low` to `high`, spaced geometrically.
fn logarithmic_step(low: MajorUnits, high: MajorUnits, step: u32, steps: u32) -> MajorUnits {
    let low = low.max(1) as f64;
    let high = high.max(2) as f64;
    let fraction = f64::from(step) / f64::from(steps);
    (low * (high / low).powf(fraction)).round() as MajorUnits
}

/// Guesses where `rank` falls between two probed prices, assuming listings are spread
/// evenly between them. Always lands strictly inside the bracket, so a guess that is
/// wildly wrong still makes progress rather than looping.
fn interpolate(
    below: MajorUnits,
    count_below: u32,
    at_or_above: MajorUnits,
    count_above: u32,
    rank: u32,
) -> MajorUnits {
    let width = at_or_above - below;
    let span = count_above.saturating_sub(count_below);
    let guess = if span == 0 {
        below + width / 2
    } else {
        let position = f64::from(rank.saturating_sub(count_below)) / f64::from(span);
        below + (width as f64 * position).round() as MajorUnits
    };
    // Held away from the bracket edges. A rank sitting exactly on the upper count — the
    // common case, since counts are step functions — interpolates to the very top of the
    // window and would then creep down one unit at a time.
    let margin = (width / 8).max(1);
    guess.clamp(below + margin, at_or_above - margin)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU32, Ordering};

    /// The truth a sketch is checked against: a population held in memory, counted the
    /// same way Reverb counts, and charged for every probe.
    struct Population {
        prices: Vec<MajorUnits>,
        probes: AtomicU32,
    }

    impl Population {
        fn new(mut prices: Vec<MajorUnits>) -> Self {
            prices.sort_unstable();
            Self {
                prices,
                probes: AtomicU32::new(0),
            }
        }

        fn count_at_or_below(&self, price: MajorUnits) -> Result<u32> {
            self.probes.fetch_add(1, Ordering::Relaxed);
            Ok(self.prices.partition_point(|value| *value <= price) as u32)
        }

        fn probes(&self) -> u32 {
            self.probes.load(Ordering::Relaxed)
        }

        /// The percentile a full enumeration would report, by the same nearest-rank rule.
        fn true_quantile(&self, fraction: f64) -> MajorUnits {
            let rank = rank_for(fraction, self.prices.len() as u32);
            self.prices[rank as usize - 1]
        }
    }

    fn sketch_of(prices: Vec<MajorUnits>, wanted: &[f64]) -> (Sketch, Population) {
        let population = Population::new(prices);
        let total = population.prices.len() as u32;
        let low = *population.prices.first().unwrap_or(&0);
        let high = *population.prices.last().unwrap_or(&0);
        let result = sketch(
            &|price| population.count_at_or_below(price),
            total,
            low,
            high,
            wanted,
        )
        .unwrap();
        (result, population)
    }

    #[test]
    fn percentiles_match_a_full_enumeration_of_the_same_population() {
        // A market shaped like a real one: a dense middle, a long expensive tail.
        let prices: Vec<MajorUnits> = (0..400)
            .map(|index| 800 + (index * index) / 40)
            .chain([25_000, 38_000])
            .collect();
        let (result, population) = sketch_of(prices, &REPORT_PERCENTILES);
        for fraction in REPORT_PERCENTILES {
            let found = result.quantile(fraction).unwrap();
            let truth = population.true_quantile(fraction);
            assert_eq!(
                truth,
                found,
                "p{:.0} came out {found}, enumeration says {truth}",
                fraction * 100.0
            );
        }
    }

    #[test]
    fn a_market_of_ties_resolves_to_the_exact_price() {
        // Half the market listed at exactly 1,999 — the shape a popular model really has.
        let prices: Vec<MajorUnits> = std::iter::repeat_n(1_999, 200)
            .chain(std::iter::repeat_n(3_500, 100))
            .chain([700, 12_000])
            .collect();
        let (result, _) = sketch_of(prices, &[0.25, 0.50, 0.90]);
        assert_eq!(Some(1_999), result.quantile(0.25));
        assert_eq!(Some(1_999), result.quantile(0.50));
        assert_eq!(Some(3_500), result.quantile(0.90));
    }

    #[test]
    fn the_cost_stays_logarithmic_across_four_orders_of_magnitude() {
        let prices: Vec<MajorUnits> = (0..5_000).map(|index| 50 + index * 20).collect();
        let (_, population) = sketch_of(prices, &REPORT_PERCENTILES);
        let probes = population.probes();
        // Seven percentiles over 100,000 units of price range. A linear scan would be
        // hopeless; a per-percentile search without the shared ladder would be far worse.
        assert!(
            probes <= 110,
            "took {probes} probes — interpolating searches over a shared curve should settle \
             seven percentiles across a 100,000-unit range without brute force"
        );
    }

    #[test]
    fn extremes_land_on_the_cheapest_and_dearest_listings() {
        let (result, _) = sketch_of(vec![300, 450, 900, 2_000, 9_500], &[0.0, 1.0]);
        assert_eq!(Some(300), result.quantile(0.0));
        assert_eq!(Some(9_500), result.quantile(1.0));
    }

    #[test]
    fn a_single_listing_is_its_own_every_percentile() {
        let (result, _) = sketch_of(vec![1_450], &REPORT_PERCENTILES);
        for fraction in REPORT_PERCENTILES {
            assert_eq!(Some(1_450), result.quantile(fraction));
        }
    }

    #[test]
    fn the_seeding_ladder_spans_the_range_geometrically() {
        // Even steps would put seven of eight rungs above $50,000 on this range; log
        // steps put them where listings actually are.
        let rungs: Vec<_> = (1..8)
            .map(|step| logarithmic_step(100, 100_000, step, 8))
            .collect();
        assert!(rungs.windows(2).all(|pair| pair[0] < pair[1]), "{rungs:?}");
        assert!(rungs[0] < 250, "first rung at {}", rungs[0]);
        assert!(rungs[6] > 30_000, "last rung at {}", rungs[6]);
    }

    #[test]
    fn an_empty_market_costs_nothing_and_reports_nothing() {
        let population = Population::new(Vec::new());
        let result = sketch(
            &|price| population.count_at_or_below(price),
            0,
            0,
            0,
            &REPORT_PERCENTILES,
        )
        .unwrap();
        assert_eq!(0, population.probes());
        assert!(result.quantiles.iter().all(|(_, price)| *price == 0));
    }

    #[test]
    fn the_histogram_covers_the_range_it_is_asked_for_without_new_probes() {
        let prices: Vec<MajorUnits> = (0..300).map(|index| 500 + index * 7).collect();
        let (result, population) = sketch_of(prices, &REPORT_PERCENTILES);
        let spent = population.probes();
        let low = result.quantile(0.05).unwrap();
        let high = result.quantile(0.95).unwrap();
        let bands = result.histogram_between(low, high, 8);

        assert!(!bands.is_empty());
        // Built entirely from probes already taken.
        assert_eq!(spent, population.probes());
        // Contiguous: each band starts where the last one ended.
        for pair in bands.windows(2) {
            assert_eq!(pair[0].1, pair[1].0);
        }
        // Roughly the middle ninety per cent of a population of three hundred.
        let counted: u32 = bands.iter().map(|(_, _, count)| count).sum();
        assert!((240..=290).contains(&counted), "counted {counted}");
    }

    #[test]
    fn nearest_rank_places_the_edges_where_they_belong() {
        assert_eq!(1, rank_for(0.0, 100));
        assert_eq!(1, rank_for(0.001, 100));
        assert_eq!(50, rank_for(0.50, 100));
        assert_eq!(100, rank_for(1.0, 100));
        // A population of one has one rank, whatever is asked for.
        assert_eq!(1, rank_for(0.99, 1));
    }
}
