//! Rendering reports as tables, JSON or CSV.
//!
//! The table is the one a person reads, so it leads with the answer — the class, the
//! going rate, the verdict on an asking price — and puts the evidence under it. JSON and
//! CSV are the same report without the typography.

use std::io::{self, IsTerminal, Write};

use crate::classify::{Band, Basis, Segment};
use crate::money::{self, Money};
use crate::report::{
    BandTable, ClassReport, DealReport, ListingReport, ListingSummary, ModelReport, PriceReport,
    SOURCE_SHORT, TrackReport, TrackedReport, VariantReport,
};

/// The width tables are drawn to.
///
/// Measured from the terminal rather than fixed. A fixed rule either wastes half a wide
/// window or overflows a narrow one, and overflow is the worse of the two: a wrapped row
/// destroys the column alignment that makes a table readable at all.
///
/// Bounded at both ends. Below the lower bound the columns cannot fit whatever is done,
/// and above the upper one the eye loses the row on the way across.
fn rule_width() -> usize {
    const NARROWEST: usize = 60;
    const WIDEST: usize = 110;
    terminal_size::terminal_size()
        .map(|(terminal_size::Width(columns), _)| usize::from(columns).saturating_sub(1))
        .unwrap_or(92)
        .clamp(NARROWEST, WIDEST)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
pub enum Format {
    #[default]
    Table,
    Json,
    Csv,
}

#[derive(Clone, Copy)]
pub struct Style {
    color: bool,
}

impl Style {
    pub fn new(disabled: bool) -> Self {
        Self {
            color: !disabled
                && std::env::var_os("NO_COLOR").is_none()
                && io::stdout().is_terminal(),
        }
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    fn band(self, band: Band, text: &str) -> String {
        // Cheap is green, dear is red — the direction a buyer reads them in.
        self.paint(
            match band {
                Band::Steal => "32;1",
                Band::Low => "32",
                Band::Fair => "36",
                Band::High => "33",
                Band::Premium => "31",
            },
            text,
        )
    }

    fn segment(self, segment: Segment, text: &str) -> String {
        self.paint(
            match segment {
                Segment::Entry => "32",
                Segment::Mid => "36",
                Segment::Pro => "34",
                Segment::Premium => "33",
                Segment::Boutique => "35",
            },
            text,
        )
    }
}

pub fn print_json(out: &mut impl Write, value: &impl serde::Serialize) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, value)?;
    writeln!(out)
}

// ---------------------------------------------------------------------------
// Price
// ---------------------------------------------------------------------------

