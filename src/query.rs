//! The search filter every command shares, and its translation into Reverb query
//! parameters.
//!
//! Reverb ignores a filter value it does not recognise rather than rejecting it — a
//! `condition=brand-new` request comes back as a full unfiltered result set with a 200.
//! A price built on that would be silently wrong, so the condition vocabulary is a
//! closed enum here: values Reverb honours are the only ones that can be constructed.

use std::fmt;

use clap::ValueEnum;
use serde::Serialize;

use crate::money::Money;

/// The condition filters Reverb actually applies.
///
/// `Used` and `New` are aggregates; the rest are single grades. The grade Reverb reports
/// back on a new listing is `brand-new`, but the *filter* spelling is `new` — passing
/// `brand-new` as a filter is one of the values it silently ignores, so it is absent here
/// by design.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Condition {
    /// Every listing, new and used alike.
    #[default]
    All,
    /// The used aggregate: mint through non-functioning, excluding new stock.
    Used,
    /// New stock. Reverb reports these back with the grade `brand-new`.
    New,
    BStock,
    Mint,
    MintInventory,
    Excellent,
    VeryGood,
    Good,
    Fair,
    Poor,
    NonFunctioning,
}

impl Condition {
    /// The wire value, or `None` for [`Condition::All`], which is the absence of a filter.
    pub fn slug(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Used => Some("used"),
            Self::New => Some("new"),
            Self::BStock => Some("b-stock"),
            Self::Mint => Some("mint"),
            Self::MintInventory => Some("mint-inventory"),
            Self::Excellent => Some("excellent"),
            Self::VeryGood => Some("very-good"),
            Self::Good => Some("good"),
            Self::Fair => Some("fair"),
            Self::Poor => Some("poor"),
            Self::NonFunctioning => Some("non-functioning"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all conditions",
            Self::Used => "used",
            Self::New => "new",
            Self::BStock => "B-stock",
            Self::Mint => "mint",
            Self::MintInventory => "mint (dealer stock)",
            Self::Excellent => "excellent",
            Self::VeryGood => "very good",
            Self::Good => "good",
            Self::Fair => "fair",
            Self::Poor => "poor",
            Self::NonFunctioning => "non-functioning",
        }
    }

    /// Whether a listing reporting `grade` belongs to this filter, used to verify that
    /// the server applied what was asked for.
    pub fn admits(self, grade: &str) -> bool {
        match self {
            Self::All => true,
            Self::Used => grade != "brand-new",
            Self::New => grade == "brand-new",
            other => other.slug() == Some(grade),
        }
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// How a listing search is ordered.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum Sort {
    /// Reverb's own relevance ranking.
    #[default]
    Relevance,
    PriceAscending,
    PriceDescending,
    Newest,
}

impl Sort {
    fn wire(self) -> Option<&'static str> {
        match self {
            Self::Relevance => None,
            Self::PriceAscending => Some("price|asc"),
            Self::PriceDescending => Some("price|desc"),
            Self::Newest => Some("published_at|desc"),
        }
    }
}

/// A Reverb listing search, independent of which endpoint runs it.
#[derive(Clone, Debug, Default)]
pub struct Search {
    pub text: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub category: Option<String>,
    pub product_type: Option<String>,
    /// Catalogue product ids. Present when the search is pinned to a specific model
    /// rather than matching free text.
    pub product_ids: Vec<u64>,
    pub condition: Condition,
    pub year_min: Option<u32>,
    pub year_max: Option<u32>,
    pub region: Option<String>,
    /// Only listings whose seller will send to this destination. Reverb applies it, so it
    /// composes with counting and narrows a market of any size to the buyable part.
    pub ships_to: Option<String>,
    pub price_min: Option<Money>,
    pub price_max: Option<Money>,
    pub sort: Sort,
}

impl Search {
    pub fn text(query: impl Into<String>) -> Self {
        Self {
            text: Some(query.into()),
            ..Self::default()
        }
    }

    pub fn products(ids: Vec<u64>) -> Self {
        Self {
            product_ids: ids,
            ..Self::default()
        }
    }

    /// The same search bounded to a price window, for walking the price axis.
    pub fn between(&self, low: Option<Money>, high: Option<Money>) -> Self {
        Self {
            price_min: low,
            price_max: high,
            ..self.clone()
        }
    }

    /// Whether this search can match anything at all. Reverb treats a request with no
    /// selector as "the entire marketplace", which is never what a caller meant.
    pub fn is_targeted(&self) -> bool {
        !self.product_ids.is_empty()
            || self.text.is_some()
            || self.make.is_some()
            || self.model.is_some()
            || self.category.is_some()
            || self.product_type.is_some()
    }

