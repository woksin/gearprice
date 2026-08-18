//! End-to-end tests.
//!
//! Everything here runs against a stub Reverb served from this process, so the suite is
//! deterministic, needs no network, and can put the API into states a live server would
//! not co-operate in producing — a rate limit, a filter it quietly ignores, a market of
//! one listing. `GEARPRICE_API_ROOT` is what points the binary at it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_gearprice")
}

/// One listing in the stub's inventory.
#[derive(Clone)]
struct Item {
    price_cents: i64,
    grade: &'static str,
    title: &'static str,
    /// When the listing went up, as an RFC 3339 timestamp.
    published_at: String,
    offers: bool,
}

/// A stub Reverb.
///
/// `/listings` is a real implementation rather than a canned response: it applies
/// `price_min`, `price_max` and `condition[]` to an inventory, sorts, pages, and reports
/// the filtered total. That is what makes the counting engine testable — the engine's
/// whole premise is that `total` respects a price bound, so a stub that ignored the bound
/// would let a broken engine pass. Other paths are canned.
struct Stub {
    address: String,
    hits: Arc<AtomicU32>,
}

impl Stub {
    fn start(inventory: Vec<Item>, routes: HashMap<String, Value>, rate_limit_first: u32) -> Self {
        Self::start_with(inventory, routes, rate_limit_first, true)
    }

    /// `honour_condition: false` reproduces Reverb's behaviour for a filter value it does
    /// not recognise — a 200 carrying the unfiltered set.
    fn start_with(
        inventory: Vec<Item>,
        routes: HashMap<String, Value>,
        rate_limit_first: u32,
        honour_condition: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a port");
        let address = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new((Mutex::new(routes), inventory, honour_condition));
        let hits = Arc::new(AtomicU32::new(0));

        let served = Arc::clone(&state);
        let counter = Arc::clone(&hits);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let served = Arc::clone(&served);
                let counter = Arc::clone(&counter);
                thread::spawn(move || {
                    handle(stream, &served, &counter, rate_limit_first);
                });
            }
        });
        Self { address, hits }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(binary())
            .args(arguments)
            .env("GEARPRICE_API_ROOT", &self.address)
            .env("GEARPRICE_NO_PROGRESS", "1")
            .env("NO_COLOR", "1")
            // A cache would make a second run in the same test answer from disk and hide
            // whatever the stub was set up to say.
            .args(["--no-cache"])
            .output()
            .expect("run gearprice")
    }
}

type State = (Mutex<HashMap<String, Value>>, Vec<Item>, bool);

fn handle(mut stream: TcpStream, state: &State, hits: &AtomicU32, rate_limit_first: u32) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone the socket"));
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    // Drain the headers so the client sees a clean exchange.
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) if header.trim().is_empty() => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
    let served = hits.fetch_add(1, Ordering::SeqCst);

    if served < rate_limit_first {
        let _ = stream.write_all(
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }

    let (routes, inventory, honour_condition) = state;
    let body = if path.starts_with("/listings") && !inventory.is_empty() {
        Some(listings_response(&path, inventory, *honour_condition).to_string())
    } else {
        routes
            .lock()
            .unwrap()
            .iter()
            // Longest matching key wins, so a specific route beats the general one.
            .filter(|(pattern, _)| matches(&path, pattern))
            .max_by_key(|(pattern, _)| pattern.len())
            .map(|(_, body)| body.to_string())
    };

    let response = match body {
        Some(body) => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/hal+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
        None => "HTTP/1.1 404 Not Found\r\nContent-Length: 34\r\nConnection: close\r\n\r\n{\"message\":\"no stub for that\"}".to_string(),
    };
    let _ = stream.write_all(response.as_bytes());
}

/// A pattern matches when every one of its `&`-separated fragments appears in the path.
fn matches(path: &str, pattern: &str) -> bool {
    pattern.split('&').all(|fragment| path.contains(fragment))
}

