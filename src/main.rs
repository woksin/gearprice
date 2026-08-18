//! gearprice — what guitar gear costs on Reverb right now, and what class it sits in.

mod cache;
mod classify;
mod history;
mod market;
mod money;
mod output;
mod paths;
mod progress;
mod quantile;
mod query;
mod report;
mod resolve;
mod reverb;
mod shipping;
mod sold;
mod target;
mod taxonomy;
mod update;
mod years;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use rayon::prelude::*;

use classify::{Band, Bands, SEGMENT_PERCENTILES, place, segment_ladder};
use history::{History, Movement, Snapshot, Subject};
use market::{Market, measure, measure_at};
use money::Money;
use output::{Format, Style};
use progress::Progress;
use query::{Condition, Search, Sort};
use report::{
    AlternativeModel, AskingVerdict, BandTable, ClassReport, ClassRow, ClassSummary, DealReport,
    Diagnostics, ListingReport, ListingSummary, MarketSummary, ModelReport, ModelRow, ModelSummary,
    PriceReport, Reading, SOURCE, SoldSummary, TrackReport, TrackedReport, TrackedSubject,
    VariantReport, VariantRow,
};
use reverb::{CatalogueModel, Client, DEFAULT_REQUEST_BUDGET, MAX_PER_PAGE};
use shipping::Regions;
use taxonomy::{Selector, Taxonomy};

const ABOUT: &str = "What guitar gear costs on Reverb right now, and what class it sits in";

const LONG_ABOUT: &str = "\
What guitar gear costs on Reverb right now, and what class it sits in.

gearprice reads Reverb's public marketplace — no account, no API key — and answers three
things. What the gear actually sells for, from completed sales rather than hopeful asking
prices. Whether a given price is a steal or a fleecing. And what class of gear it is:
entry, mid, pro, premium or boutique.

Looking at a listing right now? `gearprice deal <paste the address>` judges that one.";

/// Shown under the command list, because a list of eleven commands is not an answer to
/// "what do I type".
const EXAMPLES: &str = "\
Examples:
  gearprice \"gibson les paul standard\"          what it sells for, and what class it is
  gearprice deal https://reverb.com/item/…      is this listing worth it?
  gearprice \"les paul\" --asking 1900            is 1900 a good price?
  gearprice variants \"marshall major\"           which version am I looking at?
  gearprice \"jazzmaster\" -c NOK --ships-to NO   in kroner, from sellers who post to Norway
  gearprice classes electric-guitars            what entry, mid and boutique cost

Spelling is forgiven: \"gibsen les pual standrd\" finds the Les Paul Standard.";

#[derive(Debug, Parser)]
#[command(name = "gearprice", version, about = ABOUT, long_about = LONG_ABOUT, after_help = EXAMPLES)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Currency to report in. Reverb converts every listing to it.
    #[arg(
        long,
        short = 'c',
        global = true,
        env = "GEARPRICE_CURRENCY",
        default_value = "USD",
        value_name = "CODE"
    )]
    currency: String,

    #[arg(long, global = true, value_enum, default_value_t = Format::Table)]
    format: Format,

    /// Ignore cached responses and ask Reverb again.
    #[arg(long, global = true)]
    no_cache: bool,

    /// How long a cached response stays good.
    #[arg(
        long,
        global = true,
        env = "GEARPRICE_CACHE_TTL",
        default_value_t = 360,
        value_name = "MINUTES"
    )]
    cache_ttl: u64,

    /// Most API requests one run may make.
    #[arg(long, global = true, default_value_t = DEFAULT_REQUEST_BUDGET, value_name = "N")]
    request_budget: u32,

    #[arg(long, global = true, help = "Do not show the progress spinner")]
    no_progress: bool,

    #[arg(long, global = true, help = "Do not colour the output")]
    no_color: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Price one model: its bands, its class, and a verdict on an asking price
    Price(Box<PriceArguments>),
    /// Find the catalogue models matching a search, to price one exactly
    #[command(visible_alias = "search")]
    Models(ModelsArguments),
    /// List what is for sale right now, labelled by band
    Listings(ListingsArguments),
    /// Show what each price class means in money, for a whole category
    Classes(ClassesArguments),
    /// Show Reverb's category tree, for --category
    Categories,
    /// Judge one listing: paste its Reverb address, or give its id
    Deal(Box<DealArguments>),
    /// Compare every version of a model — eras, guitar against bass, each run
    Variants(VariantsArguments),
    /// Record what a market costs today, and show what it has done since
    Track(Box<TrackArguments>),
    /// Print a shell completion script
    Completions(CompletionArguments),
    /// Inspect or empty the response cache
    Cache(CacheArguments),
    /// Check for and install a newer gearprice release
    Update(UpdateArguments),
}

/// Filters shared by every command that searches listings.
#[derive(Clone, Debug, Args)]
struct Filters {
    /// Which condition to price.
    #[arg(long, value_enum, default_value_t = Condition::Used)]
    condition: Condition,

    /// Restrict to a category slug, or `root/leaf`. See `gearprice categories`.
    #[arg(long, value_name = "SLUG")]
    category: Option<String>,

    /// Restrict to one brand.
    #[arg(long, value_name = "NAME")]
    make: Option<String>,

    #[arg(long, value_name = "YEAR")]
    year_min: Option<u32>,

    #[arg(long, value_name = "YEAR")]
    year_max: Option<u32>,

    /// Restrict to listings in one region, as a country code such as US or GB.
    #[arg(long, value_name = "CODE")]
    region: Option<String>,

    /// Only listings whose seller ships to here, and price them delivered.
    #[arg(long, env = "GEARPRICE_SHIPS_TO", value_name = "CODE")]
    ships_to: Option<String>,
}

#[derive(Debug, Args)]
struct PriceArguments {
    /// The gear to price, as you would type it into a search box.
    #[arg(value_name = "QUERY", num_args = 0..)]
    query: Vec<String>,

    /// Price this exact catalogue model, from `gearprice models`.
    #[arg(long, value_name = "ID", conflicts_with = "raw")]
    model_id: Option<u64>,

    /// Judge an asking price against the bands.
    #[arg(long, short = 'a', value_name = "PRICE")]
    asking: Option<f64>,

    /// Search the words given rather than resolving them to a catalogue model.
    #[arg(long)]
    raw: bool,