    pub fn parameters(&self, currency: &str, page: u32, per_page: u32) -> Vec<(String, String)> {
        let mut parameters = Vec::new();
        let mut push = |key: &str, value: String| parameters.push((key.to_string(), value));

        for id in &self.product_ids {
            push("cp_ids[]", id.to_string());
        }
        if let Some(text) = &self.text {
            push("query", text.clone());
        }
        if let Some(make) = &self.make {
            push("make", make.clone());
        }
        if let Some(model) = &self.model {
            push("model", model.clone());
        }
        if let Some(category) = &self.category {
            push("category", category.clone());
        }
        if let Some(product_type) = &self.product_type {
            push("product_type", product_type.clone());
        }
        if let Some(condition) = self.condition.slug() {
            push("condition[]", condition.to_string());
        }
        if let Some(year) = self.year_min {
            push("year_min", year.to_string());
        }
        if let Some(year) = self.year_max {
            push("year_max", year.to_string());
        }
        if let Some(region) = &self.region {
            push("item_region", region.clone());
        }
        if let Some(destination) = &self.ships_to {
            push("ships_to", destination.clone());
        }
        // Reverb takes these bounds in whole major units and treats both ends as
        // inclusive. Rounding outwards keeps a window from clipping a listing that sits
        // exactly on the boundary.
        if let Some(price) = self.price_min {
            push("price_min", price.major_floor(currency).to_string());
        }
        if let Some(price) = self.price_max {
            push("price_max", price.major_ceil(currency).to_string());
        }
        if let Some(sort) = self.sort.wire() {
            push("sort", sort.to_string());
        }
        push("page", page.to_string());
        push("per_page", per_page.to_string());
        parameters
    }

    /// A short human description of what was searched, for report headers.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(text) = &self.text {
            parts.push(text.clone());
        }
        if !self.product_ids.is_empty() {
            parts.push(format!("{} catalogue products", self.product_ids.len()));
        }
        if let Some(make) = &self.make {
            parts.push(format!("make {make}"));
        }
        if let Some(category) = &self.category {
            parts.push(format!("category {category}"));
        }
        match (self.year_min, self.year_max) {
            (Some(low), Some(high)) => parts.push(format!("{low}–{high}")),
            (Some(low), None) => parts.push(format!("{low} and later")),
            (None, Some(high)) => parts.push(format!("up to {high}")),
            (None, None) => {}
        }
        if let Some(region) = &self.region {
            parts.push(format!("in {region}"));
        }
        if let Some(destination) = &self.ships_to {
            parts.push(format!("shipping to {destination}"));
        }
        if parts.is_empty() {
            "the whole marketplace".to_string()
        } else {
            parts.join(", ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_condition_vocabulary_excludes_values_reverb_ignores() {
        // `brand-new` is the grade Reverb reports, never a filter it honours. If it were
        // constructible the tool would quietly report unfiltered numbers as "new".
        assert!(Condition::from_str("brand-new", true).is_err());
        assert_eq!(Some("new"), Condition::New.slug());
        assert_eq!(None, Condition::All.slug());
    }

    #[test]
    fn condition_membership_matches_the_grades_reverb_reports() {
        assert!(Condition::New.admits("brand-new"));
        assert!(!Condition::New.admits("excellent"));
        assert!(Condition::Used.admits("mint-inventory"));
        assert!(Condition::Used.admits("fair"));
        assert!(!Condition::Used.admits("brand-new"));
        assert!(Condition::Excellent.admits("excellent"));
        assert!(!Condition::Excellent.admits("very-good"));
        assert!(Condition::All.admits("anything"));
    }

    #[test]
    fn parameters_repeat_product_ids_and_round_price_bounds_outwards() {
        let search = Search {
            product_ids: vec![219_711, 219_712],
            condition: Condition::Used,
            price_min: Some(Money::from_minor(175_050)),
            price_max: Some(Money::from_minor(245_050)),
            sort: Sort::PriceAscending,
            ..Search::default()
        };
        let parameters = search.parameters("USD", 1, 50);
        let ids: Vec<_> = parameters
            .iter()
            .filter(|(key, _)| key == "cp_ids[]")
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(vec!["219711", "219712"], ids);
        assert!(parameters.contains(&("condition[]".into(), "used".into())));
        // Not sent at all when no destination was asked for, since an empty value is one
        // of the things Reverb ignores.
        assert!(!parameters.iter().any(|(key, _)| key == "ships_to"));
        // Outwards: the floor below, the ceiling above, so a listing on the boundary survives.
        assert!(parameters.contains(&("price_min".into(), "1750".into())));
        assert!(parameters.contains(&("price_max".into(), "2451".into())));
        assert!(parameters.contains(&("sort".into(), "price|asc".into())));
    }

    #[test]
    fn a_destination_becomes_a_filter_the_server_applies() {
        let search = Search {
            product_ids: vec![1],
            ships_to: Some("NO".into()),
            ..Search::default()
        };
        let parameters = search.parameters("USD", 1, 1);
        assert!(parameters.contains(&("ships_to".into(), "NO".into())));
        assert!(search.describe().contains("shipping to NO"));
    }

    #[test]
    fn a_search_with_no_selector_is_not_targeted() {
        assert!(!Search::default().is_targeted());
        assert!(Search::text("gibson").is_targeted());
        assert!(Search::products(vec![1]).is_targeted());
    }
}