/// Reverb's `/listings`, as far as gearprice depends on it.
fn listings_response(path: &str, inventory: &[Item], honour_condition: bool) -> Value {
    let parameters = query_parameters(path);
    let number = |key: &str| {
        parameters
            .get(key)
            .and_then(|value| value.parse::<i64>().ok())
    };

    // Price bounds arrive in whole major units and are inclusive at both ends.
    let low = number("price_min").map(|value| value * 100);
    let high = number("price_max").map(|value| value * 100);
    let condition = honour_condition
        .then(|| parameters.get("condition[]").cloned())
        .flatten();

    let mut matched: Vec<&Item> = inventory
        .iter()
        .filter(|item| low.is_none_or(|low| item.price_cents >= low))
        .filter(|item| high.is_none_or(|high| item.price_cents <= high))
        .filter(|item| match condition.as_deref() {
            None => true,
            Some("used") => item.grade != "brand-new",
            Some("new") => item.grade == "brand-new",
            Some(grade) => item.grade == grade,
        })
        .collect();
    matched.sort_by_key(|item| item.price_cents);
    if parameters
        .get("sort")
        .is_some_and(|sort| sort.contains("desc"))
    {
        matched.reverse();
    }

    let total = matched.len();
    let per_page = number("per_page").unwrap_or(24).clamp(1, 50) as usize;
    let page = number("page").unwrap_or(1).max(1) as usize;
    let listings: Vec<Value> = matched
        .into_iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .enumerate()
        .map(|(index, item)| listing_from((index + 1) as u64, item))
        .collect();
    json!({"total": total, "listings": listings})
}

fn query_parameters(path: &str) -> HashMap<String, String> {
    let Some((_, query)) = path.split_once('?') else {
        return HashMap::new();
    };
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (decode(key), decode(value)))
        .collect()
}

fn decode(value: &str) -> String {
    let bytes = value.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) =
                u8::from_str_radix(&String::from_utf8_lossy(&bytes[index + 1..index + 3]), 16)
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn listing_from(id: u64, item: &Item) -> Value {
    let mut value = json!({
        "id": id,
        "make": "Gibson",
        "model": item.title,
        "title": item.title,
        "year": "2021",
        "shop_name": "Test Shop",
        "offers_enabled": item.offers,
        "condition": {"slug": item.grade, "display_name": item.grade},
        "price": {"amount_cents": item.price_cents, "currency": "USD"},
        "_links": {"web": {"href": format!("https://reverb.com/item/{id}")}}
    });
    if !item.published_at.is_empty() {
        value["published_at"] = json!(item.published_at);
    }
    value
}

/// An RFC 3339 timestamp `days` before now, for a listing that has been up that long.
fn listed_days_ago(days: i64) -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - days * 86_400;
    // 1970-01-01 plus `seconds`, formatted by hand to keep the test free of dependencies.
    let days_since_epoch = seconds / 86_400;
    let (year, month, day) = civil_from_days(days_since_epoch);
    format!("{year:04}-{month:02}-{day:02}T00:00:00Z")
}

/// Howard Hinnant's days-to-civil-date algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (year + i64::from(month <= 2), month, day)
}

/// An inventory of `count` listings priced evenly from `low` to `high`.
fn inventory(count: i64, low: i64, high: i64) -> Vec<Item> {
    (0..count)
        .map(|index| Item {
            price_cents: low + (high - low) * index / (count - 1).max(1),
            grade: "excellent",
            title: "Gibson Les Paul Standard",
            published_at: String::new(),
            offers: false,
        })
        .collect()
}

/// The canned responses every test needs: the catalogue model and the category tree.
fn catalogue_routes(used_total: i64) -> HashMap<String, Value> {
    let mut routes = HashMap::new();
    routes.insert(
        "/categories".to_string(),
        json!({"categories": [{
            "slug": "electric-guitars",
            "name": "Electric Guitars",
            "full_name": "Electric Guitars",
            "subcategories": [{
                "slug": "solid-body",
                "name": "Solid Body",
                "full_name": "Electric Guitars / Solid Body",
                "subcategories": []
            }]
        }]}),
    );
    routes.insert(
        "/csps".to_string(),
        json!({"total": 1, "comparison_shopping_pages": [{
            "id": 104713,
            "slug": "gibson-les-paul-standard-60s",
            "title": "Gibson Les Paul Standard '60s",
            "brand": {"name": "Gibson"},
            "used_total": used_total,
            "new_total": 0,
            "root_category_slug": "electric-guitars",
            "_links": {"listings": {"href": "https://api.reverb.com/api/listings/all?cp_ids%5B%5D=219711"}}
        }]}),
    );
    routes
}