    /// Never widen a thin catalogue market with a text search.
    #[arg(long, conflicts_with = "raw")]
    strict: bool,

    /// Skip the price class, which costs two extra requests.
    #[arg(long)]
    no_class: bool,

    /// Skip the sold history, which costs one or two extra requests.
    #[arg(long)]
    no_sold: bool,

    /// How many recent sales to read.
    #[arg(long, default_value_t = sold::TARGET_SAMPLE, value_name = "N")]
    sold_sample: u32,

    /// How many listings to show.
    #[arg(long, default_value_t = 3, value_name = "N")]
    listings: usize,

    /// Show the asking-price histogram and everything else that is normally trimmed.
    #[arg(long)]
    full: bool,

    #[command(flatten)]
    filters: Filters,
}

#[derive(Debug, Args)]
struct ModelsArguments {
    #[arg(value_name = "QUERY", num_args = 1..)]
    query: Vec<String>,

    #[arg(long, default_value_t = 15, value_name = "N")]
    limit: u32,
}

#[derive(Debug, Args)]
struct ListingsArguments {
    #[arg(value_name = "QUERY", num_args = 0..)]
    query: Vec<String>,

    #[arg(long, value_name = "ID")]
    model_id: Option<u64>,

    /// Show only listings priced below the going rate.
    #[arg(long)]
    deals: bool,

    /// Search the words given rather than resolving them to a catalogue model.
    #[arg(long, conflicts_with = "model_id")]
    raw: bool,

    #[arg(long, default_value_t = 20, value_name = "N")]
    limit: u32,

    /// Order the results.
    #[arg(long, value_enum, default_value_t = Sort::PriceAscending)]
    sort: Sort,

    #[command(flatten)]
    filters: Filters,
}

#[derive(Debug, Args)]
struct ClassesArguments {
    /// The category to break into classes. See `gearprice categories`.
    #[arg(value_name = "CATEGORY")]
    category: String,

    #[arg(long, value_enum, default_value_t = Condition::Used)]
    condition: Condition,

    #[arg(long, value_name = "NAME")]
    make: Option<String>,
}

#[derive(Debug, Args)]
struct VariantsArguments {
    #[arg(value_name = "QUERY", num_args = 1..)]
    query: Vec<String>,

    /// How many versions to compare.
    #[arg(long, default_value_t = 8, value_name = "N")]
    limit: u32,

    /// How many recent sales to read for each.
    #[arg(long, default_value_t = 50, value_name = "N")]
    sold_sample: u32,
}

#[derive(Debug, Args)]
struct DealArguments {
    /// A reverb.com listing address, or a listing id.
    #[arg(value_name = "LISTING")]
    listing: String,

    /// How many recent sales to read.
    #[arg(long, default_value_t = sold::TARGET_SAMPLE, value_name = "N")]
    sold_sample: u32,

    #[command(flatten)]
    filters: Filters,
}

#[derive(Debug, Args)]
struct TrackArguments {
    /// The gear to record. Omit with --list to see everything already tracked.
    #[arg(value_name = "QUERY", num_args = 0..)]
    query: Vec<String>,

    /// Show everything the history has readings of, and record nothing.
    #[arg(long, conflicts_with_all = ["query", "no_record"])]
    list: bool,

    /// Report the history without adding a reading to it.
    #[arg(long)]
    no_record: bool,

    #[arg(long, value_name = "ID")]
    model_id: Option<u64>,

    /// Search the words given rather than resolving them to a catalogue model.
    #[arg(long, conflicts_with = "model_id")]
    raw: bool,

    #[command(flatten)]
    filters: Filters,
}

#[derive(Debug, Args)]
struct CompletionArguments {
    /// Which shell to generate for.
    #[arg(value_enum)]
    shell: clap_complete::Shell,
}

#[derive(Debug, Args)]
struct CacheArguments {
    /// Delete every cached response.
    #[arg(long)]
    clear: bool,
}

#[derive(Debug, Args)]
struct UpdateArguments {
    /// Report whether a newer version exists without installing it.
    #[arg(long)]
    check: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gearprice: {error:#}");
        std::process::exit(1);
    }
}

/// Lets the first argument be the gear itself: `gearprice "les paul"` rather than
/// `gearprice price "les paul"`.
///
/// Only when it is the *first* argument, and only when it is not a flag. Anything else
/// would have to know which options take values, and `gearprice --currency NOK price ...`
/// would start pricing `NOK`.
fn with_default_command(arguments: impl Iterator<Item = String>) -> Vec<String> {
    let mut arguments: Vec<String> = arguments.collect();
    let definition = <Cli as clap::CommandFactory>::command();
    let known: Vec<&str> = definition
        .get_subcommands()
        .flat_map(|command| std::iter::once(command.get_name()).chain(command.get_all_aliases()))
        .collect();
    let takes_default = arguments
        .get(1)
        .is_some_and(|first| !first.starts_with('-') && !known.contains(&first.as_str()));
    if takes_default {
        arguments.insert(1, "price".to_string());
    }
    arguments
}

fn run() -> Result<()> {
    let cli = Cli::parse_from(with_default_command(std::env::args()));
    let style = Style::new(cli.no_color);

    match &cli.command {
        Command::Update(arguments) => return run_update(arguments),
        Command::Cache(arguments) => return run_cache(arguments, &cli),
        Command::Completions(arguments) => {
            let mut command = <Cli as clap::CommandFactory>::command();
            clap_complete::generate(
                arguments.shell,
                &mut command,
                "gearprice",
                &mut std::io::stdout(),
            );
            return Ok(());
        }
        _ => {}
    }

    let client = build_client(&cli)?;
    let mut progress = Progress::new(cli.no_progress || cli.format != Format::Table);

    let result = match &cli.command {
        Command::Price(arguments) => run_price(&cli, arguments, &client, &progress, style),
        Command::Models(arguments) => run_models(&cli, arguments, &client, &progress, style),
        Command::Listings(arguments) => run_listings(&cli, arguments, &client, &progress, style),
        Command::Classes(arguments) => run_classes(&cli, arguments, &client, &progress, style),
        Command::Deal(arguments) => run_deal(&cli, arguments, &client, &progress, style),
        Command::Variants(arguments) => run_variants(&cli, arguments, &client, &progress, style),
        Command::Track(arguments) => run_track(&cli, arguments, &client, &progress, style),
        Command::Categories => run_categories(&cli, &client, &progress, style),
        Command::Cache(_) | Command::Update(_) | Command::Completions(_) => {
            unreachable!("handled above")
        }
    };
    progress.finish();
    result
}

