//! What it costs to have something sent to you, and whether the seller will send it.
//!
//! An asking price is not what a guitar costs. A $1,750 listing from Tokyo with $656 of
//! postage is dearer than a $1,900 one an hour's drive away, and a third of the listings
//! in any large market will not ship to a given country at all — of the 108,373 used
//! electric guitars on Reverb, 31,028 ship to Norway.
//!
//! Both facts come from Reverb rather than from anything hardcoded here. `ships_to` is a
//! search filter the server applies, so it composes with the counting engine and narrows
//! a market of any size to the part of it you can actually buy from. And the rate a
//! listing quotes is looked up through Reverb's own region tree, so Norway resolves to
//! its `EUR_NON_EU` rate rather than falling through to the everywhere-else price.

use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::money::Money;
use crate::reverb::{Client, Listing, ShippingRegion};

/// What a seller will charge to send one listing to one destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cost {
    /// A rate the seller published.
    Quoted(Money),
    /// The seller has the carrier price it at checkout. Genuinely unknown until then, and
    /// reported as unknown rather than as free.
    AtCheckout,
    /// No rate covers this destination.
    NotOffered,
}

impl Cost {
    pub fn amount(self) -> Option<Money> {
        match self {
            Self::Quoted(amount) => Some(amount),
            _ => None,
        }
    }
}

/// Reverb's shipping regions, flattened into a lookup from a code to the region above it.
pub struct Regions {
    parents: HashMap<String, Option<String>>,
    names: HashMap<String, String>,
}

impl Regions {
    pub fn load(client: &Client) -> Result<Self> {
        Ok(Self::build(&client.shipping_regions()?.shipping_regions))
    }

    pub fn build(roots: &[ShippingRegion]) -> Self {
        let mut parents = HashMap::new();
        let mut names = HashMap::new();
        fn walk(
            regions: &[ShippingRegion],
            parent: Option<&str>,
            parents: &mut HashMap<String, Option<String>>,
            names: &mut HashMap<String, String>,
        ) {
            for region in regions {
                parents.insert(region.code.to_uppercase(), parent.map(str::to_string));
                names.insert(region.code.to_uppercase(), region.name.clone());
                walk(&region.children, Some(&region.code), parents, names);
            }
        }
        walk(roots, None, &mut parents, &mut names);
        Self { parents, names }
    }

    pub fn knows(&self, code: &str) -> bool {
        self.parents.contains_key(&code.to_uppercase())
    }

    pub fn name(&self, code: &str) -> Option<&str> {
        self.names.get(&code.to_uppercase()).map(String::as_str)
    }

    /// A destination and every region that contains it, narrowest first, ending at the
    /// everywhere-else catch-all.
    ///
    /// This is the order a rate should be looked for: Norway's own rate beats the
    /// Europe-non-EU rate, which beats what the seller charges to ship anywhere.
    pub fn chain(&self, code: &str) -> Vec<String> {
        let code = code.to_uppercase();
        let mut chain = Vec::new();
        let mut current = Some(code);
        while let Some(step) = current {
            if !chain.contains(&step) {
                current = self.parents.get(&step).cloned().flatten();
                chain.push(step);
            } else {
                break;
            }
        }
        // Every seller who ships internationally at all quotes this one.
        if !chain.iter().any(|step| step == EVERYWHERE) {
            chain.push(EVERYWHERE.to_string());
        }
        chain
    }

    /// Checks a destination before it is sent as a filter.
    ///
    /// Reverb ignores a `ships_to` value it does not recognise and answers with the
    /// unfiltered market — the same trap as an unknown category or condition — so an
    /// unusable code has to be caught here rather than quietly widening the search.
    pub fn resolve(&self, code: &str) -> Result<String> {
        let code = code.trim().to_uppercase();
        if self.knows(&code) {
            return Ok(code);
        }
        let suggestions: Vec<&str> = self
            .names
            .iter()
            .filter(|(known, name)| {
                known.starts_with(&code) || name.to_uppercase().starts_with(&code)
            })
            .map(|(known, _)| known.as_str())
            .take(3)
            .collect();
        if suggestions.is_empty() {
            bail!(
                "{code:?} is not a shipping destination Reverb knows. Use a country code \
                 such as US, GB, DE or NO."
            );
        }
        bail!("{code:?} is not a shipping destination Reverb knows. Did you mean {suggestions:?}?")
    }
}

const EVERYWHERE: &str = "XX";

/// What one listing charges to reach a destination.
pub fn cost(listing: &Listing, chain: &[String]) -> Cost {
    let mut at_checkout = false;
    for step in chain {
        for rate in &listing.shipping.rates {
            if !rate.region_code.eq_ignore_ascii_case(step) {
                continue;
            }
            match &rate.rate {
                Some(price) => return Cost::Quoted(Money::from_minor(price.amount_cents)),
                // Keep looking: a wider region may carry a real number. Only if nothing
                // does is the answer "the carrier decides".
                None => at_checkout = true,
            }
        }
    }
    if at_checkout {
        Cost::AtCheckout
    } else {
        Cost::NotOffered
    }
}

