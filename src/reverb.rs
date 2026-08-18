//! The Reverb API client, and the shapes gearprice reads back from it.
//!
//! Reverb's public read endpoints need no credentials, which is what lets gearprice stay
//! a single binary with nothing to configure. What they do need is manners: the client
//! paces its own requests, backs off when told to, caches what it has already asked for,
//! and refuses to make an unbounded number of calls on one run.
//!
//! Two limits shape everything built on top of this module:
//!
//! * `per_page` is capped at 50, and paging stops at page 50 — 2,500 listings is the most
//!   any single search can enumerate, however many matches it reports.
//! * Every search reports a `total` that respects its filters. That count is the way
//!   past the paging cap: see [`crate::quantile`].

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize};

use crate::cache::ResponseCache;
use crate::money::Money;
use crate::query::Search;

const DEFAULT_API_ROOT: &str = "https://api.reverb.com/api";
const API_VERSION: &str = "3.0";

/// Where the API lives. Overridable so the integration tests can drive the whole tool
/// against a stub server, and so a caller behind a proxy can point it somewhere else.
fn api_root() -> String {
    std::env::var("GEARPRICE_API_ROOT").unwrap_or_else(|_| DEFAULT_API_ROOT.to_string())
}

/// Reverb refuses anything larger and silently clamps to this.
pub const MAX_PER_PAGE: u32 = 50;
/// Reverb stops paging here regardless of how many results a search reports.
pub const MAX_PAGES: u32 = 50;
/// The most listings any one search can enumerate.
pub const MAX_ENUMERABLE: u32 = MAX_PER_PAGE * MAX_PAGES;

const RETRY_ATTEMPTS: u32 = 4;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(400);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(8);

/// Cost of one price report is dominated by count probes; this ceiling is well above
/// what any single command needs and exists to make a bug cost a slow run, not a ban.
pub const DEFAULT_REQUEST_BUDGET: u32 = 400;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Usage {
    pub requests: u32,
    pub cache_hits: u32,
}

pub struct Client {
    agent: ureq::Agent,
    currency: String,
    cache: Option<ResponseCache>,
    pace: Mutex<Option<Instant>>,
    min_interval: Duration,
    requests: AtomicU32,
    cache_hits: AtomicU32,
    budget: u32,
}