/// The price report.
///
/// `full` restores the parts trimmed by default. The trimmed ones — the histogram above
/// all — describe the *asking* prices, which are the secondary number now that bands come
/// from completed sales, and the report ran to sixty lines with them in.
pub fn print_price(
    out: &mut impl Write,
    report: &PriceReport,
    style: Style,
    full: bool,
) -> io::Result<()> {
    let currency = report.currency.as_str();

    let heading = report
        .model
        .as_ref()
        .map(|model| model.title.clone())
        .unwrap_or_else(|| report.query.clone());
    writeln!(out, "{}", style.bold(&format!("GEARPRICE  {heading}")))?;
    writeln!(out, "{}", style.dim(&"═".repeat(rule_width())))?;

    if let Some(model) = &report.model {
        let mut parts = vec![model.brand.clone()];
        if !model.category.is_empty() {
            parts.push(model.category.clone());
        }
        parts.retain(|part| !part.is_empty());
        writeln!(out, "  {:<21} {}", "Model", parts.join(" · "))?;
        writeln!(out, "  {:<21} {}", "", style.dim(&model.url))?;
    }
    if let Some(sold) = &report.sold {
        let covering = sold
            .covering
            .as_ref()
            .map(|span| format!(" · {span}"))
            .unwrap_or_default();
        writeln!(
            out,
            "  {:<21} {:>10}  {}",
            style.bold("Sold"),
            price_or_dash(sold.median, currency),
            style.dim(&format!(
                "median of {} {}{covering}",
                number(sold.read as u32),
                if sold.read == 1 { "sale" } else { "sales" }
            ))
        )?;
        // Quoted only when the whole record reaches back far enough to be describing a
        // different market than the one somebody is buying in today.
        if let Some(recent) = sold.recent_median {
            writeln!(
                out,
                "  {:<21} {:>10}  {}",
                "Sold, last year",
                price(recent, currency),
                style.dim(&format!(
                    "median of the {} most recent — read this one, not the figure above",
                    number(sold.recent_sales as u32)
                ))
            )?;
        }
        writeln!(
            out,
            "  {:<21} {:>10}  {}",
            "Asking",
            report
                .market
                .percentiles
                .iter()
                .find(|point| point.percentile == 50)
                .map(|point| price(point.price, currency))
                .unwrap_or_else(|| "—".to_string()),
            style.dim(&match (sold.asking_premium, sold.asking_percentile) {
                (Some(premium), Some(percentile)) => format!(
                    "{:+.0}% above sold · {} percentile of what was paid",
                    premium * 100.0,
                    ordinal(percentile)
                ),
                _ => format!("{} listings on the market", number(report.market.listings)),
            })
        )?;
        if let Some(discount) = sold.typical_discount {
            writeln!(
                out,
                "  {:<21} {:>10}  {}",
                "Room",
                format!("{:.0}%", discount * 100.0),
                style.dim(&format!(
                    "typical, but {} of {} went at or above the ask",
                    number(sold.sold_at_or_above_ask as u32),
                    number(sold.sales_with_both_prices as u32)
                ))
            )?;
        }
    } else {
        writeln!(
            out,
            "  {:<21} {} {} listings · {} · {}",
            "Market",
            number(report.market.listings),
            report.condition,
            currency,
            style.dim(SOURCE_SHORT)
        )?;
    }
    if let Some(destination) = &report.delivered_to {
        writeln!(
            out,
            "  {:<21} {} {}",
            "Ships to",
            destination,
            style.dim("· only sellers who send there, priced delivered")
        )?;
    }
    writeln!(
        out,
        "  {:<21} {}",
        "Method",
        style.dim(&report.market.method)
    )?;
    if let Some(reading) = &report.interpreted_as {
        writeln!(
            out,
            "  {:<21} {} {}",
            "Read as",
            reading,
            style.dim(&format!("(you typed “{}”)", report.query))
        )?;
    }
    print_warnings(out, &report.diagnostics, style)?;

    if !report.alternatives.is_empty() {
        writeln!(out)?;
        writeln!(
            out,
            "  {:<21} {}",
            "Close matches",
            style.dim("these fit what you typed about as well — price one with --model-id")
        )?;
        for other in &report.alternatives {
            writeln!(
                out,
                "  {:<21} {:<36} {}",
                "",
                fit(&other.title, 56),
                style.dim(&format!(
                    "--model-id {:<9} {} used",
                    other.id,
                    number(other.used_listings)
                ))
            )?;
        }
    }

    if let Some(class) = &report.class {
        writeln!(out)?;
        writeln!(
            out,
            "  {:<21} {}  {}",
            "Class",
            style.segment(class.segment, &class.segment.name().to_uppercase()),
            class.description
        )?;
        writeln!(
            out,
            "  {:<21} {}",
            "",
            style.dim(&format!(
                "{} percentile of {} {} listings",
                ordinal(class.percentile),
                number(class.peer_listings),
                class.peer_group
            ))
        )?;
    }

    if let Some(asking) = &report.asking {
        writeln!(out)?;
        writeln!(
            out,
            "  {:<21} {}  {}",
            format!("Asking {}", price(asking.price, currency)),
            style.band(asking.band, &asking.band.name().to_uppercase()),
            asking.verdict
        )?;
        let gap = asking.against_median;
        let direction = if gap < 0.0 { "below" } else { "above" };
        writeln!(
            out,
            "  {:<21} {}",
            "",
            style.dim(&format!(
                "{} {direction} the median {}",
                price(gap.abs(), currency),
                match asking.basis {
                    Basis::Sold => "price people paid",
                    Basis::Asking => "asking price",
                }
            ))
        )?;
    }

    if let Some(bands) = &report.bands {
        writeln!(out)?;
        print_bands(out, bands, currency, style)?;
    }

    if !report.market.percentiles.is_empty() && report.market.listings > 0 {
        print_distribution(out, report, currency, style, full)?;
    }

    let (grades, grade_heading) = match report.sold.as_ref().filter(|sold| !sold.grades.is_empty())
    {
        Some(sold) => (
            &sold.grades,
            "What condition is worth  (median price paid, best grade first)",
        ),
        None => (
            &report.market.grades,
            "Asking price by condition  (median, best grade first)",
        ),
    };
    if !grades.is_empty() {
        writeln!(out)?;
        writeln!(out, "{}", style.bold(grade_heading))?;
        for grade in grades {
            let against = match grade.against_median {
                Some(difference) if difference.abs() >= 1.0 => {
                    let sign = if difference < 0.0 { "−" } else { "+" };
                    style.dim(&format!(
                        "{sign}{} against the median",
                        price(difference.abs(), currency)
                    ))
                }
                _ => String::new(),
            };
            writeln!(
                out,
                "  {:<21} {:>5} {:<9}{:>12}  {against}",
                grade.grade,
                number(grade.listings),
                if report.sold.is_some() {
                    "sales"
                } else {
                    "listings"
                },
                price(grade.median, currency),
            )?;
        }
    }

    if let Some(share) = report.market.sellers_taking_offers {
        writeln!(out)?;
        writeln!(
            out,
            "  {} of these sellers accept offers{}",
            percent(share),
            if share >= 0.5 {
                " — treat the asking prices above as a starting point"
            } else {
                ""
            }
        )?;
    }

    if !report.listings.is_empty() {
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            style.bold(if report.delivered_to.is_some() {
                "Cheapest delivered"
            } else {
                "Cheapest listings"
            })
        )?;
        print_listing_rows(out, &report.listings, currency, style)?;
    }

    print_usage(out, &report.diagnostics, style)
}

fn print_bands(
    out: &mut impl Write,
    bands: &BandTable,
    currency: &str,
    style: Style,
) -> io::Result<()> {
    let timed = bands.bands.iter().any(|row| row.days_listed.is_some());
    let paired = bands
        .bands
        .iter()
        .any(|row| row.asking_from.is_some() || row.asking_to.is_some());
    writeln!(
        out,
        "{}",
        style.bold(match bands.basis {
            Basis::Sold => "What people actually paid",
            Basis::Asking if timed =>
                "Price bands  (what sellers are asking, and whether anyone is paying it)",
            Basis::Asking => "Price bands  (what sellers are asking for this model right now)",
        })
    )?;
    if paired {
        writeln!(
            out,
            "  {:<9} {:>25} {:>7} {:>25}",
            "Band", "Sold range", "Share", "Asking equivalent"
        )?;
    } else if timed {
        writeln!(
            out,
            "  {:<9} {:>25} {:>7} {:>13}  Meaning",
            "Band", "Range", "Share", "Listed for"
        )?;
    } else {
        writeln!(
            out,
            "  {:<9} {:>25} {:>7}  Meaning",
            "Band", "Range", "Share"
        )?;
    }
    writeln!(out, "  {}", style.dim(&"─".repeat(rule_width() - 2)))?;
    let coarse = coarse_column(
        bands
            .bands
            .iter()
            .flat_map(|row| [row.from, row.to, row.asking_from, row.asking_to]),
    );
    for row in &bands.bands {
        let range = price_range(row.from, row.to, currency, coarse);
        let band = Band::ALL
            .into_iter()
            .find(|band| band.name() == row.band)
            .unwrap_or(Band::Fair);
        if paired {
            // No trailing verdict here: with both price columns the row is already at the
            // rule, and the band name and the caption below say what it means.
            writeln!(
                out,
                "  {:<9} {:>25} {:>7} {:>25}",
                style.band(band, &row.band),
                range,
                format!("{:.0}%", row.share * 100.0),
                price_range(row.asking_from, row.asking_to, currency, coarse),
            )?;
        } else if timed {
            writeln!(
                out,
                "  {:<9} {:>25} {:>7} {:>13}  {}",
                style.band(band, &row.band),
                range,
                format!("{:.0}%", row.share * 100.0),
                row.days_listed.map(days).unwrap_or_else(|| "—".into()),
                style.dim(&fit(&row.verdict, 60))
            )?;
        } else {
            writeln!(
                out,
                "  {:<9} {:>25} {:>7}  {}",
                style.band(band, &row.band),
                range,
                format!("{:.0}%", row.share * 100.0),
                style.dim(&fit(&row.verdict, 46))
            )?;
        }
    }
    if paired {
        writeln!(
            out,
            "  {}",
            style.dim(
                "Bands are cut from what people paid. The asking column is the listing price \
                 at the"
            )
        )?;
        writeln!(
            out,
            "  {}",
            style.dim("same percentile — what a seller wants for gear that trades in that band.")
        )?;
    } else if timed {
        writeln!(
            out,
            "  {}",
            style.dim(
                "Listed for = median days these have been up. Cheap listings leave because \
                 they sell;"
            )
        )?;
        writeln!(
            out,
            "  {}",
            style.dim("dear ones pile up, so a long wait is a price the market is not paying.")
        )?;
    }
    Ok(())
}

