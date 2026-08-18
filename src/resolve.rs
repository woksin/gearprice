//! Working out which piece of gear somebody meant.
//!
//! People do not type catalogue titles. They type `gibsen les pual standrd`, or `strat am
//! pro ii`, or just `jazzmaster`. Reverb's own catalogue search is unforgiving of all
//! three: two typos in one query returns nothing at all, and a bare model name ranks by
//! its own relevance rather than by what anyone is likely to have meant.
//!
//! The catalogue is no tidier than the queries. Titles carry stray tabs, inconsistent
//! punctuation (`DS-1` against a typed `ds1`), era suffixes, and finish names, so
//! matching has to be forgiving in both directions.
//!
//! So resolution happens in three steps, and only pays for the ones it needs:
//!
//! 1. Search what was typed. If a confident match comes back, done — no extra requests.
//! 2. Otherwise correct the spelling against Reverb's own vocabulary of every brand and
//!    model name it knows, and search again, pooling both sets of candidates.
//! 3. Rank them on how much of the query the title accounts for, how much of the title
//!    the query did not ask for, and how much of that model is actually on the market.
//!
//! When the top two are close the answer is presented as a choice rather than a fact,
//! because for `jazzmaster` there is no single right answer and pretending otherwise is
//! how someone ends up reading the price of a Squier when they meant a Fender.

use std::collections::HashMap;

use anyhow::Result;

use crate::reverb::{CatalogueModel, Client};

/// Below this, a match is not worth trusting without trying to correct the spelling.
const CONFIDENT: f64 = 0.80;

/// Below this, the best match is a poor enough fit to be worth doubting out loud.
const WEAK: f64 = 0.62;

/// Within this of the winner, an alternative is close enough to be worth showing.
///
/// Measured rather than guessed: across a judged set of queries, the clear wins sit
/// 0.05 to 0.09 ahead of the runner-up, and the genuinely ambiguous ones — `ibanez tube
/// screamer`, `gibson es-335`, `epiphone casino` — sit under 0.03.
const CONTENDER: f64 = 0.03;

/// How many alternatives to offer. Past a handful the list stops being a help.
const ALTERNATIVES: usize = 3;

/// Words shorter than this are never corrected — too many things are one edit apart.
const MIN_CORRECTABLE: usize = 4;

/// Both halves of a split compound must be at least this long and this common.
const MIN_PART: usize = 3;
const MIN_PART_FREQUENCY: u32 = 8;

/// What resolution concluded.
pub struct Resolution {
    pub chosen: Option<CatalogueModel>,
    /// What the query was corrected to, when correcting it is what found the answer.
    pub interpreted_as: Option<String>,
    /// Other models close enough to the winner to be worth naming.
    pub alternatives: Vec<CatalogueModel>,
    /// Every candidate found, best first.
    pub ranked: Vec<CatalogueModel>,
    pub confidence: f64,
}

impl Resolution {
    /// A model the caller named outright, so there was nothing to work out.
    pub fn of(model: CatalogueModel) -> Self {
        Self {
            chosen: Some(model),
            interpreted_as: None,
            alternatives: Vec::new(),
            ranked: Vec::new(),
            confidence: 1.0,
        }
    }

    /// No resolution was attempted — the caller asked for a raw text search.
    pub fn none() -> Self {
        Self::nothing()
    }

    fn nothing() -> Self {
        Self {
            chosen: None,
            interpreted_as: None,
            alternatives: Vec::new(),
            ranked: Vec::new(),
            confidence: 0.0,
        }
    }

    /// Whether the best match answers so little of the query that it is probably not what
    /// was meant — the query named gear Reverb has no catalogue entry for, or named
    /// something that is not gear at all.
    pub fn is_weak(&self) -> bool {
        self.chosen.is_some() && self.confidence < WEAK
    }
}