/// Catalogue routes for a stub that answers `/csps` only for the exactly-right spelling,
/// the way Reverb does — two typos in one query and it returns nothing at all.
fn fussy_catalogue_routes() -> HashMap<String, Value> {
    let mut routes = catalogue_routes(100);
    routes.remove("/csps");
    // Matched on the encoded query, so only the corrected spelling finds anything.
    routes.insert(
        "/csps&query=gibson+les+paul+standard".to_string(),
        json!({"total": 1, "comparison_shopping_pages": [{
            "id": 104713,
            "slug": "gibson-les-paul-standard",
            "title": "Gibson Les Paul Standard",
            "brand": {"name": "Gibson"},
            "used_total": 190,
            "new_total": 40,
            "root_category_slug": "electric-guitars",
            "_links": {"listings": {"href": "https://api.reverb.com/api/listings/all?cp_ids%5B%5D=219711"}}
        }]}),
    );
    routes.insert(
        "/csps".to_string(),
        json!({"total": 0, "comparison_shopping_pages": []}),
    );
    // Reverb's vocabulary dump, which is what the spelling is corrected against.
    routes.insert(
        "/autocomplete".to_string(),
        json!({
            "makes": ["Gibson", "Fender", "Epiphone", "Boss"],
            "models": [
                "Les Paul Standard", "Les Paul Standard '60s", "Les Paul Studio",
                "Standard Stratocaster", "Standard Telecaster", "Dual Rectifier",
                "Paul Reed Smith Custom", "Paul Bearer", "Standard Jazzmaster"
            ]
        }),
    );
    routes
}

/// Catalogue routes plus an explicitly empty `/listings`, for the nothing-for-sale case.
fn empty_market_routes() -> HashMap<String, Value> {
    let mut routes = catalogue_routes(0);
    routes.insert("/listings".to_string(), json!({"total": 0, "listings": []}));
    routes
}

fn json_of(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "gearprice failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid JSON on stdout")
}

// ---------------------------------------------------------------------------

#[test]
fn version_and_help_describe_the_tool_without_touching_the_network() {
    let version = Command::new(binary()).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(
        String::from_utf8_lossy(&version.stdout)
            .contains(&format!("gearprice {}", env!("CARGO_PKG_VERSION")))
    );

    let help = Command::new(binary()).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout);
    for command in ["price", "models", "listings", "classes", "categories"] {
        assert!(help.contains(command), "--help does not mention {command}");
    }
    // The one caveat a reader must not miss.
    assert!(help.contains("asking prices"), "{help}");
}