impl Client {
    pub fn new(currency: String, cache: Option<ResponseCache>, budget: u32) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(user_agent())
            // Status codes are handled here rather than raised as errors, because a 429
            // is a wait instruction and a 404 is an answer — neither is a failure.
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
            currency,
            cache,
            pace: Mutex::new(None),
            min_interval: Duration::from_millis(80),
            requests: AtomicU32::new(0),
            cache_hits: AtomicU32::new(0),
            budget,
        }
    }

    pub fn currency(&self) -> &str {
        &self.currency
    }

    pub fn usage(&self) -> Usage {
        Usage {
            requests: self.requests.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
        }
    }

    /// Sleeps just long enough that requests leave at most `min_interval` apart, however
    /// many threads are asking at once.
    fn pace(&self) {
        let mut last = self.pace.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = *last {
            let elapsed = previous.elapsed();
            if elapsed < self.min_interval {
                thread::sleep(self.min_interval - elapsed);
            }
        }
        *last = Some(Instant::now());
    }

    fn fetch(&self, url: &str) -> Result<String> {
        // The display currency changes the body for an identical URL, so it is part of
        // the cache key rather than left to collide.
        let cache_key = format!("{url}#{}", self.currency);
        if let Some(cache) = &self.cache
            && let Some(body) = cache.get(&cache_key)
        {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(body);
        }

        let mut last_error = None;
        for attempt in 0..RETRY_ATTEMPTS {
            if attempt > 0 {
                thread::sleep(backoff(
                    attempt,
                    last_error.as_ref().and_then(|error| {
                        if let RequestFailure::Retryable { retry_after, .. } = error {
                            *retry_after
                        } else {
                            None
                        }
                    }),
                ));
            }
            let used = self.requests.fetch_add(1, Ordering::Relaxed);
            if used >= self.budget {
                bail!(
                    "request budget of {} exhausted — narrow the search, or raise it with --request-budget",
                    self.budget
                );
            }
            self.pace();

            match self.attempt(url) {
                Ok(body) => {
                    if let Some(cache) = &self.cache {
                        // A cache that cannot be written is not a reason to fail a run
                        // that already has its answer.
                        let _ = cache.put(&cache_key, &body);
                    }
                    return Ok(body);
                }
                Err(failure @ RequestFailure::Retryable { .. }) => last_error = Some(failure),
                Err(RequestFailure::Fatal(error)) => return Err(error),
            }
        }
        match last_error {
            Some(RequestFailure::Retryable { message, .. }) => {
                bail!("{message} — gave up after {RETRY_ATTEMPTS} attempts")
            }
            _ => bail!("could not reach the Reverb API after {RETRY_ATTEMPTS} attempts"),
        }
    }

    fn attempt(&self, url: &str) -> std::result::Result<String, RequestFailure> {
        let response = self
            .agent
            .get(url)
            .header("Accept", "application/hal+json")
            .header("Content-Type", "application/hal+json")
            .header("Accept-Version", API_VERSION)
            .header("X-Display-Currency", &self.currency)
            .call();

        let mut response = match response {
            Ok(response) => response,
            // Transport-level trouble — DNS, TLS, a dropped connection, a timeout. All
            // worth another go.
            Err(error) => {
                return Err(RequestFailure::Retryable {
                    message: format!("could not reach the Reverb API: {error}"),
                    retry_after: None,
                });
            }
        };

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);

        if status == 429 || (500..600).contains(&status) {
            return Err(RequestFailure::Retryable {
                message: format!("Reverb returned HTTP {status} for {url}"),
                retry_after,
            });
        }
        if !(200..300).contains(&status) {
            let detail = response
                .body_mut()
                .with_config()
                .limit(8 * 1024)
                .read_to_string()
                .unwrap_or_default();
            return Err(RequestFailure::Fatal(anyhow::anyhow!(
                "Reverb returned HTTP {status} for {url}{}",
                summarise_error_body(&detail)
            )));
        }

        response
            .body_mut()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_string()
            .map_err(|error| RequestFailure::Retryable {
                message: format!("could not read the Reverb response: {error}"),
                retry_after: None,
            })
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let body = self.fetch(url)?;
        serde_json::from_str(&body)
            .with_context(|| format!("could not parse the Reverb response from {url}"))
    }

    /// One page of listings for `search`.
    pub fn listings(&self, search: &Search, page: u32, per_page: u32) -> Result<ListingPage> {
        let per_page = per_page.clamp(1, MAX_PER_PAGE);
        let parameters = search.parameters(&self.currency, page, per_page);
        self.get_json(&url(&format!("{}/listings", api_root()), &parameters))
    }

    /// How many listings match `search`, without transferring any of them.
    ///
    /// This is the primitive the whole percentile engine rests on: it respects every
    /// filter, including price bounds, and it is not subject to the paging cap.
    pub fn count(&self, search: &Search) -> Result<u32> {
        Ok(self.listings(search, 1, 1)?.total)
    }

    /// Reverb's own vocabulary: every brand it knows and every model name in its
    /// catalogue.
    ///
    /// The endpoint ignores its `query` parameter and returns the same list every time,
    /// which makes it useless as an autocomplete and ideal as a spelling dictionary —
    /// one request, cached, and the words come from Reverb rather than from a list
    /// hardcoded here that would rot.
    pub fn vocabulary(&self) -> Result<Vocabulary> {
        self.get_json(&format!("{}/autocomplete", api_root()))
    }

    /// Catalogue models matching `query`, most relevant first.
    pub fn models(&self, query: &str, per_page: u32) -> Result<ModelPage> {
        let parameters = vec![
            ("query".to_string(), query.to_string()),
            (
                "per_page".to_string(),
                per_page.min(MAX_PER_PAGE).to_string(),
            ),
        ];
        self.get_json(&url(&format!("{}/csps", api_root()), &parameters))
    }

    /// One catalogue model by id.
    pub fn model(&self, id: u64) -> Result<CatalogueModel> {
        self.get_json(&format!("{}/comparison_shopping_pages/{id}", api_root()))
    }

    pub fn categories(&self) -> Result<CategoryTree> {
        self.get_json(&format!("{}/categories", api_root()))
    }
}