/// `12 days`, `4 months`, `2.7 years` — the unit a reader thinks in at that scale.
fn days(count: i64) -> String {
    match count {
        ..=1 => format!("{count} day"),
        2..=89 => format!("{count} days"),
        90..=545 => format!("{} months", (count as f64 / 30.44).round() as i64),
        _ => format!("{:.1} years", count as f64 / 365.25),
    }
}

fn print_distribution(
    out: &mut impl Write,
    report: &PriceReport,
    currency: &str,
    style: Style,
    full: bool,
) -> io::Result<()> {
    writeln!(out)?;
    writeln!(out, "{}", style.bold("Distribution"))?;
    let mut cells = vec![
        ("low".to_string(), report.market.cheapest),
        ("p25".to_string(), 0.0),
        ("median".to_string(), 0.0),
        ("p75".to_string(), 0.0),
        ("high".to_string(), report.market.dearest),
    ];
    for point in &report.market.percentiles {
        match point.percentile {
            25 => cells[1].1 = point.price,
            50 => cells[2].1 = point.price,
            75 => cells[3].1 = point.price,
            _ => {}
        }
    }
    if let Some(mean) = report.market.mean {
        cells.push(("mean".to_string(), mean));
    }
    let labels: Vec<String> = cells
        .iter()
        .map(|(label, _)| format!("{label:>12}"))
        .collect();
    let values: Vec<String> = cells
        .iter()
        .map(|(_, value)| format!("{:>12}", price(*value, currency)))
        .collect();
    writeln!(out, "  {}", style.dim(&labels.join(" ")))?;
    writeln!(out, "  {}", values.join(" "))?;

    if !report.market.histogram.is_empty() && full {
        writeln!(out)?;
        writeln!(
            out,
            "  {}",
            style.dim("Where the listings sit  (middle 90% — the tails are in low and high above)")
        )?;
        let widest = report
            .market
            .histogram
            .iter()
            .map(|band| band.listings)
            .max()
            .unwrap_or(1)
            .max(1);
        for band in &report.market.histogram {
            writeln!(
                out,
                "  {:>11} – {:<11} {:>6}  {}",
                price(band.from, currency),
                price(band.to, currency),
                number(band.listings),
                bar(band.listings, widest, 30)
            )?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Models, classes, listings
// ---------------------------------------------------------------------------

pub fn print_models(out: &mut impl Write, report: &ModelReport, style: Style) -> io::Result<()> {
    let currency = report.currency.as_str();
    writeln!(
        out,
        "{}",
        style.bold(&format!("GEARPRICE  models matching “{}”", report.query))
    )?;
    writeln!(out, "{}", style.dim(&"═".repeat(rule_width())))?;
    if report.models.is_empty() {
        writeln!(
            out,
            "  No catalogue model matches that. Try fewer words, or a different spelling."
        )?;
        return print_usage(out, &report.diagnostics, style);
    }
    writeln!(
        out,
        "  {:<48} {:>6} {:>11} {:>6} {:>11}",
        "Model", "Used", "Used from", "New", "New from"
    )?;
    writeln!(out, "  {}", style.dim(&"─".repeat(rule_width() - 2)))?;
    for model in &report.models {
        writeln!(
            out,
            "  {:<48} {:>6} {:>11} {:>6} {:>11}",
            truncate(&model.title, 48),
            number(model.used_listings),
            price_or_dash(model.used_from, currency),
            number(model.new_listings),
            price_or_dash(model.new_from, currency),
        )?;
    }
    writeln!(out)?;
    writeln!(
        out,
        "  {}",
        style.dim("Price any of these exactly:  gearprice price --model-id <ID>")
    )?;
    for model in report.models.iter().take(3) {
        writeln!(
            out,
            "  {}",
            style.dim(&format!(
                "  {:<10} {}",
                model.id,
                truncate(&model.title, 60)
            ))
        )?;
    }
    print_usage(out, &report.diagnostics, style)
}

pub fn print_classes(out: &mut impl Write, report: &ClassReport, style: Style) -> io::Result<()> {
    let currency = report.currency.as_str();
    writeln!(
        out,
        "{}",
        style.bold(&format!("GEARPRICE  price classes of {}", report.category))
    )?;
    writeln!(out, "{}", style.dim(&"═".repeat(rule_width())))?;
    writeln!(
        out,
        "  {:<21} {} {} listings · {} · {}",
        "Market",
        number(report.listings),
        report.condition,
        currency,
        style.dim(SOURCE_SHORT)
    )?;
    writeln!(out, "  {:<21} {}", "Method", style.dim(&report.method))?;
    print_warnings(out, &report.diagnostics, style)?;
    writeln!(out)?;
    writeln!(
        out,
        "  {:<10} {:>27} {:>7}  Meaning",
        "Class", "Range", "Share"
    )?;
    writeln!(out, "  {}", style.dim(&"─".repeat(rule_width() - 2)))?;
    let coarse = coarse_column(report.classes.iter().flat_map(|row| [row.from, row.to]));
    for row in &report.classes {
        let range = price_range(row.from, row.to, currency, coarse);
        writeln!(
            out,
            "  {:<10} {:>27} {:>7}  {}",
            style.segment(row.segment, row.segment.name()),
            range,
            format!("{:.0}%", row.share * 100.0),
            style.dim(&row.description)
        )?;
    }
    print_usage(out, &report.diagnostics, style)
}

pub fn print_listings(
    out: &mut impl Write,
    report: &ListingReport,
    style: Style,
) -> io::Result<()> {
    let currency = report.currency.as_str();
    writeln!(
        out,
        "{}",
        style.bold(&format!("GEARPRICE  listings for “{}”", report.query))
    )?;
    writeln!(out, "{}", style.dim(&"═".repeat(rule_width())))?;
    writeln!(
        out,
        "  {:<21} {} of {} {} listings · {} · {}",
        "Showing",
        number(report.shown as u32),
        number(report.matched),
        report.condition,
        currency,
        style.dim(SOURCE_SHORT)
    )?;
    if let Some(destination) = &report.delivered_to {
        writeln!(
            out,
            "  {:<21} {} {}",
            "Ships to",
            destination,
            style.dim("· only sellers who send there, priced delivered")
        )?;
    }
    print_warnings(out, &report.diagnostics, style)?;
    if let Some(bands) = &report.bands {
        writeln!(out)?;
        print_bands(out, bands, currency, style)?;
    }
    writeln!(out)?;
    print_listing_rows(out, &report.listings, currency, style)?;
    print_usage(out, &report.diagnostics, style)
}

fn print_listing_rows(
    out: &mut impl Write,
    listings: &[ListingSummary],
    currency: &str,
    style: Style,
) -> io::Result<()> {
    if listings.is_empty() {
        return writeln!(out, "  Nothing listed right now.");
    }
    let delivered = listings.iter().any(|listing| listing.shipping.is_some());
    // Per listing rather than per band, because a market too thin for a band median is
    // exactly the one where "this has been sitting for nineteen months" matters most.
    let aged = listings.iter().any(|listing| listing.days_listed.is_some());
    for listing in listings {
        let label = match listing.band {
            Some(band) => style.band(band, &format!("{:<8}", band.name())),
            None => " ".repeat(8),
        };
        let landed = if delivered {
            match (listing.delivered, listing.shipping) {
                (Some(total), Some(carriage)) => format!(
                    "{:>11} {}",
                    price(total, currency),
                    style.dim(&format!("(+{} post)", price(carriage, currency)))
                ),
                // Said rather than left blank: a seller who has the carrier price it at
                // checkout has not told anyone what delivery costs.
                _ => format!("{:>11} {}", "—", style.dim("(post at checkout)")),
            }
        } else {
            String::new()
        };
        let waiting = match listing.days_listed.filter(|_| aged) {
            Some(count) => format!("  {:>10}", days(count)),
            None if aged => format!("  {:>10}", ""),
            None => String::new(),
        };
        writeln!(
            out,
            "  {:>11}  {label}  {:<14} {}{waiting}",
            price(listing.price, currency),
            truncate(&listing.condition, 14),
            // Price, band label, condition and the age column all take fixed room; the
            // title gets whatever is left.
            fit(
                &listing.title,
                42 + usize::from(delivered) * 22 + usize::from(aged) * 12,
            )
        )?;
        if delivered {
            writeln!(out, "  {:>11}            {landed}", "")?;
        }
        if let Some(url) = &listing.url {
            let shop = if listing.shop.is_empty() {
                String::new()
            } else {
                format!("{} · ", listing.shop)
            };
            writeln!(
                out,
                "  {:>11}            {}",
                "",
                style.dim(&format!("{shop}{url}"))
            )?;
        }
    }
    Ok(())
}

/// Warnings go first, because they change how the numbers under them should be read.
///
/// "No catalogue model matched, so this prices the words themselves" is not a footnote:
/// a reader who sees it after the bands has already believed them.
fn print_warnings(
    out: &mut impl Write,
    diagnostics: &crate::report::Diagnostics,
    style: Style,
) -> io::Result<()> {
    if diagnostics.warnings.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    for warning in &diagnostics.warnings {
        for (index, line) in wrap(warning, rule_width() - 6).into_iter().enumerate() {
            let marker = if index == 0 {
                style.paint("33;1", "!")
            } else {
                " ".to_string()
            };
            writeln!(out, "  {marker} {}", style.paint("33", &line))?;
        }
    }
    Ok(())
}

/// Wraps on whitespace at `width`, so a warning stays inside the rule the tables use.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let projected = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };
        if projected > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn print_usage(
    out: &mut impl Write,
    diagnostics: &crate::report::Diagnostics,
    style: Style,
) -> io::Result<()> {
    writeln!(out)?;
    writeln!(
        out,
        "{}",
        style.dim(&format!(
            "  {} API requests, {} served from cache",
            diagnostics.requests, diagnostics.cache_hits
        ))
    )
}

pub fn print_variants(
    out: &mut impl Write,
    report: &VariantReport,
    style: Style,
) -> io::Result<()> {
    let currency = report.currency.as_str();
    writeln!(
        out,
        "{}",
        style.bold(&format!("GEARPRICE  versions of “{}”", report.query))
    )?;
    writeln!(out, "{}", style.dim(&"═".repeat(rule_width())))?;
    print_warnings(out, &report.diagnostics, style)?;
    writeln!(
        out,
        "  {:<44} {:>6} {:>11} {:>11} {:>7}",
        "Version", "Used", "Asking from", "Sold", "Sales"
    )?;
    writeln!(out, "  {}", style.dim(&"─".repeat(rule_width() - 2)))?;
    for variant in &report.variants {
        // The recent median where the record is long, since that is the one to act on.
        let sold = price_or_dash(variant.sold_recent_median.or(variant.sold_median), currency);
        writeln!(
            out,
            "  {:<44} {:>6} {:>11} {:>11} {:>7}",
            truncate(&variant.title, 44),
            number(variant.used_listings),
            price_or_dash(variant.used_from, currency),
            sold,
            number(variant.sales),
        )?;
    }
    writeln!(out)?;
    writeln!(
        out,
        "  {}",
        style.dim("Sold is the last year where a version has that much history, else all of it.")
    )?;
    writeln!(
        out,
        "  {}",
        style.dim("Price one exactly:  gearprice price --model-id <ID>")
    )?;
    for variant in report.variants.iter().take(3) {
        writeln!(
            out,
            "  {}",
            style.dim(&format!(
                "  {:<10} {}",
                variant.id,
                truncate(&variant.title, 58)
            ))
        )?;
    }
    print_usage(out, &report.diagnostics, style)
}

pub fn print_variants_csv(out: &mut impl Write, report: &VariantReport) -> io::Result<()> {
    let mut writer = csv::Writer::from_writer(&mut *out);
    writer.write_record([
        "id",
        "title",
        "currency",
        "used_listings",
        "new_listings",
        "asking_from",
        "sold_median",
        "sold_recent_median",
        "sales",
        "url",
    ])?;
    for variant in &report.variants {
        writer.write_record([
            variant.id.to_string(),
            neutralize(&variant.title),
            report.currency.clone(),
            variant.used_listings.to_string(),
            variant.new_listings.to_string(),
            variant
                .used_from
                .map(|v| format!("{v:.2}"))
                .unwrap_or_default(),
            variant
                .sold_median
                .map(|v| format!("{v:.2}"))
                .unwrap_or_default(),
            variant
                .sold_recent_median
                .map(|v| format!("{v:.2}"))
                .unwrap_or_default(),
            variant.sales.to_string(),
            variant.url.clone(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

/// One listing, judged. The answer goes first, because it is the only thing being asked.
pub fn print_deal(out: &mut impl Write, report: &DealReport, style: Style) -> io::Result<()> {
    let currency = report.currency.as_str();
    let verdict = &report.verdict;
    writeln!(
        out,
        "{}",
        style.bold(&format!(
            "GEARPRICE  {}",
            truncate(&report.listing.title, 70)
        ))
    )?;
    writeln!(out, "{}", style.dim(&"═".repeat(rule_width())))?;
    writeln!(
        out,
        "  {:<21} {:>10}  {}",
        "Asking",
        price(verdict.price, currency),
        style.band(verdict.band, &verdict.band.name().to_uppercase())
    )?;
    writeln!(
        out,
        "  {:<21} {:>10}  {}",
        "",
        "",
        style.dim(&verdict.verdict)
    )?;

    if let Some(sold) = &report.sold {
        let headline = sold.recent_median.or(sold.median);
        if let Some(median) = headline {
            let gap = verdict.price - median;
            let direction = if gap < 0.0 { "below" } else { "above" };
            let colour = if gap < 0.0 { "32" } else { "31" };
            writeln!(
                out,
                "  {:<21} {:>10}  {}",
                if sold.recent_median.is_some() {
                    "Sold, last year"
                } else {
                    "Sold"
                },
                price(median, currency),
                style.paint(
                    colour,
                    &format!("this one is {} {direction}", price(gap.abs(), currency))
                )
            )?;
        }
        if let Some(discount) = sold.typical_discount {
            writeln!(
                out,
                "  {:<21} {:>10}  {}",
                "Room",
                format!("{:.0}%", discount * 100.0),
                style.dim(&format!(
                    "typical off the ask · {} of {} went at or above it",
                    number(sold.sold_at_or_above_ask as u32),
                    number(sold.sales_with_both_prices as u32)
                ))
            )?;
            // The number somebody about to make an offer actually wants.
            let target = verdict.price * (1.0 + discount);
            writeln!(
                out,
                "  {:<21} {:>10}  {}",
                "An offer at",
                price(target, currency),
                style.dim("would be the usual discount off this asking price")
            )?;
        }
    }
    if let Some(waiting) = report.listing.days_listed {
        writeln!(
            out,
            "  {:<21} {:>10}  {}",
            "Listed for",
            days(waiting),
            style.dim(if waiting > 90 {
                "a long wait at this price"
            } else {
                ""
            })
        )?;
    }
    writeln!(
        out,
        "  {:<21} {}",
        "Model",
        style.dim(&truncate(&report.model.title, 62))
    )?;
    print_warnings(out, &report.diagnostics, style)?;

    writeln!(out)?;
    print_bands(out, &report.bands, currency, style)?;
    if let Some(url) = &report.listing.url {
        writeln!(out)?;
        writeln!(out, "  {}", style.dim(url))?;
    }
    print_usage(out, &report.diagnostics, style)
}

pub fn print_track(out: &mut impl Write, report: &TrackReport, style: Style) -> io::Result<()> {
    let currency = report.currency.as_str();
    let heading = report
        .model
        .as_ref()
        .map(|model| model.title.clone())
        .unwrap_or_else(|| report.query.clone());
    writeln!(out, "{}", style.bold(&format!("GEARPRICE  {heading}")))?;
    writeln!(out, "{}", style.dim(&"═".repeat(rule_width())))?;
    writeln!(
        out,
        "  {:<21} {} · {} · {}",
        "Tracking",
        plural(report.readings.len(), "reading"),
        report.condition,
        currency
    )?;
    if !report.recorded {
        writeln!(
            out,
            "  {:<21} {}",
            "",
            style.dim("today's reading is shown but was not recorded (--no-record)")
        )?;
    }
    print_warnings(out, &report.diagnostics, style)?;

    if let Some(movement) = &report.movement {
        writeln!(out)?;
        let change = movement.median_change().unwrap_or_default();
        let share = movement.median_change_share().unwrap_or_default();
        let direction = if change < 0.0 { "down" } else { "up" };
        let colour = if change < 0.0 { "32" } else { "31" };
        writeln!(
            out,
            "  {:<21} {:>10}  {}",
            "Median",
            price(movement.median_now.unwrap_or_default(), currency),
            style.paint(
                colour,
                &format!(
                    "{direction} {} ({:.1}%) since {}",
                    price(change.abs(), currency),
                    share.abs() * 100.0,
                    local_date(&movement.since.to_rfc3339())
                )
            )
        )?;
        let listings = i64::from(movement.listings_now) - i64::from(movement.listings_then);
        writeln!(
            out,
            "  {:<21} {:>10}  {}",
            "Listings",
            number(movement.listings_now),
            style.dim(&format!("{listings:+} on the market since then"))
        )?;
    }

    writeln!(out)?;
    writeln!(out, "{}", style.bold("Readings"))?;
    writeln!(
        out,
        "  {:<12} {:>12} {:>10} {:>14}",
        "When", "Median", "Listings", "Typical wait"
    )?;
    writeln!(out, "  {}", style.dim(&"─".repeat(52)))?;
    let last = report.readings.len().saturating_sub(1);
    for (index, reading) in report.readings.iter().enumerate() {
        writeln!(
            out,
            "  {:<12} {:>12} {:>10} {:>14}{}",
            local_date(&reading.recorded_at),
            price_or_dash(reading.median, currency),
            number(reading.listings),
            reading
                .median_days_listed
                .map(days)
                .unwrap_or_else(|| "—".into()),
            if index == last {
                style.dim("  ← now")
            } else {
                String::new()
            }
        )?;
    }
    print_usage(out, &report.diagnostics, style)
}

/// The category tree, as roots with their leaves indented under them.
///
/// The root is the heading and the leaves are what a `--category` flag actually takes,
/// so the root is bolded and the slug column is what the eye runs down.
pub fn print_categories(
    out: &mut impl Write,
    roots: &[crate::reverb::Category],
    style: Style,
) -> io::Result<()> {
    for root in roots {
        writeln!(
            out,
            "{}",
            style.bold(&format!("{:<32} {}", root.slug, root.name))
        )?;
        for leaf in &root.subcategories {
            writeln!(out, "    {:<28} {}", leaf.slug, style.dim(&leaf.name))?;
        }
    }
    Ok(())
}

pub fn print_tracked(out: &mut impl Write, report: &TrackedReport, style: Style) -> io::Result<()> {
    writeln!(out, "{}", style.bold("GEARPRICE  what is being tracked"))?;
    writeln!(out, "{}", style.dim(&"═".repeat(rule_width())))?;
    if report.tracked.is_empty() {
        writeln!(
            out,
            "  Nothing yet. `gearprice track \"<gear>\"` takes the first reading."
        )?;
        return writeln!(out, "  {}", style.dim(&report.history));
    }
    writeln!(
        out,
        "  {:<44} {:>9} {:>9} {:>8} {:>12}",
        "Model", "Currency", "Condition", "Readings", "Last read"
    )?;
    writeln!(out, "  {}", style.dim(&"─".repeat(rule_width() - 2)))?;
    for subject in &report.tracked {
        writeln!(
            out,
            "  {:<44} {:>9} {:>9} {:>8} {:>12}",
            truncate(&subject.title, 44),
            subject.currency,
            truncate(&subject.condition, 9),
            number(subject.readings as u32),
            local_date(&subject.last_recorded)
        )?;
    }
    writeln!(out)?;
    writeln!(out, "  {}", style.dim(&report.history))
}

/// A timestamp as a plain date, for a column a person reads.
fn local_date(timestamp: &str) -> String {
    timestamp.split('T').next().unwrap_or(timestamp).to_string()
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

pub fn print_price_csv(out: &mut impl Write, report: &PriceReport) -> io::Result<()> {
    let mut writer = csv::Writer::from_writer(&mut *out);
    writer.write_record([
        "query",
        "model",
        "model_id",
        "currency",
        "condition",
        "listings",
        "metric",
        "value",
    ])?;
    let model = report
        .model
        .as_ref()
        .map(|model| model.title.clone())
        .unwrap_or_default();
    let model_id = report
        .model
        .as_ref()
        .map(|model| model.id.to_string())
        .unwrap_or_default();
    let mut row = |metric: &str, value: String| -> io::Result<()> {
        writer.write_record([
            neutralize(&report.query),
            neutralize(&model),
            model_id.clone(),
            report.currency.clone(),
            report.condition.clone(),
            report.market.listings.to_string(),
            metric.to_string(),
            value,
        ])?;
        Ok(())
    };
    if let Some(sold) = &report.sold {
        row("basis", "sold".to_string())?;
        if let Some(median) = sold.median {
            row("sold_median", format!("{median:.2}"))?;
        }
        for point in &sold.percentiles {
            row(
                &format!("sold_p{:02}", point.percentile),
                format!("{:.2}", point.price),
            )?;
        }
        row("sold_sales_read", sold.read.to_string())?;
        row("sold_sales_recorded", sold.recorded.to_string())?;
        if let Some(covering) = &sold.covering {
            row("sold_covering", covering.clone())?;
        }
        if let Some(premium) = sold.asking_premium {
            row("asking_premium_over_sold", format!("{premium:.4}"))?;
        }
        if let Some(discount) = sold.typical_discount {
            row("typical_discount", format!("{discount:.4}"))?;
        }
        row(
            "sold_at_or_above_ask",
            sold.sold_at_or_above_ask.to_string(),
        )?;
    } else {
        row("basis", "asking".to_string())?;
    }
    row("cheapest", format!("{:.2}", report.market.cheapest))?;
    row("dearest", format!("{:.2}", report.market.dearest))?;
    if let Some(mean) = report.market.mean {
        row("mean", format!("{mean:.2}"))?;
    }
    for point in &report.market.percentiles {
        row(
            &format!("p{:02}", point.percentile),
            format!("{:.2}", point.price),
        )?;
    }
    if let Some(class) = &report.class {
        row("class", class.segment.name().to_string())?;
        row("class_percentile", class.percentile.to_string())?;
    }
    if let Some(bands) = &report.bands {
        row("band_basis", bands.basis.label().to_string())?;
        for band in &bands.bands {
            row(
                &format!("band_{}_from", band.band),
                band.from
                    .map(|value| format!("{value:.2}"))
                    .unwrap_or_default(),
            )?;
            row(
                &format!("band_{}_to", band.band),
                band.to
                    .map(|value| format!("{value:.2}"))
                    .unwrap_or_default(),
            )?;
        }
    }
    writer.flush()?;
    Ok(())
}

pub fn print_listings_csv(out: &mut impl Write, report: &ListingReport) -> io::Result<()> {
    let mut writer = csv::Writer::from_writer(&mut *out);
    writer.write_record([
        "price",
        "currency",
        "condition",
        "band",
        "year",
        "shop",
        "title",
        "url",
    ])?;
    for listing in &report.listings {
        writer.write_record([
            format!("{:.2}", listing.price),
            report.currency.clone(),
            neutralize(&listing.condition),
            listing
                .band
                .map(|band| band.name().to_string())
                .unwrap_or_default(),
            neutralize(&listing.year),
            neutralize(&listing.shop),
            neutralize(&listing.title),
            listing.url.clone().unwrap_or_default(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

pub fn print_models_csv(out: &mut impl Write, report: &ModelReport) -> io::Result<()> {
    let mut writer = csv::Writer::from_writer(&mut *out);
    writer.write_record([
        "id",
        "title",
        "brand",
        "category",
        "used_listings",
        "used_from",
        "new_listings",
        "new_from",
        "url",
    ])?;
    for model in &report.models {
        writer.write_record([
            model.id.to_string(),
            neutralize(&model.title),
            neutralize(&model.brand),
            neutralize(&model.category),
            model.used_listings.to_string(),
            model
                .used_from
                .map(|value| format!("{value:.2}"))
                .unwrap_or_default(),
            model.new_listings.to_string(),
            model
                .new_from
                .map(|value| format!("{value:.2}"))
                .unwrap_or_default(),
            model.url.clone(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

pub fn print_classes_csv(out: &mut impl Write, report: &ClassReport) -> io::Result<()> {
    let mut writer = csv::Writer::from_writer(&mut *out);
    writer.write_record([
        "category",
        "currency",
        "condition",
        "class",
        "from",
        "to",
        "share",
    ])?;
    for row in &report.classes {
        writer.write_record([
            neutralize(&report.category),
            report.currency.clone(),
            report.condition.clone(),
            row.segment.name().to_string(),
            row.from
                .map(|value| format!("{value:.2}"))
                .unwrap_or_default(),
            row.to
                .map(|value| format!("{value:.2}"))
                .unwrap_or_default(),
            format!("{:.2}", row.share),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn price(value: f64, currency: &str) -> String {
    money::format(Money::from_major(value, currency), currency)
}

/// A price where there is one, and an em dash where there is not.
///
/// The em dash is the report's one way of saying "no figure", so it is spelled in a
/// single place rather than at each of the sites that needs it.
fn price_or_dash(value: Option<f64>, currency: &str) -> String {
    value.map_or_else(|| "—".to_string(), |value| price(value, currency))
}

/// The precision a whole column of ranges should share: whole units once anything in it
/// is three figures or more.
fn coarse_column(boundaries: impl IntoIterator<Item = Option<f64>>) -> bool {
    boundaries
        .into_iter()
        .flatten()
        .any(|value| value.abs() >= 100.0)
}

/// A range, at the precision the rest of its column is using.
///
/// Deciding per value gives a ladder reading `under $80.00` above `$80 – $150`, where the
/// same boundary is printed two ways one line apart. A column is one thing to a reader,
/// so the whole column agrees.
fn price_range(from: Option<f64>, to: Option<f64>, currency: &str, coarse: bool) -> String {
    let render = |value: f64| {
        if coarse {
            let symbol = money::symbol(currency);
            let digits = money::group_thousands(value, 0);
            if symbol.is_empty() {
                format!("{digits} {currency}")
            } else {
                format!("{symbol}{digits}")
            }
        } else {
            price(value, currency)
        }
    };
    match (from, to) {
        (None, Some(to)) => format!("under {}", render(to)),
        (Some(from), None) => format!("over {}", render(from)),
        (Some(from), Some(to)) => format!("{} – {}", render(from), render(to)),
        (None, None) => "—".to_string(),
    }
}

/// `1 reading`, `6 readings`.
fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{} {noun}s", number(count as u32))
    }
}

fn percent(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

pub fn number(value: u32) -> String {
    money::group_thousands(f64::from(value), 0)
}

/// `1st`, `2nd`, `3rd`, `52nd`, `11th`. The teens are the exception that catches people.
fn ordinal(value: u32) -> String {
    let suffix = match (value % 100, value % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{value}{suffix}")
}

fn bar(value: u32, largest: u32, width: usize) -> String {
    let filled = ((f64::from(value) / f64::from(largest.max(1))) * width as f64).round() as usize;
    "█".repeat(filled.min(width))
}

/// Truncates `text` to whatever room is left on a row after `used` characters.
///
/// The alternative is a wrapped line, which puts the next row's columns half a line out
/// and makes the whole table unreadable rather than just the one cell.
fn fit(text: &str, used: usize) -> String {
    truncate(text, rule_width().saturating_sub(used).max(8))
}

/// Public so a prompt can draw rows that fit, without duplicating the rule.
pub fn shorten(value: &str, width: usize) -> String {
    truncate(value, width)
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let kept: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// Defuses a value a spreadsheet would run as a formula.
///
/// Shop names and listing titles are seller-controlled text going into a CSV that someone
/// will open in Excel; `=cmd|...` in a guitar title should be a string, not an execution.
fn neutralize(value: &str) -> String {
    if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_stays_within_bounds_whatever_the_terminal_says() {
        // Not measurable directly here — there is no terminal under a test harness — but
        // the bounds are what keep a table readable at either extreme.
        let width = rule_width();
        assert!((60..=110).contains(&width), "rule width was {width}");
        // And whatever it is, fitting text to it never returns something wider.
        assert!(
            fit("a very long listing title that would run past any rule", 40)
                .chars()
                .count()
                <= width
        );
        // Even an absurd claim on the space leaves room for something.
        assert!(!fit("something", 500).is_empty());
    }

    #[test]
    fn truncation_keeps_the_width_and_marks_the_cut() {
        assert_eq!("short", truncate("short", 10));
        assert_eq!("exactlyten", truncate("exactlyten", 10));
        assert_eq!("a very lo…", truncate("a very long guitar title", 10));
        // Counted in characters, so an accented model name is not cut mid-character.
        assert_eq!("Ærø…", truncate("Ærøbass Deluxe", 4));
    }

    #[test]
    fn csv_values_that_look_like_formulas_are_defused() {
        assert_eq!("'=cmd|'/c calc'!A1", neutralize("=cmd|'/c calc'!A1"));
        assert_eq!("'+44 Guitars", neutralize("+44 Guitars"));
        assert_eq!("'-Ludwig", neutralize("-Ludwig"));
        assert_eq!("Gibson Les Paul", neutralize("Gibson Les Paul"));
    }

    #[test]
    fn bars_scale_to_the_largest_value_and_never_overflow() {
        assert_eq!(10, bar(100, 100, 10).chars().count());
        assert_eq!(5, bar(50, 100, 10).chars().count());
        assert_eq!(0, bar(0, 100, 10).chars().count());
        // A value above the stated maximum is clamped rather than wrapping the line.
        assert_eq!(10, bar(200, 100, 10).chars().count());
        assert_eq!(0, bar(0, 0, 10).chars().count());
    }

    #[test]
    fn styling_is_inert_when_colour_is_off() {
        let plain = Style { color: false };
        assert_eq!("premium", plain.band(Band::Premium, "premium"));
        assert_eq!("text", plain.dim("text"));
        let colored = Style { color: true };
        assert!(colored.band(Band::Premium, "premium").contains("\x1b["));
    }

    #[test]
    fn warnings_wrap_inside_the_rule_without_losing_a_word() {
        let warning = "no catalogue model matches \"zzqqxx not a real instrument\", so this \
                       prices the words themselves — results may include unrelated gear";
        let lines = wrap(warning, 60);
        assert!(lines.len() > 1);
        assert!(
            lines.iter().all(|line| line.chars().count() <= 60),
            "{lines:?}"
        );
        // Every word survives the wrap, in order.
        assert_eq!(
            warning.split_whitespace().collect::<Vec<_>>(),
            lines.join(" ").split_whitespace().collect::<Vec<_>>()
        );
        // A word longer than the width goes on its own line rather than vanishing.
        assert_eq!(
            vec!["supercalifragilistic"],
            wrap("supercalifragilistic", 5)
        );
        assert!(wrap("", 20).is_empty());
    }

    #[test]
    fn a_column_of_ranges_shares_one_precision() {
        // A ladder whose top is three figures prints its bottom the same way, so the same
        // boundary never appears as both "$80.00" and "$80" one line apart.
        let ladder = [
            (None, Some(80.0)),
            (Some(80.0), Some(150.0)),
            (Some(150.0), None),
        ];
        let coarse = coarse_column(ladder.iter().flat_map(|(from, to)| [*from, *to]));
        assert!(coarse);
        let rendered: Vec<String> = ladder
            .iter()
            .map(|(from, to)| price_range(*from, *to, "USD", coarse))
            .collect();
        assert_eq!(vec!["under $80", "$80 – $150", "over $150"], rendered);

        // A ladder that lives entirely under a hundred keeps its cents.
        let cheap = [(Some(42.0), Some(67.87))];
        let fine = coarse_column(cheap.iter().flat_map(|(from, to)| [*from, *to]));
        assert!(!fine);
        assert_eq!(
            "$42.00 – $67.87",
            price_range(Some(42.0), Some(67.87), "USD", fine)
        );

        assert_eq!(
            "1,495 NOK – 3,800 NOK",
            price_range(Some(1_495.0), Some(3_800.0), "NOK", true)
        );
        assert_eq!("—", price_range(None, None, "USD", true));
    }

    #[test]
    fn counts_agree_with_their_nouns() {
        assert_eq!("1 reading", plural(1, "reading"));
        assert_eq!("6 readings", plural(6, "reading"));
        assert_eq!("0 readings", plural(0, "reading"));
        assert_eq!("1,200 readings", plural(1_200, "reading"));
    }

    #[test]
    fn a_span_of_days_is_told_in_the_unit_a_reader_thinks_in() {
        assert_eq!("0 day", days(0));
        assert_eq!("1 day", days(1));
        assert_eq!("28 days", days(28));
        assert_eq!("89 days", days(89));
        assert_eq!("3 months", days(90));
        assert_eq!("8 months", days(234));
        assert_eq!("1.6 years", days(600));
        // The five-and-a-half-year Stratocaster listings that started all this.
        assert_eq!("5.4 years", days(1_985));
    }

    #[test]
    fn ordinals_read_the_way_a_person_would_say_them() {
        assert_eq!("1st", ordinal(1));
        assert_eq!("2nd", ordinal(2));
        assert_eq!("3rd", ordinal(3));
        assert_eq!("4th", ordinal(4));
        assert_eq!("52nd", ordinal(52));
        assert_eq!("84th", ordinal(84));
        // The teens take "th" despite ending in 1, 2 and 3.
        assert_eq!("11th", ordinal(11));
        assert_eq!("12th", ordinal(12));
        assert_eq!("13th", ordinal(13));
        assert_eq!("111th", ordinal(111));
        assert_eq!("21st", ordinal(21));
        assert_eq!("100th", ordinal(100));
    }

    #[test]
    fn thousands_are_grouped_in_counts() {
        assert_eq!("46,231", number(46_231));
        assert_eq!("7", number(7));
    }
}