fn build_client(cli: &Cli) -> Result<Client> {
    let currency = cli.currency.trim().to_uppercase();
    if currency.len() != 3
        || !currency
            .chars()
            .all(|character| character.is_ascii_alphabetic())
    {
        bail!(
            "currency must be a three-letter code such as USD, EUR or NOK, not {:?}",
            cli.currency
        );
    }
    let cache = (!cli.no_cache).then(|| {
        cache::ResponseCache::new(
            paths::response_cache_dir(),
            Duration::from_secs(cli.cache_ttl.saturating_mul(60)),
        )
    });
    Ok(Client::new(currency, cache, cli.request_budget))
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Applies the shared filters to a search, resolving the category against the live tree.
fn apply_filters(
    search: &mut Search,
    filters: &Filters,
    client: &Client,
) -> Result<Option<Selector>> {
    search.condition = filters.condition;
    search.make = filters.make.clone();
    search.year_min = filters.year_min;
    search.year_max = filters.year_max;
    search.region = filters.region.as_ref().map(|code| code.to_uppercase());
    if let Some(destination) = &filters.ships_to {
        // Checked against Reverb's own region tree first: an unknown destination is
        // ignored by the server, which would silently widen the market back out.
        let regions = Regions::load(client)?;
        let code = regions.resolve(destination)?;
        search.ships_to = Some(code);
    }
    let Some(slug) = &filters.category else {
        return Ok(None);
    };
    let selector = Taxonomy::load(client)?.resolve(slug)?;
    search.product_type = Some(selector.product_type.clone());
    search.category = selector.category.clone();
    Ok(Some(selector))
}

// ---------------------------------------------------------------------------
// price
// ---------------------------------------------------------------------------

fn run_price(
    cli: &Cli,
    arguments: &PriceArguments,
    client: &Client,
    progress: &Progress,
    style: Style,
) -> Result<()> {
    let words = arguments.query.join(" ").trim().to_string();
    if words.is_empty() && arguments.model_id.is_none() {
        bail!("say what to price, for example: gearprice price \"Gibson Les Paul Standard\"");
    }
    let mut warnings = Vec::new();

    progress.set("Resolving the model");
    let resolution = resolve_model(
        client,
        arguments.model_id,
        arguments.raw,
        &words,
        &mut warnings,
    )?;
    let model = resolution.chosen.clone();

    let mut search = match &model {
        Some(model) if !model.product_ids().is_empty() => Search::products(model.product_ids()),
        // A model with no catalogue products behind it cannot pin a search, so fall back
        // to its title, which is still tighter than what the user typed.
        Some(model) => Search::text(model.title.clone()),
        None => Search::text(words.clone()),
    };
    let selector = apply_filters(&mut search, &arguments.filters, client)?;
    taxonomy::require_targeted(&search)?;

    progress.set("Measuring the market");
    let market = measure(client, &search)?;
    warnings.extend(market.warnings.iter().cloned());
    if market.is_empty() {
        warnings.push(format!(
            "no {} listings match right now — try --condition all, or a wider search",
            arguments.filters.condition
        ));
    }

    // What people paid, where Reverb has a record of it. Only a catalogue model has a
    // sold history — a free-text search is not a thing that can have been sold.
    let sold = match model.as_ref().filter(|_| !arguments.no_sold) {
        Some(model) => {
            progress.set("Reading what it has sold for");
            // Sold history is an enhancement, not a dependency. A model Reverb has no
            // transaction record for, or an endpoint that has moved again, must cost the
            // sold section and nothing else — the live market is still worth reporting.
            let history = match sold::read(client, model.id, arguments.sold_sample) {
                Ok(history) => history,
                Err(error) => {
                    warnings.push(format!(
                        "could not read the sold history ({error}) — the prices below are \
                         what sellers are asking"
                    ));
                    sold::Sold::none(client.currency())
                }
            };
            if history.is_empty() {
                warnings.push(
                    "Reverb has no record of this model selling, so every price here is \
                     what someone is asking rather than what anyone paid"
                        .into(),
                );
                None
            } else {
                if !history.carries_bands() {
                    warnings.push(format!(
                        "only {} recorded {} — quoted below, but too few to cut into bands, \
                         so those come from asking prices",
                        history.sales.len(),
                        if history.sales.len() == 1 {
                            "sale"
                        } else {
                            "sales"
                        }
                    ));
                }
                Some(history)
            }
        }
        None => None,
    };

    // A pinned market this thin is probably missing listings the seller never attached to
    // the catalogue entry. Widen, and say what widening found.
    let market = match model.as_ref().filter(|_| !arguments.strict) {
        Some(model) if market.total < market::WIDEN_BELOW && !market.is_empty() => {
            progress.set("Widening the search");
            match widen(client, &search, &market, model, &arguments.filters) {
                Ok(Some((wider, added))) => {
                    warnings.push(format!(
                        "the catalogue entry has {} listings; a wider search found {added} more \
                         that no seller attached to it, and all {} are priced below",
                        market.total, wider.total
                    ));
                    wider
                }
                Ok(None) => market,
                Err(error) => {
                    warnings.push(format!("could not widen the search ({error})"));
                    market
                }
            }
        }
        _ => market,
    };

    // Sold where there is enough of it, asking otherwise — and the report says which.
    let bands = sold
        .as_ref()
        .and_then(Bands::of_sold)
        .or_else(|| Bands::of(&market));

    let class = if arguments.no_class || market.is_empty() {
        None
    } else {
        progress.set("Placing it against its category");
        price_class(
            client,
            &model,
            &selector,
            &market,
            &arguments.filters,
            &mut warnings,
        )?
    };

    let asking = arguments.asking.and_then(|price| {
        let bands = bands.as_ref()?;
        let amount = Money::from_major(price, &market.currency);
        let band = bands.band_for(amount);
        // Compared against what people paid when that is known. Judging an asking price
        // against other asking prices only says whether the seller is in line with other
        // sellers, which is not the question anyone is asking.
        let against = match bands.basis {
            classify::Basis::Sold => sold.as_ref().and_then(sold::Sold::median)?,
            classify::Basis::Asking => market.median()?,
        };
        Some(AskingVerdict {
            price,
            band,
            basis: bands.basis,
            verdict: band.verdict(bands.basis).to_string(),
            against_median: amount.major(&market.currency) - against.major(&market.currency),
        })
    });

    // Sorted by what they actually cost to get here, which is not the order they are
    // priced in: a cheaper guitar from further away is often the dearer one.
    let destination = search
        .ships_to
        .as_ref()
        .map(|code| {
            Regions::load(client).map(|regions| {
                let label = regions
                    .name(code)
                    .map(|name| format!("{name} ({code})"))
                    .unwrap_or_else(|| code.clone());
                (label, regions.chain(code))
            })
        })
        .transpose()?;
    let chain = destination.as_ref().map(|(_, chain)| chain.as_slice());
    let mut shown: Vec<&reverb::Listing> = market.sample.iter().collect();
    if let Some(chain) = chain {
        shown.sort_by_key(|listing| {
            shipping::landed(listing, chain).unwrap_or_else(|| listing.amount())
        });
    }
    let model_title = model
        .as_ref()
        .map(|model| model.title.as_str())
        .unwrap_or_default();
    let now = Utc::now();
    let listings: Vec<ListingSummary> = shown
        .into_iter()
        .take(arguments.listings)
        .map(|listing| {
            ListingSummary::of(
                listing,
                &market.currency,
                bands.as_ref(),
                chain,
                model_title,
                now,
            )
        })
        .collect();

    // Said before the tables rather than left for the reader to work out.
    if let Some(caution) = thin_market(&market, &search) {
        warnings.push(caution);
    }
    if let Some(caution) = year_field_caution(model.as_ref(), &market) {
        warnings.push(caution);
    }

    let report = PriceReport {
        query: if words.is_empty() {
            search.describe()
        } else {
            words
        },
        source: SOURCE,
        generated_at: timestamp(),
        currency: market.currency.clone(),
        condition: arguments.filters.condition.label().to_string(),
        model: model.as_ref().map(ModelSummary::of),
        interpreted_as: resolution.interpreted_as.clone(),
        alternatives: resolution
            .alternatives
            .iter()
            .map(AlternativeModel::of)
            .collect(),
        market: MarketSummary::of(&market),
        sold: sold.as_ref().map(|sold| SoldSummary::of(sold, &market)),
        bands: bands.as_ref().map(|bands| BandTable::of(bands, &market)),
        class: class.as_ref().map(ClassSummary::of),
        asking,
        delivered_to: destination.as_ref().map(|(code, _)| code.clone()),
        listings,
        diagnostics: Diagnostics::from(client.usage(), warnings),
    };

    match cli.format {
        Format::Table => output::print_price(&report, style, arguments.full),
        Format::Json => output::print_json(&report)?,
        Format::Csv => output::print_price_csv(&report)?,
    }
    Ok(())
}

/// Finds the catalogue model a query means, unless the caller asked for raw text.
///
/// This is what separates a price for a Boss DS-1 from a price for everything with those
/// words in the title — the T-shirt, the knob set, the footswitch cover, the service
/// notes. A text search returns all of them and the cheap junk lands in the `steal` band,
/// which is exactly the wrong answer. Pinning to the catalogue model searches the
/// products Reverb has identified as that piece of gear.
///
/// See [`crate::resolve`] for how a misspelled or abbreviated query gets there.
fn resolve_model(
    client: &Client,
    model_id: Option<u64>,
    raw: bool,
    words: &str,
    warnings: &mut Vec<String>,
) -> Result<resolve::Resolution> {
    // A pasted URL is not a search. Someone looking at a listing and asking about it has
    // told us exactly which thing they mean, and guessing from its words would be worse.
    match target::parse(words) {
        target::Target::Listing(id) => {
            let listing = client
                .listing(id)
                .with_context(|| format!("no Reverb listing with id {id}"))?;
            let Some(model) = listing.model_id() else {
                warnings.push(format!(
                    "Reverb has not matched {:?} to a catalogue model, so this prices its \
                     words rather than the model",
                    listing.describe()
                ));
                return Ok(resolve::Resolution::none());
            };
            return Ok(resolve::Resolution::of(client.model(model)?));
        }
        target::Target::Model(id) => return Ok(resolve::Resolution::of(client.model(id)?)),
        target::Target::ModelSlug(slug) => {
            return Ok(resolve::Resolution::of(
                client
                    .model_by_slug(&slug)
                    .with_context(|| format!("no catalogue model at reverb.com/p/{slug}"))?,
            ));
        }
        target::Target::Words(_) => {}
    }
    if let Some(id) = model_id {
        let model = client.model(id).with_context(|| {
            format!("no catalogue model has id {id} — find one with `gearprice models`")
        })?;
        return Ok(resolve::Resolution::of(model));
    }
    if raw {
        return Ok(resolve::Resolution::none());
    }
    let resolution = resolve::resolve(client, words, 12)?;
    let Some(model) = resolution.chosen.as_ref() else {
        warnings.push(format!(
            "nothing in Reverb's catalogue matches {words:?}, so this prices the words \
             themselves and may include unrelated gear. `gearprice models` searches the \
             catalogue directly."
        ));
        return Ok(resolution);
    };
    if resolution.is_weak() {
        warnings.push(format!(
            "{:?} is the closest thing in Reverb's catalogue to {words:?}, and it is not a \
             close thing — check it is what you meant before trusting these numbers",
            model.title
        ));
    }
    Ok(resolution)
}

/// Adds listings a text search finds that the catalogue entry does not carry.
///
/// Only the ones whose titles plausibly describe the same model are kept — a widened
/// search for a Boss DS-1 also returns a T-shirt, and the whole reason for pinning in the
/// first place was to keep that out of the cheap band.
fn widen(
    client: &Client,
    pinned: &Search,
    market: &Market,
    model: &CatalogueModel,
    filters: &Filters,
) -> Result<Option<(Market, usize)>> {
    // The name a seller would use, not the catalogue's full description of the thing.
    let wanted = resolve::signature(&model.title, model.brand_name());
    if wanted.is_empty() {
        return Ok(None);
    }
    let mut text = Search::text(wanted.join(" "));
    apply_filters(&mut text, filters, client)?;
    text.ships_to = pinned.ships_to.clone();
    let found = measure(client, &text)?;

    let known: Vec<u64> = market.sample.iter().map(|listing| listing.id).collect();
    // A second guard, on price. Word matching alone let a set of Bluetooth headphones into
    // a market of four-thousand dollar amplifiers; nothing genuinely the same model is an
    // order of magnitude from the middle of it.
    let middle = market.median().map(|price| price.minor).unwrap_or_default();
    let plausible = |listing: &reverb::Listing| {
        middle == 0
            || (listing.amount().minor * 8 >= middle && listing.amount().minor <= middle * 8)
    };
    let extra: Vec<reverb::Listing> = found
        .sample
        .iter()
        .filter(|listing| !known.contains(&listing.id))
        .filter(|listing| resolve::describes(&listing.describe(), &wanted))
        .filter(|listing| plausible(listing))
        .cloned()
        .collect();
    if extra.is_empty() {
        return Ok(None);
    }
    let added = extra.len();
    let mut merged = market.sample.clone();
    merged.extend(extra);
    Ok(Some((
        market::from_listings(merged, market.currency.clone(), filters.condition),
        added,
    )))
}

/// Warns when a market is too small for five percentile bands to mean anything.
///
/// Six listings do not have a ninetieth percentile. Printing five bands over them looks
/// exactly like printing five bands over six hundred, and a reader has no way to tell.
fn thin_market(market: &Market, search: &Search) -> Option<String> {
    const ENOUGH: u32 = 12;
    (market.total > 0 && market.total < ENOUGH).then(|| {
        format!(
            "only {} {} listings — the bands below are arithmetic on {} numbers rather than \
             a distribution, so read the listings themselves",
            market.total, search.condition, market.total
        )
    })
}

/// Warns when Reverb's year field on this model holds the model number.
///
/// Marshall's guitar Major is Model 1967 and was built from 1968; every listing reports
/// its year as 1967. Anyone filtering by year on this model gets a confident wrong answer,
/// and this tool did until it was caught.
fn year_field_caution(model: Option<&CatalogueModel>, market: &Market) -> Option<String> {
    let model = model?;
    let designations = years::model_numbers(&model.title);
    if designations.is_empty() {
        return None;
    }
    let confused = market
        .sample
        .iter()
        .filter(|listing| {
            listing
                .year
                .trim()
                .parse::<u32>()
                .is_ok_and(|year| designations.contains(&year))
        })
        .count();
    (confused > 0).then(|| {
        format!(
            "Reverb reports the year on {confused} of these listings as {}, which is this \
             model's designation rather than a year — --year-min and --year-max are \
             unreliable here, and the years shown come from the titles",
            designations
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" or ")
        )
    })
}