#[test]
fn a_price_report_carries_bands_a_class_and_the_asking_price_verdict() {
    let stub = Stub::start(inventory(100, 100_000, 300_000), catalogue_routes(100), 0);
    let output = stub.run(&["price", "Les Paul", "--asking", "1200", "--format", "json"]);
    let report = json_of(&output);

    assert_eq!("Gibson Les Paul Standard '60s", report["model"]["title"]);
    assert_eq!(100, report["market"]["listings"]);
    // Priced evenly from $1,000 to $3,000, so the median is the middle of that.
    let median = report["market"]["percentiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|point| point["percentile"] == 50)
        .unwrap()["price"]
        .as_f64()
        .unwrap();
    assert!((1_980.0..=2_020.0).contains(&median), "median was {median}");

    // $1,200 against a market running to $3,000 is cheap.
    assert_eq!("low", report["asking"]["band"]);
    assert!(report["asking"]["against_median"].as_f64().unwrap() < 0.0);

    let bands = report["bands"]["bands"].as_array().unwrap();
    assert_eq!(5, bands.len());
    assert!(bands[0]["from"].is_null());
    assert!(bands[4]["to"].is_null());
    assert_eq!(
        "live Reverb listings — asking prices, not sold prices",
        report["source"]
    );
}

#[test]
fn percentiles_from_counting_match_percentiles_from_reading_every_listing() {
    // The same market twice: once small enough for the tool to enumerate, once large
    // enough that it must measure by counting instead. The two paths must agree.
    let small = Stub::start(inventory(100, 100_000, 300_000), catalogue_routes(100), 0);
    let read = json_of(&small.run(&["price", "Les Paul", "--format", "json"]));
    assert!(
        read["market"]["method"]
            .as_str()
            .unwrap()
            .contains("read all"),
        "expected the enumerating path, got {}",
        read["market"]["method"]
    );
    assert!(read["market"]["mean"].is_number());

    let large = Stub::start(
        inventory(5_000, 100_000, 300_000),
        catalogue_routes(5_000),
        0,
    );
    let counted = json_of(&large.run(&["price", "Les Paul", "--format", "json"]));
    assert!(
        counted["market"]["method"]
            .as_str()
            .unwrap()
            .contains("counting"),
        "expected the counting path, got {}",
        counted["market"]["method"]
    );
    // Counting never sees individual prices, so it must not invent a mean.
    assert!(counted["market"]["mean"].is_null());

    for point in read["market"]["percentiles"].as_array().unwrap() {
        let percentile = point["percentile"].as_u64().unwrap();
        let by_reading = point["price"].as_f64().unwrap();
        let by_counting = counted["market"]["percentiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|other| other["percentile"] == percentile)
            .unwrap()["price"]
            .as_f64()
            .unwrap();
        // Both describe the same even spread from $1,000 to $3,000.
        assert!(
            (by_reading - by_counting).abs() <= 40.0,
            "p{percentile}: reading says {by_reading}, counting says {by_counting}"
        );
    }
}

#[test]
fn a_rate_limited_api_is_waited_out_rather_than_failed() {
    // The first two requests are refused with a 429 and a Retry-After.
    let stub = Stub::start(inventory(20, 50_000, 90_000), catalogue_routes(20), 2);
    let output = stub.run(&["price", "Les Paul", "--format", "json"]);
    let report = json_of(&output);
    assert_eq!(20, report["market"]["listings"]);
    assert!(stub.hits.load(Ordering::SeqCst) > 2);
}

#[test]
fn a_filter_the_server_ignores_is_reported_rather_than_passed_off_as_filtered() {
    // An inventory of nothing but new stock, and a stub that does not honour
    // `condition[]` on it — which is how Reverb behaves for a filter value it does not
    // recognise: a 200, and the unfiltered set.
    let stock: Vec<Item> = (0..30)
        .map(|index| Item {
            price_cents: 100_000 + index * 5_000,
            grade: "brand-new",
            title: "Gibson Les Paul Standard",
            published_at: String::new(),
            offers: false,
        })
        .collect();
    let stub = Stub::start_with(stock, catalogue_routes(30), 0, false);
    let report = json_of(&stub.run(&[
        "price",
        "Les Paul",
        "--condition",
        "used",
        "--format",
        "json",
    ]));

    let warnings = report["diagnostics"]["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|warning| {
            let warning = warning.as_str().unwrap_or_default();
            warning.contains("outside the requested condition")
        }),
        "no warning about the ignored filter: {warnings:?}"
    );
}

#[test]
fn an_empty_market_reports_nothing_rather_than_inventing_a_price() {
    let stub = Stub::start(Vec::new(), empty_market_routes(), 0);
    let report = json_of(&stub.run(&["price", "Les Paul", "--format", "json"]));
    assert_eq!(0, report["market"]["listings"]);
    assert!(report["bands"].is_null());
    assert!(report["class"].is_null());
    let warnings = report["diagnostics"]["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap_or_default()
            .contains("no used listings match")),
        "{warnings:?}"
    );
}

#[test]
fn a_query_reverbs_own_search_cannot_spell_is_corrected_and_found() {
    // `/csps` here answers only for the exactly-right spelling, which is how Reverb
    // behaves: two typos in one query returns nothing at all.
    let stub = Stub::start(
        inventory(100, 100_000, 300_000),
        fussy_catalogue_routes(),
        0,
    );
    let report = json_of(&stub.run(&["price", "gibsen les pual standrd", "--format", "json"]));

    assert_eq!("Gibson Les Paul Standard", report["model"]["title"]);
    // And it says what it did, rather than quietly answering a different question.
    assert_eq!("gibson les paul standard", report["interpreted_as"]);
    assert!(report["market"]["listings"].as_u64().unwrap() > 0);
}

