//! Working out what year a listing is actually from.
//!
//! Two traps, both met while pricing a 1969 Marshall Major, and both of which caught this
//! tool's own first analysis.
//!
//! **Reverb's `year` field can hold the model number.** Marshall's guitar Major is Model
//! 1967 and the bass Major is Model 1978. Those are model designations, not years — the
//! guitar version was built from 1968 to 1974 — and Reverb populates the year field on
//! these listings with them. A genuine 1969 amp therefore reports "1967", and anyone
//! filtering by year gets a confidently wrong answer.
//!
//! **A range in a title is a production run, not a date.** Sellers routinely paste the
//! catalogue title verbatim, so `... Guitar Amp Head 1968 - 1974` says when the model was
//! made rather than when this one was. Reading the first number out of it dates a 1972 amp
//! to 1968.
//!
//! When neither source can be trusted, the year is unstated. That is a better answer than
//! a plausible wrong one.

/// Where a year came from, so a report can say how much to trust it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    /// Stated in the listing's title.
    Title,
    /// Reverb's own year field.
    Field,
}

/// The year a listing is from, and where that came from.
///
/// `model_title` is the catalogue model it was matched to, which is what makes a model
/// number recognisable as one.
pub fn stated(title: &str, year_field: &str, model_title: &str) -> Option<(u32, Source)> {
    let designations = model_numbers(model_title);
    if let Some(year) = title_year(title, &designations) {
        return Some((year, Source::Title));
    }
    // The field carries ranges too — Reverb hands back a year of `1968 - 1974` on some of
    // these listings, and reading the first number out of it dates every one of them to
    // the first year of the model's production run.
    let year = four_digit_years(&strip_ranges(year_field))
        .into_iter()
        .find(|year| !designations.contains(year))?;
    plausible(year).then_some((year, Source::Field))
}

/// The model numbers a catalogue title declares, as in `JMP Model 1967 "Major"`.
///
/// Only numbers introduced as a model are treated this way. A title that merely contains
/// `1959` is describing a year like everyone else.
pub fn model_numbers(model_title: &str) -> Vec<u32> {
    let characters: Vec<char> = model_title.chars().collect();
    let mut numbers = Vec::new();
    for index in 0..characters.len() {
        if let Some((number, _)) = model_number_at(&characters, index) {
            numbers.push(number);
        }
    }
    numbers
}

/// A `Model 1967` starting at this position: the number, and how many characters it spans.
///
/// Indexed by character rather than by byte throughout. Titles carry em dashes, curly
/// quotes and accents, and mixing the two kinds of index panics the moment one appears.
fn model_number_at(characters: &[char], start: usize) -> Option<(u32, usize)> {
    const WORD: [char; 5] = ['m', 'o', 'd', 'e', 'l'];
    let matches_word = characters
        .get(start..start + WORD.len())?
        .iter()
        .zip(WORD)
        .all(|(character, wanted)| character.to_ascii_lowercase() == wanted);
    if !matches_word {
        return None;
    }
    let mut cursor = start + WORD.len();
    while matches!(characters.get(cursor), Some(' ' | '#' | '.')) {
        cursor += 1;
    }
    let digits: String = characters
        .get(cursor..)?
        .iter()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    (digits.len() == 4)
        .then(|| digits.parse().ok())
        .flatten()
        .map(|number| (number, cursor + 4 - start))
}

/// A year stated in a title, ignoring production ranges and model numbers.
fn title_year(title: &str, designations: &[u32]) -> Option<u32> {
    let without_ranges = strip_ranges(title);
    let without_models = strip_model_numbers(&without_ranges);
    four_digit_years(&without_models)
        .into_iter()
        .find(|year| plausible(*year) && !designations.contains(year))
}

/// Removes `1968 - 1974` and `1968-1974`, which state when a model was made rather than
/// when one example was.
fn strip_ranges(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut kept = String::with_capacity(text.len());
    let mut index = 0;
    while index < characters.len() {
        if let Some(width) = range_at(&characters, index) {
            kept.push(' ');
            index += width;
        } else {
            kept.push(characters[index]);
            index += 1;
        }
    }
    kept
}