/// Places a model's median against its category.
fn price_class(
    client: &Client,
    model: &Option<CatalogueModel>,
    selector: &Option<Selector>,
    market: &Market,
    filters: &Filters,
    warnings: &mut Vec<String>,
) -> Result<Option<classify::Placement>> {
    let Some(median) = market.median() else {
        return Ok(None);
    };
    // The peer group is whatever category was asked for, or the one the model lives in.
    let (product_type, category, label) = match selector {
        Some(selector) => (
            selector.product_type.clone(),
            selector.category.clone(),
            selector.label.clone(),
        ),
        None => {
            let Some(slug) = model
                .as_ref()
                .map(|model| model.root_category_slug.clone())
                .filter(|slug| !slug.is_empty())
            else {
                warnings.push(
                    "no category to compare against, so no price class — pass --category".into(),
                );
                return Ok(None);
            };
            // A category the tree does not know is a reason to skip the class, not to
            // fail the report: the bands above it are measured and correct either way.
            // A category the *caller* typed still errors — that one is theirs to fix.
            match Taxonomy::load(client).and_then(|tree| tree.resolve(&slug)) {
                Ok(resolved) => (resolved.product_type, resolved.category, resolved.label),
                Err(error) => {
                    warnings.push(format!(
                        "no price class: cannot place {slug:?} in Reverb's category tree ({error})"
                    ));
                    return Ok(None);
                }
            }
        }
    };
    let peers = Search {
        product_type: Some(product_type),
        category,
        condition: filters.condition,
        region: filters.region.as_ref().map(|code| code.to_uppercase()),
        ..Search::default()
    };
    // The peer group is named with its condition, because "the 64th percentile of
    // Electric Guitars" means something different for used gear than for new.
    let label = format!("{} {label}", filters.condition.label());
    place(client, &peers, &label, median)
}