#[test]
fn a_correctly_spelled_query_never_pays_for_the_dictionary() {
    let stub = Stub::start(
        inventory(100, 100_000, 300_000),
        fussy_catalogue_routes(),
        0,
    );
    let report = json_of(&stub.run(&["price", "gibson les paul standard", "--format", "json"]));
    assert_eq!("Gibson Les Paul Standard", report["model"]["title"]);
    // Nothing to correct, so nothing to report and no vocabulary fetched.
    assert!(report["interpreted_as"].is_null());
}

#[test]
fn models_that_fit_the_query_equally_well_are_offered_rather_than_hidden() {
    let mut routes = catalogue_routes(100);
    routes.insert(
        "/csps".to_string(),
        json!({"total": 2, "comparison_shopping_pages": [
            {"id": 1, "slug": "a", "title": "Epiphone Casino (2023 - Present)",
             "brand": {"name": "Epiphone"}, "used_total": 10, "new_total": 16,
             "root_category_slug": "electric-guitars",
             "_links": {"listings": {"href": "https://api.reverb.com/api/listings?cp_ids%5B%5D=1"}}},
            {"id": 2, "slug": "b", "title": "Epiphone Casino Reissue 1995 - 2004",
             "brand": {"name": "Epiphone"}, "used_total": 30, "new_total": 0,
             "root_category_slug": "electric-guitars",
             "_links": {"listings": {"href": "https://api.reverb.com/api/listings?cp_ids%5B%5D=2"}}}
        ]}),
    );
    let stub = Stub::start(inventory(50, 50_000, 90_000), routes, 0);
    let report = json_of(&stub.run(&["price", "epiphone casino", "--format", "json"]));

    let alternatives = report["alternatives"].as_array().unwrap();
    assert_eq!(1, alternatives.len(), "{alternatives:?}");
    // Named with the id needed to price it instead, not just mentioned.
    assert!(alternatives[0]["id"].is_number());
    assert_ne!(report["model"]["id"], alternatives[0]["id"]);
}

#[test]
fn gear_reverb_has_no_catalogue_entry_for_is_flagged_rather_than_dressed_up() {
    let mut routes = catalogue_routes(100);
    routes.insert(
        "/csps".to_string(),
        json!({"total": 1, "comparison_shopping_pages": [
            {"id": 9, "slug": "bag", "title": "Gator Icon Bass Gig Bag",
             "brand": {"name": "Gator"}, "used_total": 9, "new_total": 0,
             "root_category_slug": "accessories",
             "_links": {"listings": {"href": "https://api.reverb.com/api/listings?cp_ids%5B%5D=9"}}}
        ]}),
    );
    let stub = Stub::start(inventory(20, 10_000, 40_000), routes, 0);
    let report = json_of(&stub.run(&["price", "a bag of crisps", "--format", "json"]));

    let warnings = report["diagnostics"]["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap_or_default()
            .contains("not a close thing")),
        "a poor match should say so: {warnings:?}"
    );
}

#[test]
fn a_band_that_is_not_clearing_shows_the_age_of_what_is_stuck_in_it() {
    // A market shaped like a real one: cheap listings turn over in a fortnight, dear ones
    // have been sitting for years. That is the whole signal — Reverb will not say what
    // anything sold for, but it will say how long the unsold have been waiting.
    let stock: Vec<Item> = (0..100)
        .map(|index| Item {
            price_cents: 100_000 + index * 2_000,
            grade: if index % 3 == 0 { "mint" } else { "excellent" },
            title: "Gibson Les Paul Standard",
            published_at: listed_days_ago(if index < 50 { 14 } else { 800 }),
            offers: index % 4 != 0,
        })
        .collect();
    let stub = Stub::start(stock, catalogue_routes(100), 0);
    let report = json_of(&stub.run(&["price", "Les Paul", "--format", "json"]));

    let bands = report["bands"]["bands"].as_array().unwrap();
    let aged = |name: &str| -> i64 {
        bands
            .iter()
            .find(|band| band["band"] == name)
            .and_then(|band| band["days_listed"].as_i64())
            .unwrap_or_else(|| panic!("no age for the {name} band: {bands:#?}"))
    };
    assert!(aged("steal") < 30, "the cheap end should be moving");
    assert!(aged("premium") > 365, "the dear end should be stuck");
    assert!(aged("premium") > aged("steal") * 10);

    // And the reader is told how firm these asking prices are.
    let offers = report["market"]["sellers_taking_offers"].as_f64().unwrap();
    assert!((offers - 0.75).abs() < 0.02, "offers share was {offers}");

    // Condition is reported best grade first, whatever order the listings arrived in.
    let grades: Vec<&str> = report["market"]["grades"]
        .as_array()
        .unwrap()
        .iter()
        .map(|grade| grade["grade"].as_str().unwrap())
        .collect();
    // The stub reports its display names in lower case; what matters is the order.
    assert_eq!(vec!["mint", "excellent"], grades);
}