/// What a listing actually costs delivered, where that can be known.
pub fn landed(listing: &Listing, chain: &[String]) -> Option<Money> {
    cost(listing, chain)
        .amount()
        .map(|shipping| Money::from_minor(listing.amount().minor + shipping.minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions() -> Regions {
        let tree: Vec<ShippingRegion> = serde_json::from_str(
            r#"[
                {"code": "XX", "name": "Everywhere", "children": []},
                {"code": "EUR_NON_EU", "name": "Europe (Non-EU)", "children": [
                    {"code": "NO", "name": "Norway", "children": []},
                    {"code": "CH", "name": "Switzerland", "children": []}
                ]},
                {"code": "EUR_EU", "name": "European Union", "children": [
                    {"code": "DE", "name": "Germany", "children": []}
                ]},
                {"code": "NORTH_AMERICA", "name": "North America", "children": [
                    {"code": "US", "name": "United States", "children": [
                        {"code": "US_CON", "name": "Continental U.S.", "children": []}
                    ]}
                ]}
            ]"#,
        )
        .unwrap();
        Regions::build(&tree)
    }

    fn listing(rates: &[(&str, Option<i64>)], price: i64) -> Listing {
        let rates: Vec<String> = rates
            .iter()
            .map(|(code, amount)| match amount {
                Some(amount) => {
                    format!(r#"{{"region_code":"{code}","rate":{{"amount_cents":{amount}}}}}"#)
                }
                None => format!(r#"{{"region_code":"{code}","rate":null}}"#),
            })
            .collect();
        serde_json::from_str(&format!(
            r#"{{"id":1,"condition":{{"slug":"good","display_name":"Good"}},
                 "price":{{"amount_cents":{price}}},
                 "shipping":{{"rates":[{}]}}}}"#,
            rates.join(",")
        ))
        .unwrap()
    }

    #[test]
    fn a_destination_widens_to_its_region_and_then_to_everywhere() {
        assert_eq!(vec!["NO", "EUR_NON_EU", "XX"], regions().chain("NO"));
        assert_eq!(
            vec!["US_CON", "US", "NORTH_AMERICA", "XX"],
            regions().chain("US_CON")
        );
        // A code with no place in the tree still ends at the catch-all.
        assert_eq!(vec!["ZZ", "XX"], regions().chain("ZZ"));
        // And the catch-all is not repeated.
        assert_eq!(vec!["XX"], regions().chain("XX"));
    }

    #[test]
    fn the_narrowest_rate_wins_rather_than_the_first_listed() {
        // The case that makes the tree worth fetching: quoting Norway the everywhere-else
        // price would overstate the postage by more than two hundred dollars.
        let chain = regions().chain("NO");
        let listing = listing(
            &[
                ("XX", Some(36_179)),
                ("EUR_NON_EU", Some(11_979)),
                ("US", Some(30_129)),
            ],
            175_000,
        );
        assert_eq!(
            Cost::Quoted(Money::from_minor(11_979)),
            cost(&listing, &chain)
        );
        assert_eq!(Some(Money::from_minor(186_979)), landed(&listing, &chain));
    }

    #[test]
    fn a_carrier_calculated_rate_is_unknown_rather_than_free() {
        let chain = regions().chain("US_CON");
        let listing = listing(&[("US_CON", None)], 179_200);
        assert_eq!(Cost::AtCheckout, cost(&listing, &chain));
        assert_eq!(None, landed(&listing, &chain));

        // Unless a wider region does quote a number, which is a real answer.
        let wider = listing_with_both();
        assert_eq!(Cost::Quoted(Money::from_minor(7_500)), cost(&wider, &chain));
    }

    fn listing_with_both() -> Listing {
        listing(&[("US_CON", None), ("NORTH_AMERICA", Some(7_500))], 179_200)
    }

    #[test]
    fn a_seller_who_does_not_ship_there_says_so() {
        let chain = regions().chain("NO");
        let listing = listing(&[("US_CON", Some(7_500))], 179_200);
        assert_eq!(Cost::NotOffered, cost(&listing, &chain));
        assert_eq!(None, landed(&listing, &chain));
        assert_eq!(Cost::NotOffered, cost(&listing_without_rates(), &chain));
    }

    fn listing_without_rates() -> Listing {
        listing(&[], 100_000)
    }

    #[test]
    fn an_unusable_destination_is_refused_before_it_widens_the_search() {
        let regions = regions();
        assert_eq!("NO", regions.resolve("no").unwrap());
        assert_eq!("US_CON", regions.resolve(" us_con ").unwrap());
        let error = regions.resolve("ZZZZ").unwrap_err().to_string();
        assert!(error.contains("not a shipping destination"), "{error}");
        // Reverb would answer an unknown code with the whole unfiltered market.
        assert!(regions.resolve("Norwayland").is_err());
    }

    #[test]
    fn region_names_come_from_reverb_rather_than_from_here() {
        assert_eq!(Some("Norway"), regions().name("NO"));
        assert_eq!(Some("Continental U.S."), regions().name("us_con"));
        assert_eq!(None, regions().name("ZZ"));
    }
}