// ---------------------------------------------------------------------------
// models, listings, classes, categories
// ---------------------------------------------------------------------------

fn run_models(
    cli: &Cli,
    arguments: &ModelsArguments,
    client: &Client,
    progress: &Progress,
    style: Style,
) -> Result<()> {
    let words = arguments.query.join(" ").trim().to_string();
    if words.is_empty() {
        bail!("say what to look for, for example: gearprice models \"les paul standard\"");
    }
    progress.set("Searching the catalogue");
    let resolution = resolve::resolve(client, &words, arguments.limit.min(MAX_PER_PAGE))?;
    let mut warnings = Vec::new();
    if let Some(reading) = &resolution.interpreted_as {
        warnings.push(format!("read {words:?} as {reading:?}"));
    }
    let models = resolution.ranked;
    let report = ModelReport {
        query: words,
        source: SOURCE,
        generated_at: timestamp(),
        currency: client.currency().to_string(),
        models: models
            .iter()
            .map(|model| ModelRow::of(model, client.currency()))
            .collect(),
        diagnostics: Diagnostics::from(client.usage(), warnings),
    };
    match cli.format {
        Format::Table => output::print_models(&report, style),
        Format::Json => output::print_json(&report)?,
        Format::Csv => output::print_models_csv(&report)?,
    }
    Ok(())
}