enum RequestFailure {
    Retryable {
        message: String,
        retry_after: Option<Duration>,
    },
    Fatal(anyhow::Error),
}

fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    // A server that says how long to wait is obeyed, within reason.
    if let Some(hint) = retry_after {
        return hint.min(RETRY_MAX_DELAY);
    }
    RETRY_BASE_DELAY
        .saturating_mul(1 << attempt.min(5))
        .min(RETRY_MAX_DELAY)
}

fn summarise_error_body(body: &str) -> String {
    #[derive(Deserialize)]
    struct ApiError {
        message: Option<String>,
        #[serde(rename = "Error")]
        error: Option<String>,
    }
    let parsed: Option<ApiError> = serde_json::from_str(body).ok();
    match parsed.and_then(|value| value.message.or(value.error)) {
        Some(message) => format!(": {message}"),
        None => String::new(),
    }
}

/// Collapses runs of whitespace and trims, at the point text enters the program.
///
/// Reverb's own catalogue contains entries like `Squier\tParanormal Jazzmaster XII`, and
/// seller-written listing titles contain worse. A tab reaching a table wrecks every
/// column after it, so nothing downstream should have to remember to handle it.
fn collapse<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<String, D::Error> {
    let raw = String::deserialize(deserializer)?;
    Ok(collapse_whitespace(&raw))
}

pub fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn user_agent() -> String {
    format!(
        "gearprice/{} (+https://github.com/woksin/gearprice)",
        env!("CARGO_PKG_VERSION")
    )
}