#[test]
fn a_market_with_no_publication_dates_simply_omits_the_ages() {
    // Nothing invented from an absent date: no age is better than a wrong one.
    let stub = Stub::start(inventory(60, 100_000, 300_000), catalogue_routes(60), 0);
    let report = json_of(&stub.run(&["price", "Les Paul", "--format", "json"]));
    let bands = report["bands"]["bands"].as_array().unwrap();
    assert!(
        bands.iter().all(|band| band["days_listed"].is_null()),
        "ages appeared from nowhere: {bands:?}"
    );
}

#[test]
fn an_unknown_category_fails_instead_of_silently_pricing_the_whole_marketplace() {
    let stub = Stub::start(Vec::new(), empty_market_routes(), 0);
    let output = stub.run(&["classes", "electric-guitar"]);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("unknown category"), "{error}");
    assert!(error.contains("electric-guitars"), "{error}");
}

#[test]
fn the_request_budget_is_a_ceiling_a_run_cannot_cross() {
    let stub = Stub::start(
        inventory(5_000, 100_000, 900_000),
        catalogue_routes(5_000),
        0,
    );
    let output = stub.run(&["price", "Les Paul", "--request-budget", "6"]);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("request budget"), "{error}");
}

#[test]
fn csv_output_is_a_flat_table_and_defuses_spreadsheet_formulas() {
    let hostile: Vec<Item> = (0..20)
        .map(|index| Item {
            price_cents: 100_000 + index * 5_000,
            grade: "excellent",
            // A seller-controlled title that a spreadsheet would otherwise execute.
            title: "=HYPERLINK(\"http://evil.invalid\")",
            published_at: String::new(),
            offers: false,
        })
        .collect();
    let stub = Stub::start(hostile, catalogue_routes(20), 0);
    let output = stub.run(&["listings", "Les Paul", "--format", "csv"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let csv = String::from_utf8_lossy(&output.stdout);
    assert!(csv.starts_with("price,currency,condition,band,year,shop,title,url"));
    // The leading quote is what stops a spreadsheet running the cell.
    assert!(csv.contains("'=HYPERLINK"), "{csv}");
}

#[test]
fn the_cache_directory_can_be_inspected_and_emptied() {
    let directory = tempfile::tempdir().unwrap();
    let status = Command::new(binary())
        .args(["cache"])
        .env("GEARPRICE_CACHE_DIR", directory.path())
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains("Entries"));

    let cleared = Command::new(binary())
        .args(["cache", "--clear"])
        .env("GEARPRICE_CACHE_DIR", directory.path())
        .output()
        .unwrap();
    assert!(cleared.status.success());
    assert!(String::from_utf8_lossy(&cleared.stdout).contains("Cleared 0"));
}

#[test]
fn arguments_that_cannot_both_be_meant_are_refused() {
    for arguments in [
        vec!["price", "--model-id", "1", "--raw"],
        vec!["price", "--currency", "dollars", "les paul"],
        vec!["price"],
        vec!["listings"],
        vec!["models"],
    ] {
        let output = Command::new(binary()).args(&arguments).output().unwrap();
        assert!(
            !output.status.success(),
            "expected `gearprice {}` to be refused",
            arguments.join(" ")
        );
    }
}