fn run_listings(
    cli: &Cli,
    arguments: &ListingsArguments,
    client: &Client,
    progress: &Progress,
    style: Style,
) -> Result<()> {
    let words = arguments.query.join(" ").trim().to_string();
    let mut warnings = Vec::new();

    if words.is_empty() && arguments.model_id.is_none() && arguments.filters.category.is_none() {
        bail!("say what to list, for example: gearprice listings \"Boss DS-1\"");
    }
    progress.set("Resolving the model");
    let model = resolve_model(
        client,
        arguments.model_id,
        arguments.raw || words.is_empty(),
        &words,
        &mut warnings,
    )?
    .chosen;
    let mut search = match &model {
        Some(model) if !model.product_ids().is_empty() => Search::products(model.product_ids()),
        Some(model) => Search::text(model.title.clone()),
        None => Search::text(words.clone()),
    };
    search.sort = arguments.sort;
    apply_filters(&mut search, &arguments.filters, client)?;
    taxonomy::require_targeted(&search)?;

    // Bands need the distribution, so measuring comes first whenever a label is wanted.
    progress.set("Measuring the market");
    let market = measure(client, &search)?;
    warnings.extend(market.warnings.iter().cloned());
    let bands = Bands::of(&market);

    progress.set("Fetching listings");
    let chain = search
        .ships_to
        .as_ref()
        .map(|code| Regions::load(client).map(|regions| regions.chain(code)))
        .transpose()?;
    let wanted = arguments.limit.clamp(1, MAX_PER_PAGE);
    let mut listings = client.listings(&search, 1, wanted)?.listings;
    if let Some(chain) = &chain {
        listings.sort_by_key(|listing| {
            shipping::landed(listing, chain).unwrap_or_else(|| listing.amount())
        });
    }
    if arguments.deals {
        if let Some(bands) = &bands {
            listings.retain(|listing| {
                matches!(bands.band_for(listing.amount()), Band::Steal | Band::Low)
            });
            if listings.is_empty() {
                warnings.push("nothing is currently priced below the going rate".into());
            }
        } else {
            warnings.push("--deals needs a market to compare against; showing everything".into());
        }
    }

    let report = ListingReport {
        query: model
            .as_ref()
            .map(|model| model.title.clone())
            .unwrap_or_else(|| {
                if words.is_empty() {
                    search.describe()
                } else {
                    words
                }
            }),
        source: SOURCE,
        generated_at: timestamp(),
        currency: market.currency.clone(),
        condition: arguments.filters.condition.label().to_string(),
        matched: market.total,
        shown: listings.len(),
        delivered_to: search.ships_to.clone(),
        bands: bands.as_ref().map(|bands| BandTable::of(bands, &market)),
        listings: listings
            .iter()
            .map(|listing| {
                ListingSummary::of(
                    listing,
                    &market.currency,
                    bands.as_ref(),
                    chain.as_deref(),
                    model
                        .as_ref()
                        .map(|model| model.title.as_str())
                        .unwrap_or_default(),
                    Utc::now(),
                )
            })
            .collect(),
        diagnostics: Diagnostics::from(client.usage(), warnings),
    };
    match cli.format {
        Format::Table => output::print_listings(&report, style),
        Format::Json => output::print_json(&report)?,
        Format::Csv => output::print_listings_csv(&report)?,
    }
    Ok(())
}

fn run_classes(
    cli: &Cli,
    arguments: &ClassesArguments,
    client: &Client,
    progress: &Progress,
    style: Style,
) -> Result<()> {
    progress.set("Reading the category tree");
    let selector = Taxonomy::load(client)?.resolve(&arguments.category)?;
    let search = Search {
        product_type: Some(selector.product_type.clone()),
        category: selector.category.clone(),
        condition: arguments.condition,
        make: arguments.make.clone(),
        ..Search::default()
    };

    progress.set("Measuring the category");
    let market = measure_at(client, &search, &SEGMENT_PERCENTILES)?;
    if market.is_empty() {
        bail!("no {} listings in {}", arguments.condition, selector.label);
    }
    let classes = segment_ladder(&market)
        .into_iter()
        .map(|(segment, from, to)| {
            let (low, high) = segment.bounds();
            ClassRow {
                segment,
                from: from.map(|price| price.major(&market.currency)),
                to: to.map(|price| price.major(&market.currency)),
                share: high - low,
                description: segment.description().to_string(),
            }
        })
        .collect();

    let report = ClassReport {
        category: selector.label.clone(),
        source: SOURCE,
        generated_at: timestamp(),
        currency: market.currency.clone(),
        condition: arguments.condition.label().to_string(),
        listings: market.total,
        method: market.method.label(),
        classes,
        diagnostics: Diagnostics::from(client.usage(), Vec::new()),
    };
    match cli.format {
        Format::Table => output::print_classes(&report, style),
        Format::Json => output::print_json(&report)?,
        Format::Csv => output::print_classes_csv(&report)?,
    }
    Ok(())
}

/// Compares every catalogue entry in a family.
///
/// Reverb splits a model by era and by variant, and the differences are not cosmetic:
/// Marshall's guitar Major is Model 1967 and the bass Major is Model 1978, both called
/// "Major", both 200 watts, and their markets are nothing alike. Pricing one when you
/// hold the other is an easy and expensive mistake.
fn run_variants(
    cli: &Cli,
    arguments: &VariantsArguments,
    client: &Client,
    progress: &Progress,
    style: Style,
) -> Result<()> {
    let words = arguments.query.join(" ").trim().to_string();
    if words.is_empty() {
        bail!("say which model to compare versions of");
    }
    progress.set("Finding the versions");
    let resolution = resolve::resolve(client, &words, arguments.limit.min(MAX_PER_PAGE))?;
    let mut warnings = Vec::new();
    if let Some(reading) = &resolution.interpreted_as {
        warnings.push(format!("read {words:?} as {reading:?}"));
    }
    let models: Vec<CatalogueModel> = resolution
        .ranked
        .into_iter()
        .take(arguments.limit as usize)
        .collect();
    if models.is_empty() {
        bail!("nothing in Reverb's catalogue matches {words:?}");
    }

    progress.set("Reading what each has sold for");
    let currency = client.currency().to_string();
    let sold: Vec<Option<sold::Sold>> = models
        .par_iter()
        .map(|model| sold::read(client, model.id, arguments.sold_sample).ok())
        .collect();

    let variants: Vec<VariantRow> = models
        .iter()
        .zip(sold)
        .map(|(model, sold)| {
            let sold = sold.filter(|history| !history.is_empty());
            VariantRow {
                id: model.id,
                title: model.title.clone(),
                used_listings: model.used_total,
                new_listings: model.new_total,
                used_from: model
                    .used_low_price
                    .as_ref()
                    .map(|price| Money::from_minor(price.amount_cents).major(&currency)),
                sold_median: sold
                    .as_ref()
                    .and_then(sold::Sold::median)
                    .map(|price| price.major(&currency)),
                sold_recent_median: sold
                    .as_ref()
                    .and_then(|history| history.recent_median)
                    .map(|price| price.major(&currency)),
                sales: sold.as_ref().map(|history| history.recorded).unwrap_or(0),
                url: model.web_url(),
            }
        })
        .collect();

    let report = VariantReport {
        query: words,
        source: SOURCE,
        generated_at: timestamp(),
        currency,
        variants,
        diagnostics: Diagnostics::from(client.usage(), warnings),
    };
    match cli.format {
        Format::Json => output::print_json(&report)?,
        Format::Csv => output::print_variants_csv(&report)?,
        Format::Table => output::print_variants(&report, style),
    }
    Ok(())
}