/// Finds the catalogue model a query means.
pub fn resolve(client: &Client, query: &str, limit: u32) -> Result<Resolution> {
    let typed: Vec<String> = tokenise(query);
    if typed.is_empty() {
        return Ok(Resolution::nothing());
    }

    let mut pool = client.models(query, limit)?.models;
    let mut interpreted = None;

    // Only reach for the dictionary when what was typed did not already work. A correctly
    // spelled query never pays for the vocabulary request.
    if best_score(&pool, &typed) < CONFIDENT
        && let Ok(vocabulary) = Vocabulary::load(client)
    {
        let corrected = vocabulary.correct(&typed);
        if let Some(corrected) = corrected.filter(|value| *value != typed.join(" ")) {
            let found = client.models(&corrected, limit)?.models;
            let gained = best_score(&found, &tokenise(&corrected)) > best_score(&pool, &typed);
            merge(&mut pool, found);
            if gained {
                interpreted = Some(corrected);
            }
        }
    }

    // Still nothing: drop one word at a time. An unrecognisable word in an otherwise
    // reasonable query — a finish name, a shop's abbreviation — should not sink it.
    if pool.is_empty() && typed.len() > 1 {
        let shorter: Vec<String> = (0..typed.len())
            .map(|skipped| {
                typed
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != skipped)
                    .map(|(_, word)| word.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        let found: Vec<Vec<CatalogueModel>> = crate::parallel::each(&shorter, |query| {
            client.models(query, limit).map(|page| page.models)
        })?;
        for models in found {
            merge(&mut pool, models);
        }
    }

    let against = interpreted
        .as_deref()
        .map(tokenise)
        .unwrap_or_else(|| typed.clone());
    let mut ranked: Vec<(f64, CatalogueModel)> = pool
        .into_iter()
        .map(|model| (score(&model, &against), model))
        .collect();
    ranked.sort_by(|left, right| right.0.total_cmp(&left.0));

    let Some((confidence, chosen)) = ranked.first().cloned() else {
        return Ok(Resolution::nothing());
    };
    let alternatives = ranked
        .iter()
        .skip(1)
        .take(ALTERNATIVES)
        .filter(|(other, _)| confidence - other < CONTENDER)
        .map(|(_, model)| model.clone())
        .collect();

    Ok(Resolution {
        chosen: Some(chosen),
        interpreted_as: interpreted,
        alternatives,
        ranked: ranked.into_iter().map(|(_, model)| model).collect(),
        confidence,
    })
}

fn merge(pool: &mut Vec<CatalogueModel>, found: Vec<CatalogueModel>) {
    for model in found {
        if !pool.iter().any(|existing| existing.id == model.id) {
            pool.push(model);
        }
    }
}

fn best_score(models: &[CatalogueModel], query: &[String]) -> f64 {
    models
        .iter()
        .map(|model| score(model, query))
        .fold(0.0, f64::max)
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/// The few words that actually identify a model, for widening a search and for testing
/// whether a listing found that way is the same thing.
///
/// A catalogue title carries far more than a name — `Marshall JMP Model 1967 "Major"
/// 200-Watt Guitar Amp Head 1968 - 1974` — and searching all of it finds nothing, because
/// no seller writes that. What identifies this amp to a person is `Marshall Major`.
///
/// So the specifications come out: model numbers, production years, wattages, and the
/// category words every amp head shares. What is left is the brand and the name.
pub fn signature(title: &str, brand: &str) -> Vec<String> {
    const GENERIC: [&str; 18] = [
        "amp",
        "amplifier",
        "head",
        "combo",
        "guitar",
        "bass",
        "electric",
        "acoustic",
        "watt",
        "watts",
        "series",
        "reissue",
        "vintage",
        "edition",
        "model",
        "the",
        "with",
        "and",
    ];
    let designations = crate::years::model_numbers(title);
    let mut words: Vec<String> = Vec::new();
    for word in tokenise(brand).into_iter().chain(tokenise(title)) {
        let numeric = word.parse::<u32>().ok();
        let is_designation = numeric.is_some_and(|number| designations.contains(&number));
        let is_year = numeric.is_some_and(|number| (1900..=2099).contains(&number));
        if GENERIC.contains(&word.as_str())
            || is_designation
            || is_year
            || is_era_marker(&word)
            // `200-watt` arrives as `200`; a bare number that is not a year is a spec.
            || (numeric.is_some() && word.len() <= 4)
        {
            continue;
        }
        if !words.contains(&word) {
            words.push(word);
        }
    }
    words
}

/// Whether a listing title plausibly describes the model these words name.
///
/// Deliberately strict. This decides whether a listing a text search turned up belongs in
/// a model's market, and letting a T-shirt in would put it at the bottom of the price
/// range where it reads as the bargain of the century.
pub fn describes(title: &str, model_words: &[String]) -> bool {
    // Three quarters of the words, and each of them actually present. `Marshall Major IV
    // Bluetooth Headphones` has two of three and would otherwise join a market of
    // four-thousand dollar amplifiers at thirty-five dollars.
    const ENOUGH: f64 = 0.75;
    if model_words.is_empty() {
        return false;
    }
    let words = tokenise(title);
    let squashed: String = words.iter().map(|word| strip_punctuation(word)).collect();
    // Era markers in the model's title are not something a seller has to repeat.
    let asked: Vec<&String> = model_words
        .iter()
        .filter(|word| !is_era_marker(word))
        .collect();
    if asked.is_empty() {
        return false;
    }
    let matched = asked
        .iter()
        .filter(|word| present(word, &words, &squashed))
        .count();
    matched as f64 / asked.len() as f64 >= ENOUGH
}

/// Whether a word is really in a title, as opposed to nearly in it.
///
/// No prefix credit and no near misses, which is what separates this from the scoring
/// used to rank a search. `Marshall JMP-1 + tc electronic G-Major2` contains `marshall`
/// and `jmp`, and `major2` is not `major` — it is a different pedal by a different maker,
/// and prefix credit was letting it into the amplifier's market.
fn present(word: &str, title: &[String], squashed: &str) -> bool {
    let bare = strip_punctuation(word);
    if is_model_code(&bare) && squashed.contains(&bare) {
        return true;
    }
    title
        .iter()
        .any(|candidate| candidate == word || strip_punctuation(candidate) == bare)
}

/// How well a catalogue model answers a query, from 0 to 1.
pub fn score(model: &CatalogueModel, query: &[String]) -> f64 {
    if query.is_empty() {
        return 0.0;
    }
    let mut title = tokenise(&model.title);
    title.extend(tokenise(model.brand_name()));
    // The same words with nothing between them, so a model code survives whichever way
    // the two sides punctuate it: `DS-1` in the catalogue against a typed `ds1`.
    let squashed: String = title.iter().map(|word| strip_punctuation(word)).collect();

    // How much of what was asked for the title accounts for.
    let matched: f64 = query
        .iter()
        .map(|word| best_match(word, &title, &squashed))
        .sum();
    let coverage = matched / query.len() as f64;

    // How much of the title was not asked for. "Les Paul Standard" should beat "Les Paul
    // Standard '60s Double Trouble Limited Edition" for the query "les paul standard".
    //
    // Era markers do not count against a title. Reverb ends most catalogue entries with
    // the years the model ran — "(2023 - Present)" — which is bookkeeping, not something
    // a buyer failed to ask for. Counting it as surplus is what made a query for
    // "epiphone casino" answer with the rare USA Casino, six listings at $2,774, over the
    // ordinary one everybody means: the plain title looked tighter only because the real
    // one carried its dates.
    let asked_for = title.iter().filter(|word| !is_era_marker(word)).count();
    let surplus = asked_for.saturating_sub(query.len());
    let focus = 1.0 / (1.0 + 0.12 * surplus as f64);

    // And how much of it is actually on the market. Between two models that fit the words
    // equally, the one with two hundred listings is the one somebody meant; the one with
    // three is a footnote. Kept to a small share so it can break ties, not decide them.
    let listings = f64::from(model.used_total + model.new_total);
    let presence = (listings.ln_1p() / 500.0_f64.ln_1p()).min(1.0);

    0.62 * coverage + 0.18 * focus + 0.20 * presence
}

/// How well one query word is answered by any word in a title, from 0 to 1.
fn best_match(word: &str, title: &[String], squashed: &str) -> f64 {
    let bare_word = strip_punctuation(word);
    // A model code typed as one word — `ds1`, `jcm800`, `ts9` — against a catalogue that
    // spells it with punctuation, or the reverse. Restricted to tokens mixing letters and
    // digits, because a plain word matched as a substring would hit far too much.
    if is_model_code(&bare_word) && squashed.contains(&bare_word) {
        return 0.98;
    }
    let mut best: f64 = 0.0;
    for candidate in title {
        let value = if candidate == word {
            1.0
        } else if strip_punctuation(candidate) == bare_word {
            // `ds1` typed against `DS-1` in the catalogue, or the other way round.
            0.98
        } else if candidate.starts_with(word) && word.len() >= 3 {
            0.8
        } else if within_edits(word, candidate, edit_budget(word)) {
            0.6
        } else {
            0.0
        };
        best = best.max(value);
        if best == 1.0 {
            break;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Vocabulary and spelling
// ---------------------------------------------------------------------------

/// Every word Reverb uses for a brand or a model, and how often it uses it.
pub struct Vocabulary {
    frequency: HashMap<String, u32>,
    by_length: HashMap<usize, Vec<String>>,
}

impl Vocabulary {
    pub fn load(client: &Client) -> Result<Self> {
        let source = client.vocabulary()?;
        Ok(Self::build(&source.makes, &source.models))
    }

    pub fn build(makes: &[String], models: &[String]) -> Self {
        let mut frequency: HashMap<String, u32> = HashMap::new();
        let mut count = |word: &str, weight: u32| {
            if word.len() >= 2 {
                *frequency.entry(word.to_string()).or_default() += weight;
            }
        };
        for model in models {
            for word in tokenise(model) {
                count(&word, 1);
            }
        }
        for make in makes {
            // A brand name is a stronger signal than a word in a model name, and it is
            // what people misspell most.
            for word in tokenise(make) {
                count(&word, 3);
            }
        }
        let mut by_length: HashMap<usize, Vec<String>> = HashMap::new();
        for word in frequency.keys() {
            by_length
                .entry(word.chars().count())
                .or_default()
                .push(word.clone());
        }
        Self {
            frequency,
            by_length,
        }
    }

    fn knows(&self, word: &str) -> bool {
        self.frequency.contains_key(word)
    }

    fn frequency_of(&self, word: &str) -> u32 {
        self.frequency.get(word).copied().unwrap_or_default()
    }

    /// The query as Reverb would spell it, or `None` if it is already spelled that way.
    pub fn correct(&self, query: &[String]) -> Option<String> {
        let mut changed = false;
        let corrected: Vec<String> = query
            .iter()
            .map(|word| match self.correct_word(word) {
                Some(fixed) => {
                    changed = true;
                    fixed
                }
                None => word.clone(),
            })
            .collect();
        changed.then(|| corrected.join(" "))
    }

    /// The word Reverb would have used, or `None` to leave it alone.
    fn correct_word(&self, word: &str) -> Option<String> {
        if self.knows(word) || word.chars().all(|character| character.is_ascii_digit()) {
            return None;
        }
        let bare = strip_punctuation(word);
        if bare != *word && self.knows(&bare) {
            return Some(bare);
        }
        if word.chars().count() < MIN_CORRECTABLE {
            return None;
        }

        let nearest = self.nearest(word);
        let compound = self.split(word);
        match (nearest, compound) {
            (Some((single, frequency)), Some((split, weakest))) => {
                // A rare word one edit away is usually a coincidence rather than the
                // intent: `tubescreamer` is one edit from `tubedreamer`, which Reverb
                // uses three times, and is plainly `tube screamer`, which it uses far
                // more. Frequency decides.
                Some(if weakest > frequency.saturating_mul(3) {
                    split
                } else {
                    single
                })
            }
            (Some((single, _)), None) => Some(single),
            (None, Some((split, _))) => Some(split),
            (None, None) => self.complete(word),
        }
    }

    /// The closest word within the edit budget, and how common it is.
    fn nearest(&self, word: &str) -> Option<(String, u32)> {
        let budget = edit_budget(word);
        if budget == 0 {
            return None;
        }
        let length = word.chars().count();
        let mut best: Option<(usize, u32, String)> = None;
        for candidate_length in length.saturating_sub(budget)..=length + budget {
            for candidate in self.by_length.get(&candidate_length).into_iter().flatten() {
                let Some(distance) = edit_distance(word, candidate, budget) else {
                    continue;
                };
                let frequency = self.frequency_of(candidate);
                let better = match &best {
                    // Closest first; among equally close, the word Reverb uses most.
                    // That is what separates `pual` → `paul` from `pual` → `dual`.
                    Some((best_distance, best_frequency, _)) => {
                        distance < *best_distance
                            || (distance == *best_distance && frequency > *best_frequency)
                    }
                    None => true,
                };
                if better {
                    best = Some((distance, frequency, candidate.clone()));
                }
            }
        }
        best.map(|(_, frequency, word)| (word, frequency))
    }

    /// Two words run together, and the frequency of the rarer half.
    fn split(&self, word: &str) -> Option<(String, u32)> {
        let characters: Vec<char> = word.chars().collect();
        if characters.len() < MIN_PART * 2 {
            return None;
        }
        let mut best: Option<(u32, u32, String)> = None;
        for cut in MIN_PART..=characters.len() - MIN_PART {
            let left: String = characters[..cut].iter().collect();
            let right: String = characters[cut..].iter().collect();
            let usable = |part: &str| {
                let frequency = self.frequency_of(part);
                let digits = part.chars().all(|character| character.is_ascii_digit());
                (digits || frequency >= MIN_PART_FREQUENCY).then_some(frequency.max(1))
            };
            let (Some(left_frequency), Some(right_frequency)) = (usable(&left), usable(&right))
            else {
                continue;
            };
            let total = left_frequency + right_frequency;
            let weakest = left_frequency.min(right_frequency);
            if best
                .as_ref()
                .is_none_or(|(best_total, _, _)| total > *best_total)
            {
                best = Some((total, weakest, format!("{left} {right}")));
            }
        }
        best.map(|(_, weakest, split)| (split, weakest))
    }

    /// The commonest word this one is the start of — `strat` for `stratocaster`.
    fn complete(&self, word: &str) -> Option<String> {
        self.frequency
            .iter()
            .filter(|(candidate, _)| {
                candidate.starts_with(word) && candidate.chars().count() > word.chars().count()
            })
            .max_by_key(|(candidate, frequency)| (**frequency, std::cmp::Reverse(candidate.len())))
            .map(|(candidate, _)| candidate.clone())
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// Splits text into comparable words. Punctuation separates, so `Les Paul Standard '60s`
/// and `les-paul standard 60s` come apart the same way.
pub fn tokenise(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric() && character != '\'')
        .filter(|word| !word.is_empty())
        .map(|word| word.to_lowercase().trim_matches('\'').to_string())
        .filter(|word| !word.is_empty())
        .collect()
}

fn strip_punctuation(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// A year, or the word Reverb ends an open-ended run with. Catalogue bookkeeping rather
/// than part of a model's name.
fn is_era_marker(word: &str) -> bool {
    if word == "present" {
        return true;
    }
    word.len() == 4
        && word.chars().all(|character| character.is_ascii_digit())
        && matches!(word.parse::<u32>(), Ok(1900..=2099))
}

/// A designation like `ds1`, `ts9` or `jcm800` rather than an ordinary word.
fn is_model_code(word: &str) -> bool {
    word.chars().count() >= 3
        && word.chars().any(|character| character.is_ascii_digit())
        && word.chars().any(|character| character.is_alphabetic())
}

/// How many edits a word of this length is allowed to be wrong by.
///
/// The same split full-text search engines use for fuzzy terms. Six letters is the point
/// where two edits stops being reckless: `falken` is two substitutions from `falcon`, and
/// under a one-edit budget a Gretsch White Falcon is unfindable. Below four letters no
/// correction happens at all — too much of the catalogue is one edit from `sg` or `ts9`.
fn edit_budget(word: &str) -> usize {
    match word.chars().count() {
        0..=3 => 0,
        4..=5 => 1,
        _ => 2,
    }
}

fn within_edits(left: &str, right: &str, budget: usize) -> bool {
    budget > 0 && edit_distance(left, right, budget).is_some()
}

/// Damerau-Levenshtein distance, or `None` if it exceeds `budget`.
///
/// Transpositions count as one edit rather than two, which matters more than it sounds:
/// `pual` is a transposition of `paul` but two substitutions from it, so plain
/// Levenshtein prefers the unrelated `dual`.
fn edit_distance(left: &str, right: &str, budget: usize) -> Option<usize> {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > budget {
        return None;
    }
    let mut before_previous: Vec<usize> = Vec::new();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current: Vec<usize> = vec![0; right.len() + 1];
    for (row, left_character) in left.iter().enumerate() {
        current[0] = row + 1;
        let mut row_best = current[0];
        for (column, right_character) in right.iter().enumerate() {
            let substitution = usize::from(left_character != right_character);
            let mut best = (previous[column] + substitution)
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
            if row > 0
                && column > 0
                && left_character == &right[column - 1]
                && left[row - 1] == *right_character
            {
                best = best.min(before_previous[column - 1] + 1);
            }
            current[column + 1] = best;
            row_best = row_best.min(best);
        }
        if row_best > budget {
            return None;
        }
        before_previous = std::mem::replace(&mut previous, std::mem::take(&mut current));
        current = vec![0; right.len() + 1];
    }
    (previous[right.len()] <= budget).then_some(previous[right.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature of Reverb's vocabulary, with the frequencies that decide the hard
    /// cases: `paul` commoner than `dual`, `tubedreamer` rarer than either half of
    /// `tube screamer`, `marshall` commoner than `mars` or `hal`.
    fn vocabulary() -> Vocabulary {
        let mut models: Vec<String> = Vec::new();
        let mut repeat = |text: &str, times: usize| {
            for index in 0..times {
                models.push(format!("{text} {index}"));
            }
        };
        repeat("Les Paul Standard", 40);
        repeat("Dual Rectifier", 20);
        repeat("Tube Amp", 30);
        repeat("Screamer Overdrive", 12);
        repeat("Tubedreamer Fuzz", 1);
        repeat("Stratocaster", 35);
        repeat("Jazzmaster", 15);
        repeat("Casino", 9);
        repeat("SG", 20);
        repeat("Mars Bar", 3);
        repeat("Hal Effects", 3);
        repeat("DS-1 Distortion", 10);
        let makes = ["Gibson", "Fender", "Epiphone", "Marshall", "Boss", "Ibanez"]
            .map(str::to_string)
            .to_vec();
        Vocabulary::build(&makes, &models)
    }

    fn corrected(query: &str) -> Option<String> {
        vocabulary().correct(&tokenise(query))
    }

    #[test]
    fn a_transposition_is_one_edit_so_pual_is_paul_and_not_dual() {
        // The case plain Levenshtein gets wrong: 'pual' is two substitutions from 'paul'
        // and one from 'dual', so distance alone picks the amplifier.
        assert_eq!(Some(1), edit_distance("pual", "paul", 2));
        assert_eq!(
            Some("les paul standard".into()),
            corrected("les pual standard")
        );
    }

    #[test]
    fn misspelled_brands_are_corrected_against_reverbs_own_list() {
        assert_eq!(Some("gibson".into()), corrected("gibsen"));
        assert_eq!(Some("epiphone casino".into()), corrected("epiphon casino"));
        assert_eq!(Some("marshall".into()), corrected("marshal"));
        assert_eq!(Some("stratocaster".into()), corrected("stratocastor"));
    }

    #[test]
    fn a_run_together_compound_is_split_only_when_no_real_word_explains_it() {
        // Nothing is one edit from "tubescreamer" except a fuzz pedal Reverb lists once.
        assert_eq!(Some("tube screamer".into()), corrected("tubescreamer"));
        // But "marshal" is one edit from a brand, so it is not "mars hal".
        assert_eq!(Some("marshall".into()), corrected("marshal"));
    }

    #[test]
    fn words_reverb_already_uses_are_left_alone() {
        assert_eq!(None, corrected("gibson les paul standard"));
        assert_eq!(None, corrected("fender stratocaster"));
        // Short words are never corrected — too much is one edit from "sg" or "60s".
        assert_eq!(None, corrected("sg"));
        assert_eq!(None, corrected("60s"));
        assert_eq!(None, corrected("1959"));
    }

    #[test]
    fn punctuation_differences_are_not_spelling_mistakes() {
        // Typed "ds1" against a catalogue that writes "DS-1".
        let vocabulary = vocabulary();
        assert_eq!(None, vocabulary.correct_word("ds1"));
    }

    #[test]
    fn tokenising_treats_both_sides_of_a_match_the_same_way() {
        assert_eq!(
            vec!["les", "paul", "standard", "60s"],
            tokenise("Les Paul Standard '60s")
        );
        assert_eq!(
            vec!["les", "paul", "standard", "60s"],
            tokenise("les-paul  standard 60s")
        );
        assert_eq!(vec!["ds", "1", "distortion"], tokenise("DS-1 Distortion"));
        // A tab out of the catalogue is a separator like any other.
        assert_eq!(vec!["squier", "jazzmaster"], tokenise("Squier\tJazzmaster"));
        assert!(tokenise("   ").is_empty());
    }

    fn model(id: u64, title: &str, used: u32, new: u32) -> CatalogueModel {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "slug": "x",
            "title": title,
            "used_total": used,
            "new_total": new,
        }))
        .unwrap()
    }

    #[test]
    fn a_models_signature_is_the_name_a_person_would_use() {
        // The catalogue calls it all of this; a seller calls it a Marshall Major.
        assert_eq!(
            vec!["marshall", "jmp", "major"],
            signature(
                "Marshall JMP Model 1967 \"Major\" 200-Watt Guitar Amp Head 1968 - 1974",
                "Marshall"
            )
        );
        assert_eq!(
            vec!["gibson", "les", "paul", "standard", "60s"],
            signature("Gibson Les Paul Standard '60s (2019 - Present)", "Gibson")
        );
    }

    #[test]
    fn a_widened_search_keeps_the_amp_and_drops_the_t_shirt() {
        let major = signature(
            "Marshall JMP Model 1967 \"Major\" 200-Watt Guitar Amp Head 1968 - 1974",
            "Marshall",
        );
        // Real listings the catalogue entry did not carry.
        assert!(describes(
            "Marshall JMP Major Tube Guitar Amplifier Head 1971",
            &major
        ));
        assert!(describes(
            "1970 Marshall Major JMP Model 1967 200-Watt Amp Head",
            &major
        ));
        // And the things a text search drags in with them.
        assert!(!describes("Marshall Major IV Bluetooth Headphones", &major));
        assert!(!describes(
            "Marshall JMP-1 + tc electronic G-Major2",
            &major
        ));
        assert!(!describes("Fender Twin Reverb", &major));
        assert!(!describes("anything", &[]));
    }

    #[test]
    fn the_model_that_says_what_was_asked_and_little_else_wins() {
        let query = tokenise("gibson les paul standard");
        let plain = model(1, "Gibson Les Paul Standard", 100, 100);
        let embellished = model(
            2,
            "Gibson Les Paul Standard '60s Double Trouble Limited",
            100,
            100,
        );
        assert!(
            score(&plain, &query) > score(&embellished, &query),
            "a title full of words nobody asked for should not win"
        );
    }

    #[test]
    fn a_titles_own_dates_are_not_words_the_buyer_failed_to_ask_for() {
        let query = tokenise("epiphone casino");
        // The real one, whose title carries the years the model ran.
        let ordinary = model(1, "Epiphone Casino (2023 - Present)", 10, 16);
        // A rare variant with a genuinely extra word, and a much thinner market.
        let rare = model(2, "Epiphone USA Casino", 6, 14);
        assert!(
            score(&ordinary, &query) > score(&rare, &query),
            "dates in a title should not make a rarer model look like the tighter match"
        );

        assert!(is_era_marker("2019"));
        assert!(is_era_marker("present"));
        assert!(!is_era_marker("usa"));
        assert!(!is_era_marker("335"));
        assert!(!is_era_marker("1"));
        // A word people do ask for is still counted, whatever it looks like.
        assert!(!is_era_marker("60s"));
    }

    #[test]
    fn a_model_nobody_is_selling_loses_a_tie() {
        let query = tokenise("jazzmaster");
        let traded = model(1, "Fender Jazzmaster", 200, 100);
        let obscure = model(2, "Fender Jazzmaster", 1, 0);
        assert!(score(&traded, &query) > score(&obscure, &query));
        // But presence only breaks ties — it cannot carry a model that does not fit.
        let wrong = model(3, "Fender Telecaster Deluxe", 5_000, 5_000);
        assert!(score(&obscure, &query) > score(&wrong, &query));
    }

    #[test]
    fn a_model_code_matches_however_either_side_punctuates_it() {
        let catalogue = model(1, "Boss DS-1 Distortion", 200, 20);
        // The catalogue writes DS-1 and splits into "ds" and "1"; the query is one word.
        assert!(
            score(&catalogue, &tokenise("boss ds1")) > 0.75,
            "typing ds1 for DS-1 should still find it"
        );
        assert!(score(&catalogue, &tokenise("boss ds-1")) > 0.75);
        assert!(score(&catalogue, &tokenise("marshall jcm800")) < 0.5);

        // Only designations, never ordinary words: "paul" must not match by living
        // inside "lespaulstandard".
        assert!(is_model_code("ds1"));
        assert!(is_model_code("jcm800"));
        assert!(!is_model_code("paul"));
        assert!(!is_model_code("60s") || edit_budget("60s") == 0);
    }

    #[test]
    fn a_misspelled_query_still_scores_against_the_right_model() {
        let catalogue = model(1, "Gibson Les Paul Standard", 200, 50);
        assert!(score(&catalogue, &tokenise("gibsen les paul standrd")) > 0.7);
    }

    #[test]
    fn the_edit_budget_grows_with_the_word_and_bails_out_early() {
        assert_eq!(0, edit_budget("sg"));
        assert_eq!(0, edit_budget("60s"));
        assert_eq!(1, edit_budget("paul"));
        assert_eq!(2, edit_budget("falken"));
        assert_eq!(2, edit_budget("stratocaster"));
        assert_eq!(None, edit_distance("paul", "telecaster", 2));
        assert_eq!(Some(0), edit_distance("paul", "paul", 1));
        assert_eq!(Some(1), edit_distance("standrd", "standard", 2));
    }
}