/// The length of a `19xx[ ]-[ ]19xx` range starting here, if there is one.
fn range_at(characters: &[char], start: usize) -> Option<usize> {
    let year_at = |at: usize| -> Option<u32> {
        let text: String = characters.get(at..at + 4)?.iter().collect();
        (text.chars().all(|character| character.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
            .filter(|year| plausible(*year))
    };
    year_at(start)?;
    let mut cursor = start + 4;
    let mut saw_dash = false;
    while let Some(character) = characters.get(cursor) {
        match character {
            ' ' => cursor += 1,
            '-' | '–' | '—' if !saw_dash => {
                saw_dash = true;
                cursor += 1;
            }
            _ => break,
        }
    }
    if !saw_dash {
        return None;
    }
    year_at(cursor)?;
    Some(cursor + 4 - start)
}

/// Removes `Model 1967`, so the designation cannot be read as a date.
fn strip_model_numbers(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut kept = String::with_capacity(text.len());
    let mut index = 0;
    while index < characters.len() {
        match model_number_at(&characters, index) {
            Some((_, width)) => {
                kept.push(' ');
                index += width;
            }
            None => {
                kept.push(characters[index]);
                index += 1;
            }
        }
    }
    kept
}

fn four_digit_years(text: &str) -> Vec<u32> {
    let characters: Vec<char> = text.chars().collect();
    let mut years = Vec::new();
    let mut index = 0;
    while index + 4 <= characters.len() {
        let digits = &characters[index..index + 4];
        let bounded_left = index == 0 || !characters[index - 1].is_ascii_digit();
        let bounded_right = characters
            .get(index + 4)
            .is_none_or(|character| !character.is_ascii_digit());
        if digits.iter().all(char::is_ascii_digit) && bounded_left && bounded_right {
            let text: String = digits.iter().collect();
            if let Ok(year) = text.parse::<u32>() {
                years.push(year);
            }
            index += 4;
        } else {
            index += 1;
        }
    }
    years
}

fn plausible(year: u32) -> bool {
    (1900..=2099).contains(&year)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAJOR: &str = "Marshall JMP Model 1967 \"Major\" 200-Watt Guitar Amp Head 1968 - 1974";

    #[test]
    fn a_model_number_in_the_year_field_is_not_a_year() {
        // The listing Reverb reports as year "1967" is a 1969 amp. Model 1967 is the
        // designation; the model was built 1968 to 1974, so 1967 cannot be a year for it.
        assert_eq!(
            Some((1969, Source::Title)),
            stated(
                "1969 Marshall Major 200 Watt Amp — VERY RARE",
                "1967",
                MAJOR
            )
        );
        // With nothing in the title either, the year is unstated rather than 1967.
        assert_eq!(None, stated("Marshall Major 200 Watt Amp", "1967", MAJOR));
    }

    #[test]
    fn a_production_range_in_a_title_is_not_this_amps_year() {
        // A seller pasting the catalogue title verbatim is not claiming a 1968 build.
        assert_eq!(None, stated(MAJOR, "", MAJOR));
        assert_eq!(
            Some((1972, Source::Title)),
            stated("1972 Marshall Major 200W Amp Head", "", MAJOR)
        );
        // Both spellings of a range, and an en dash.
        assert_eq!(None, stated("Amp Head 1968-1974", "", MAJOR));
        assert_eq!(None, stated("Amp Head 1968 – 1974", "", MAJOR));
    }

    #[test]
    fn a_production_range_in_the_year_field_is_not_a_year_either() {
        // Reverb really does return `1968 - 1974` in the year field on this model.
        assert_eq!(
            None,
            stated("Marshall Major Amp Head", "1968 - 1974", MAJOR)
        );
        // And the real ones still read straight through.
        assert_eq!(
            Some((1969, Source::Field)),
            stated("Marshall Major Amp Head", "1969", MAJOR)
        );
    }

    #[test]
    fn an_ordinary_year_is_still_read_from_either_source() {
        assert_eq!(
            Some((1959, Source::Title)),
            stated(
                "1959 Gibson Les Paul Standard",
                "",
                "Gibson Les Paul Standard"
            )
        );
        assert_eq!(
            Some((2021, Source::Field)),
            stated("Fender Stratocaster", "2021", "Fender Stratocaster")
        );
        assert_eq!(
            None,
            stated("Fender Stratocaster", "", "Fender Stratocaster")
        );
    }

    #[test]
    fn model_numbers_are_only_the_ones_a_title_introduces_as_such() {
        assert_eq!(vec![1967], model_numbers(MAJOR));
        assert_eq!(
            vec![1978],
            model_numbers("Marshall JMP Model 1978 \"Major\" 200-Watt Bass Amp Head")
        );
        // A year in a model name is not a model number.
        assert!(model_numbers("Gibson Les Paul Standard \"Burst\" 1958 - 1960").is_empty());
        assert!(model_numbers("Fender Stratocaster").is_empty());
    }

    #[test]
    fn nonsense_never_becomes_a_year() {
        assert_eq!(None, stated("", "", ""));
        assert_eq!(None, stated("Amp Head 12345678", "", ""));
        assert_eq!(None, stated("Serial 0042", "", ""));
        // A four-digit number that is not a plausible year is not one.
        assert_eq!(None, stated("Marshall 1800 watt", "", ""));
    }
}