/// Judges one listing against what its model actually sells for.
fn run_deal(
    cli: &Cli,
    arguments: &DealArguments,
    client: &Client,
    progress: &Progress,
    style: Style,
) -> Result<()> {
    let id = match target::parse(&arguments.listing) {
        target::Target::Listing(id) => id,
        // A bare number is words everywhere else, because model names are full of them.
        // Here the argument is a listing by definition, so a number is an id.
        target::Target::Words(words) => words
            .trim()
            .parse()
            .with_context(|| format!("{words:?} is not a Reverb listing address or id"))?,
        _ => {
            bail!("that is a model rather than a listing — `gearprice price` takes a model address")
        }
    };

    progress.set("Fetching the listing");
    let listing = client
        .listing(id)
        .with_context(|| format!("no Reverb listing with id {id}"))?;
    let Some(model_id) = listing.model_id() else {
        bail!(
            "Reverb has not matched {:?} to a catalogue model, so there is nothing to \
             compare it against. `gearprice price {:?}` will price the words instead.",
            listing.describe(),
            listing.describe()
        )
    };
    let model = client.model(model_id)?;

    progress.set("Reading what it has sold for");
    let sold = sold::read(client, model_id, arguments.sold_sample)?;

    let mut search = Search::products(model.product_ids());
    let mut filters = arguments.filters.clone();
    // Judged against its own condition where the record is thick enough to support it,
    // because a mint example and a beaten one are not the same market.
    filters.condition = Condition::Used;
    apply_filters(&mut search, &filters, client)?;
    progress.set("Measuring the market");
    let market = measure(client, &search)?;

    let mut warnings = Vec::new();
    let bands = Bands::of_sold(&sold).or_else(|| Bands::of(&market));
    if bands.is_none() {
        bail!("nothing to compare this against — no sales on record and nothing listed");
    }
    if sold.is_empty() {
        warnings.push(
            "no sold record for this model, so this is judged against what other sellers \
             are asking rather than what anyone paid"
                .into(),
        );
    }
    let bands = bands.expect("checked above");
    let price = listing.amount();
    let band = bands.band_for(price);
    let against = match bands.basis {
        classify::Basis::Sold => sold.median(),
        classify::Basis::Asking => market.median(),
    };

    let report = DealReport {
        listing: ListingSummary::of(
            &listing,
            market.currency.as_str(),
            Some(&bands),
            None,
            &model.title,
            Utc::now(),
        ),
        source: SOURCE,
        generated_at: timestamp(),
        currency: market.currency.clone(),
        model: ModelSummary::of(&model),
        verdict: AskingVerdict {
            price: price.major(&market.currency),
            band,
            basis: bands.basis,
            verdict: band.verdict(bands.basis).to_string(),
            against_median: against
                .map(|against| price.major(&market.currency) - against.major(&market.currency))
                .unwrap_or_default(),
        },
        sold: (!sold.is_empty()).then(|| SoldSummary::of(&sold, &market)),
        bands: BandTable::of(&bands, &market),
        market: MarketSummary::of(&market),
        diagnostics: Diagnostics::from(client.usage(), warnings),
    };
    match cli.format {
        Format::Json => output::print_json(&report)?,
        _ => output::print_deal(&report, style),
    }
    Ok(())
}

fn run_track(
    cli: &Cli,
    arguments: &TrackArguments,
    client: &Client,
    progress: &Progress,
    style: Style,
) -> Result<()> {
    let history = History::new(history::default_path());
    if arguments.list {
        let report = TrackedReport {
            history: history.path().display().to_string(),
            tracked: history
                .subjects()?
                .into_iter()
                .map(|(subject, readings, last)| TrackedSubject {
                    title: subject.title,
                    model_id: subject.model_id,
                    currency: subject.currency,
                    condition: subject.condition.label().to_string(),
                    ships_to: subject.ships_to,
                    readings,
                    last_recorded: last.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                })
                .collect(),
        };
        match cli.format {
            Format::Json => output::print_json(&report)?,
            _ => output::print_tracked(&report, style),
        }
        return Ok(());
    }

    let words = arguments.query.join(" ").trim().to_string();
    if words.is_empty() && arguments.model_id.is_none() {
        bail!("say what to track, or pass --list to see what already is");
    }
    let mut warnings = Vec::new();

    progress.set("Resolving the model");
    let resolution = resolve_model(
        client,
        arguments.model_id,
        arguments.raw,
        &words,
        &mut warnings,
    )?;
    let model = resolution.chosen.clone();

    let mut search = match &model {
        Some(model) if !model.product_ids().is_empty() => Search::products(model.product_ids()),
        Some(model) => Search::text(model.title.clone()),
        None => Search::text(words.clone()),
    };
    apply_filters(&mut search, &arguments.filters, client)?;
    taxonomy::require_targeted(&search)?;

    progress.set("Measuring the market");
    let market = measure(client, &search)?;
    warnings.extend(market.warnings.iter().cloned());
    if market.is_empty() {
        bail!("nothing is listed, so there is no reading to record");
    }

    let subject = Subject {
        model_id: model.as_ref().map(|model| model.id),
        title: model
            .as_ref()
            .map(|model| model.title.clone())
            .unwrap_or_else(|| words.clone()),
        currency: market.currency.clone(),
        condition: arguments.filters.condition,
        ships_to: search.ships_to.clone(),
    };
    // Recorded alongside the asking median, so a history built now can answer "what was
    // this trading at in March" and not only "what were people hoping for".
    let sold = model
        .as_ref()
        .and_then(|model| sold::read(client, model.id, sold::TARGET_SAMPLE).ok())
        .filter(|history| !history.is_empty());
    let snapshot = Snapshot::of(subject.clone(), &market, sold.as_ref());
    if !arguments.no_record {
        history.record(&snapshot)?;
    }

    let mut readings = history.readings_of(&subject)?;
    if arguments.no_record {
        // Shown alongside the past, without joining it.
        readings.push(snapshot);
    }
    let movement = Movement::between(&readings);
    if readings.len() < 2 {
        warnings
            .push("first reading of this market — run this again in a week to see it move".into());
    }

    let report = TrackReport {
        query: if words.is_empty() {
            subject.title.clone()
        } else {
            words
        },
        source: SOURCE,
        generated_at: timestamp(),
        currency: market.currency.clone(),
        condition: arguments.filters.condition.label().to_string(),
        model: model.as_ref().map(ModelSummary::of),
        recorded: !arguments.no_record,
        readings: readings.iter().map(Reading::of).collect(),
        movement,
        diagnostics: Diagnostics::from(client.usage(), warnings),
    };
    match cli.format {
        Format::Json => output::print_json(&report)?,
        _ => output::print_track(&report, style),
    }
    Ok(())
}

