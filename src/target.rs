//! Working out what a person pasted.
//!
//! The natural way to use a price checker is to be looking at something and want to know
//! about it. So anything identifying a listing or a model should work as an argument: a
//! listing URL copied from a browser, a product page URL, a bare id, or the words someone
//! would have typed anyway.

/// What an argument turned out to point at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// One listing, by Reverb id.
    Listing(u64),
    /// One catalogue model, by Reverb id.
    Model(u64),
    /// One catalogue model, by the slug in its web address.
    ModelSlug(String),
    /// Words to search for.
    Words(String),
}

/// Reads an argument as whatever it identifies.
///
/// A bare number is deliberately *not* treated as an id. Model names are full of them —
/// `1967`, `6505`, `4003`, `335` — and silently pricing catalogue entry 335 for somebody
/// who typed `335` would be both wrong and baffling.
pub fn parse(argument: &str) -> Target {
    let trimmed = argument.trim();
    let lower = trimmed.to_lowercase();

    // The path markers are only trusted on Reverb's own hosts. Plenty of sites have an
    // `/item/` path, and reading an id out of one of those would price something the
    // person is not looking at.
    if lower.contains("reverb.com") {
        if let Some(id) = after_marker(trimmed, "/item/").and_then(leading_number) {
            return Target::Listing(id);
        }
        if let Some(id) = after_marker(trimmed, "/listings/").and_then(leading_number) {
            return Target::Listing(id);
        }
        if let Some(id) =
            after_marker(trimmed, "/comparison_shopping_pages/").and_then(leading_number)
        {
            return Target::Model(id);
        }
        if let Some(rest) = after_marker(trimmed, "/p/") {
            let slug: String = rest
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '-')
                .collect();
            if !slug.is_empty() {
                return Target::ModelSlug(slug);
            }
        }
    }
    Target::Words(trimmed.to_string())
}

/// Whatever follows `marker`, wherever it appears. Tolerates the scheme, `www.`, the
/// locale prefix Reverb adds outside the United States, and anything a browser bar adds.
fn after_marker<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let lower = text.to_lowercase();
    let at = lower.find(marker)?;
    // Indexing by byte is safe here: everything before the marker in a URL is ASCII, and
    // the marker itself is ASCII, so the boundary is a character boundary.
    text.get(at + marker.len()..)
}

fn leading_number(text: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_url_copied_from_a_browser_is_a_listing() {
        assert_eq!(
            Target::Listing(95_521_465),
            parse("https://reverb.com/item/95521465-1969-marshall-major-200-watt-amp-very-rare")
        );
        // With the locale prefix Reverb adds outside the United States, and a query string.
        assert_eq!(
            Target::Listing(95_521_465),
            parse("https://reverb.com/en-gb/item/95521465-1969-marshall?bk=abc")
        );
        assert_eq!(
            Target::Listing(95_521_465),
            parse("  reverb.com/item/95521465  ")
        );
    }

    #[test]
    fn a_product_page_url_is_a_model() {
        assert_eq!(
            Target::ModelSlug("gibson-les-paul-standard-60s-2019-present".into()),
            parse("https://reverb.com/p/gibson-les-paul-standard-60s-2019-present")
        );
        assert_eq!(
            Target::ModelSlug("gibson-les-paul-standard-60s".into()),
            parse("https://reverb.com/p/gibson-les-paul-standard-60s?condition=used")
        );
        assert_eq!(
            Target::Model(137_981),
            parse("https://api.reverb.com/api/comparison_shopping_pages/137981")
        );
    }

    #[test]
    fn a_bare_number_is_words_because_model_names_are_full_of_numbers() {
        // Peavey 6505, Rickenbacker 4003, Gibson ES-335, Marshall Model 1967. Treating any
        // of these as a catalogue id would price something unrelated without saying so.
        assert_eq!(Target::Words("6505".into()), parse("6505"));
        assert_eq!(Target::Words("335".into()), parse("335"));
        assert_eq!(Target::Words("137981".into()), parse("137981"));
    }

    #[test]
    fn anything_else_is_what_it_looks_like() {
        assert_eq!(
            Target::Words("gibsen les pual standrd".into()),
            parse("gibsen les pual standrd")
        );
        assert_eq!(Target::Words(String::new()), parse("   "));
        // A URL that is not Reverb's is not silently treated as one.
        assert_eq!(
            Target::Words("https://example.invalid/item/123".into()),
            parse("https://example.invalid/item/123")
        );
    }
}