/// Builds a URL, percent-encoding every parameter. Written out rather than pulled in as a
/// dependency because the rule is small and the encoding of `[` and `]` in `cp_ids[]`
/// matters more than a crate would.
pub fn url(base: &str, parameters: &[(String, String)]) -> String {
    let mut result = String::from(base);
    for (index, (key, value)) in parameters.iter().enumerate() {
        result.push(if index == 0 { '?' } else { '&' });
        result.push_str(&percent_encode(key));
        result.push('=');
        result.push_str(&percent_encode(value));
    }
    result
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            b' ' => encoded.push('+'),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

// ---------------------------------------------------------------------------
// Response shapes. Only the fields gearprice reads are named; Reverb adds more
// over time and an unknown field must never fail a parse.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct ListingPage {
    pub total: u32,
    #[serde(default)]
    pub listings: Vec<Listing>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Listing {
    pub id: u64,
    #[serde(default, deserialize_with = "collapse")]
    pub make: String,
    #[serde(default, deserialize_with = "collapse")]
    pub model: String,
    #[serde(default, deserialize_with = "collapse")]
    pub title: String,
    #[serde(default, deserialize_with = "collapse")]
    pub year: String,
    #[serde(default, deserialize_with = "collapse")]
    pub shop_name: String,
    pub condition: Grade,
    pub price: Price,
    #[serde(default)]
    pub auction: bool,
    #[serde(rename = "_links", default)]
    pub links: ListingLinks,
}

impl Listing {
    pub fn amount(&self) -> Money {
        Money::from_minor(self.price.amount_cents)
    }

    /// What to print for this listing: Reverb's title, falling back to make and model
    /// when a seller left it empty.
    pub fn describe(&self) -> String {
        if !self.title.trim().is_empty() {
            return self.title.trim().to_string();
        }
        format!("{} {}", self.make, self.model).trim().to_string()
    }

    pub fn web_url(&self) -> Option<&str> {
        self.links.web.as_ref().map(|link| link.href.as_str())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ListingLinks {
    #[serde(default)]
    pub web: Option<Link>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Link {
    pub href: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Grade {
    /// The grade Reverb reports, such as `excellent` or `brand-new`. Checked against the
    /// condition that was asked for, so a filter the server ignored gets noticed.
    #[serde(default)]
    pub slug: String,
    #[serde(default, deserialize_with = "collapse")]
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Price {
    #[serde(default)]
    pub amount_cents: i64,
}

/// Every brand and model name Reverb knows.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Vocabulary {
    #[serde(default)]
    pub makes: Vec<String>,
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelPage {
    #[serde(rename = "comparison_shopping_pages", default)]
    pub models: Vec<CatalogueModel>,
}

/// A Reverb comparison shopping page: one catalogue entry for one model over one era,
/// which is the unit a price class is worth quoting for.
#[derive(Clone, Debug, Deserialize)]
pub struct CatalogueModel {
    pub id: u64,
    #[serde(default)]
    pub slug: String,
    #[serde(default, deserialize_with = "collapse")]
    pub title: String,
    #[serde(default)]
    pub brand: Option<Brand>,
    #[serde(default)]
    pub used_total: u32,
    #[serde(default)]
    pub new_total: u32,
    #[serde(default)]
    pub used_low_price: Option<Price>,
    #[serde(default)]
    pub new_low_price: Option<Price>,
    #[serde(default)]
    pub root_category_slug: String,
    #[serde(rename = "_links", default)]
    pub links: ModelLinks,
}

impl CatalogueModel {
    pub fn brand_name(&self) -> &str {
        self.brand
            .as_ref()
            .map(|brand| brand.name.as_str())
            .unwrap_or_default()
    }

    pub fn web_url(&self) -> String {
        format!("https://reverb.com/p/{}", self.slug)
    }

    /// The catalogue product ids behind this model, read out of the listings link Reverb
    /// publishes for it.
    ///
    /// A model covers several catalogue products — one per finish or run — and pinning a
    /// search to those ids is what separates "this exact model" from a text search that
    /// also drags in cases, necks and unrelated years.
    pub fn product_ids(&self) -> Vec<u64> {
        let Some(link) = &self.links.listings else {
            return Vec::new();
        };
        let Some(query) = link.href.split_once('?').map(|(_, query)| query) else {
            return Vec::new();
        };
        let mut ids: Vec<u64> = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .filter(|(key, _)| decode_key(key) == "cp_ids[]")
            .filter_map(|(_, value)| value.parse().ok())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ModelLinks {
    #[serde(default)]
    pub listings: Option<Link>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Brand {
    #[serde(default, deserialize_with = "collapse")]
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CategoryTree {
    #[serde(default)]
    pub categories: Vec<Category>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Category {
    #[serde(default)]
    pub slug: String,
    #[serde(default, deserialize_with = "collapse")]
    pub name: String,
    #[serde(default, deserialize_with = "collapse")]
    pub full_name: String,
    #[serde(default)]
    pub subcategories: Vec<Category>,
}

fn decode_key(key: &str) -> String {
    key.replace("%5B", "[").replace("%5D", "]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encoding_preserves_bracketed_repeated_parameters() {
        let built = url(
            "https://api.reverb.com/api/listings",
            &[
                ("cp_ids[]".into(), "219711".into()),
                ("cp_ids[]".into(), "219712".into()),
                ("query".into(), "Gibson Les Paul".into()),
                ("sort".into(), "price|asc".into()),
            ],
        );
        assert_eq!(
            "https://api.reverb.com/api/listings?cp_ids%5B%5D=219711&cp_ids%5B%5D=219712&query=Gibson+Les+Paul&sort=price%7Casc",
            built
        );
    }

    #[test]
    fn product_ids_are_read_out_of_the_published_listings_link() {
        let model: CatalogueModel = serde_json::from_str(
            r#"{
                "id": 104713,
                "slug": "gibson-les-paul-standard-60s",
                "title": "Gibson Les Paul Standard '60s",
                "_links": {
                    "listings": {
                        "href": "https://api.reverb.com/api/listings/all?cp_ids%5B%5D=219712&cp_ids%5B%5D=219711&cp_ids%5B%5D=219711&per_page=24"
                    }
                }
            }"#,
        )
        .unwrap();
        // Sorted and deduplicated, so the same model always produces the same URL and so
        // the same cache entry.
        assert_eq!(vec![219_711, 219_712], model.product_ids());
    }

    #[test]
    fn a_model_without_a_listings_link_yields_no_product_ids() {
        let model: CatalogueModel =
            serde_json::from_str(r#"{"id": 1, "slug": "x", "title": "X"}"#).unwrap();
        assert!(model.product_ids().is_empty());
    }

    #[test]
    fn catalogue_whitespace_never_reaches_a_table() {
        // Reverb really does ship names like this; a tab would shift every column after
        // it, and a newline would break the row in two.
        let model: CatalogueModel = serde_json::from_str(
            "{\"id\": 1, \"slug\": \"x\", \"title\": \"Squier\\tParanormal  Jazzmaster XII \"}",
        )
        .unwrap();
        assert_eq!("Squier Paranormal Jazzmaster XII", model.title);

        let page: ListingPage = serde_json::from_str(
            "{\"total\": 1, \"listings\": [{\"id\": 1, \"title\": \" Gibson\\n Les  Paul \",
             \"shop_name\": \"A\\tShop\",
             \"condition\": {\"slug\": \"good\", \"display_name\": \"Good\"},
             \"price\": {\"amount_cents\": 100}}]}",
        )
        .unwrap();
        assert_eq!("Gibson Les Paul", page.listings[0].title);
        assert_eq!("A Shop", page.listings[0].shop_name);
    }

    #[test]
    fn unknown_fields_never_break_a_parse() {
        let page: ListingPage = serde_json::from_str(
            r#"{
                "total": 2,
                "some_field_reverb_added_later": true,
                "listings": [{
                    "id": 1,
                    "title": "A guitar",
                    "condition": {"slug": "excellent", "display_name": "Excellent"},
                    "price": {"amount": "1750.00", "amount_cents": 175000, "currency": "USD"},
                    "brand_new_field": 7
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(2, page.total);
        assert_eq!(Money::from_minor(175_000), page.listings[0].amount());
        assert_eq!("A guitar", page.listings[0].describe());
    }

    #[test]
    fn backoff_grows_but_obeys_an_explicit_retry_after() {
        assert!(backoff(1, None) < backoff(3, None));
        assert_eq!(RETRY_MAX_DELAY, backoff(9, None));
        assert_eq!(
            Duration::from_secs(2),
            backoff(1, Some(Duration::from_secs(2)))
        );
        // A server asking for an unreasonable wait is capped rather than obeyed.
        assert_eq!(RETRY_MAX_DELAY, backoff(1, Some(Duration::from_secs(600))));
    }

    #[test]
    fn error_bodies_are_summarised_from_either_spelling() {
        assert_eq!(
            ": Sorry, we couldn't find what you're looking for",
            summarise_error_body(
                r#"{"message":"Sorry, we couldn't find what you're looking for"}"#
            )
        );
        assert_eq!(
            ": This endpoint is no longer publicly available.",
            summarise_error_body(
                r#"{ "Error": "This endpoint is no longer publicly available." }"#
            )
        );
        assert_eq!("", summarise_error_body("<html>nope</html>"));
    }
}