fn run_categories(cli: &Cli, client: &Client, progress: &Progress, style: Style) -> Result<()> {
    progress.set("Reading the category tree");
    let taxonomy = Taxonomy::load(client)?;
    match cli.format {
        Format::Json => output::print_json(&serde_json::json!({
            "categories": taxonomy.roots().iter().map(|root| serde_json::json!({
                "slug": root.slug,
                "name": root.name,
                "subcategories": root.subcategories.iter().map(|leaf| serde_json::json!({
                    "slug": leaf.slug,
                    "name": leaf.name,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>()
        }))?,
        _ => {
            for root in taxonomy.roots() {
                println!("{}", style_root(&root.slug, &root.name, style));
                for leaf in &root.subcategories {
                    println!("    {:<28} {}", leaf.slug, leaf.name);
                }
            }
        }
    }
    Ok(())
}

fn style_root(slug: &str, name: &str, _style: Style) -> String {
    format!("{slug:<32} {name}")
}

// ---------------------------------------------------------------------------
// cache and update
// ---------------------------------------------------------------------------

fn run_cache(arguments: &CacheArguments, cli: &Cli) -> Result<()> {
    let cache = cache::ResponseCache::new(
        paths::response_cache_dir(),
        Duration::from_secs(cli.cache_ttl.saturating_mul(60)),
    );
    if arguments.clear {
        let removed = cache.clear()?;
        println!(
            "Cleared {removed} cached responses from {}",
            cache.directory().display()
        );
        return Ok(());
    }
    let (entries, bytes) = cache.usage();
    println!("Cache      {}", cache.directory().display());
    println!("Entries    {}", output::number(entries as u32));
    println!("Size       {:.1} MB", bytes as f64 / 1_048_576.0);
    println!("Good for   {} minutes after each response", cli.cache_ttl);
    Ok(())
}

fn run_update(arguments: &UpdateArguments) -> Result<()> {
    let path = paths::update_check_path();
    if arguments.check {
        let outcome = update::check_now(&path)?;
        if outcome.available {
            println!(
                "gearprice {} is available — you have {}. Install it with `gearprice update`.",
                outcome.latest, outcome.current
            );
        } else {
            println!("gearprice {} is the latest release.", outcome.current);
        }
        return Ok(());
    }
    let outcome = update::install_latest(&path)?;
    if outcome.available {
        println!(
            "Updated gearprice {} → {}.",
            outcome.current, outcome.latest
        );
    } else {
        println!(
            "gearprice {} is already the latest release.",
            outcome.current
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use classify::Segment;

    #[test]
    fn the_gear_can_come_first_without_naming_the_command() {
        let filled = |arguments: &[&str]| {
            with_default_command(arguments.iter().map(|value| value.to_string()))
        };
        assert_eq!(
            vec!["gearprice", "price", "les paul"],
            filled(&["gearprice", "les paul"])
        );
        // A subcommand is left alone, aliases included.
        assert_eq!(
            vec!["gearprice", "models", "les paul"],
            filled(&["gearprice", "models", "les paul"])
        );
        assert_eq!(
            vec!["gearprice", "search", "les paul"],
            filled(&["gearprice", "search", "les paul"])
        );
        // So is anything starting with a flag, because working out where the query is
        // would mean knowing which options take a value.
        assert_eq!(
            vec!["gearprice", "--help"],
            filled(&["gearprice", "--help"])
        );
        assert_eq!(vec!["gearprice"], filled(&["gearprice"]));
    }

    #[test]
    fn the_command_line_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_currency_that_is_not_a_code_is_refused_before_any_request() {
        let cli = Cli::parse_from(["gearprice", "--currency", "dollars", "categories"]);
        let error = match build_client(&cli) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("expected a currency that is not a code to be refused"),
        };
        assert!(error.contains("three-letter code"), "{error}");
        // A lowercase code is a spelling, not a mistake.
        let cli = Cli::parse_from(["gearprice", "--currency", "nok", "categories"]);
        let client = build_client(&cli).expect("a lowercase code is valid");
        assert_eq!("NOK", client.currency());
    }

    #[test]
    fn price_defaults_to_used_gear_and_the_query_is_joined() {
        let cli = Cli::parse_from(["gearprice", "price", "Gibson", "Les", "Paul"]);
        let Command::Price(arguments) = cli.command else {
            panic!("expected the price command")
        };
        assert_eq!("Gibson Les Paul", arguments.query.join(" "));
        assert_eq!(Condition::Used, arguments.filters.condition);
        assert!(!arguments.raw);
    }

    #[test]
    fn a_model_id_and_raw_text_are_mutually_exclusive() {
        let clash = Cli::try_parse_from(["gearprice", "price", "--model-id", "1", "--raw"]);
        assert!(clash.is_err());
    }

    #[test]
    fn segment_and_band_names_are_stable_identifiers() {
        // These appear in JSON, CSV and scripts. Renaming one is a breaking change.
        assert_eq!(
            vec!["steal", "low", "fair", "high", "premium"],
            Band::ALL.iter().map(|band| band.name()).collect::<Vec<_>>()
        );
        assert_eq!(
            vec!["entry", "mid", "pro", "premium", "boutique"],
            Segment::ALL
                .iter()
                .map(|segment| segment.name())
                .collect::<Vec<_>>()
        );
    }
}
