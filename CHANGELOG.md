# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Sold prices, read from `/api/comparison_shopping_pages/{id}/transactions`. Bands are now
  cut from what people actually paid, with asking prices shown beside them as the
  negotiating position, and the typical discount off asking reported with the spread.
- A recent median quoted beside the overall one where a model's record runs back years,
  because a twelve-year median describes a market nobody is buying in.
- Sold medians per condition grade, which order correctly where asking prices did not.
- `--sold-sample` and `--no-sold`. `track` records sold medians alongside asking.
- `gearprice deal <url>` — paste a Reverb listing address, or give a listing id, and get a
  verdict on that listing against what the model actually sells for, with the offer that
  would be the usual discount off its ask. `price` accepts addresses too.
- `gearprice variants` — every version of a model side by side, with what each one sells
  for. Marshall's guitar Major and bass Major are both 200-watt "Majors" and one sells for
  half again what the other does.
- `gearprice completions <shell>`, and the first argument may be the gear itself, so
  `gearprice "les paul"` is `gearprice price "les paul"`.
- Thin catalogue markets are widened with a text search, keeping only listings that
  genuinely describe the same model, and saying how many the widening added.
- `--strict` to refuse the widening.
- Per-listing time on the market, shown where a market is too thin for a per-band median.
- A warning when a market is too small for percentile bands to mean anything.
- A warning when Reverb's year field holds the model number rather than a year.

### Fixed

- `--help` still claimed sold prices were unobtainable, which is the first thing anyone
  reads and had been wrong since the sold history was found.
- The default price report ran to sixty-three lines and buried its own answer. The
  asking-price histogram moved behind `--full`, listings default to three, and the sold
  band table no longer runs past the rule.
- Years are read from listing titles rather than trusting Reverb's year field, which on
  some models holds the model designation (Marshall's guitar Major is Model 1967 and was
  built from 1968) and on others holds the production range (`1968 - 1974`). Both dated
  every example of a model to the same wrong year.
- Corrected the claim that sold prices are unobtainable. Reverb's Price Guide endpoint is
  retired, but sold history is public at `/api/comparison_shopping_pages/{id}/transactions`
  and returns both the asking price and the final price for each sale. Asking prices run
  16–46% above sold depending on the model, so every band this tool currently reports
  reads high. `docs/PLAN.md` sets out the rebuild.

### Added

- Spelling correction for queries, against Reverb's own vocabulary of every brand and
  model name in its catalogue. `gibsen les pual standrd` finds the Les Paul Standard,
  where Reverb's own search returns nothing at all.
- Ranked model resolution: candidates are scored on how much of the query the title
  accounts for, how much of the title was not asked for, and how much of that model is on
  the market. Reported as `interpreted_as` when the spelling had to be corrected.
- Close matches are listed with the `--model-id` needed to price them instead, whenever
  something fits the query about as well as the model that was priced.
- A warning when the best catalogue match is a poor fit, rather than presenting it as the
  answer.
- `--raw` on `listings`, matching `price`.
- Median days on the market per price band, which is the closest thing left to evidence
  about which asking prices are actually being paid.
- Median asking price per condition grade, ordered best grade first, and the share of
  sellers who accept offers.
- `--ships-to CODE`, which narrows a market to sellers who will send there and prices the
  listings delivered, using Reverb's own shipping region tree.
- `gearprice track`, which records what a market costs today and reports what it has done
  since. Readings are kept in `~/.local/share/gearprice/history.jsonl`.

### Changed

- `gearprice listings` resolves to a catalogue model like `price` does, so a search for
  `Boss DS-1` no longer prices the T-shirt, the knob set and the footswitch cover
  alongside the pedal.
- `gearprice models` orders results by the same ranking, rather than by Reverb relevance.

### Fixed

- Whitespace in catalogue and listing text is collapsed as it is parsed. Reverb ships
  entries like `Squier\tParanormal Jazzmaster XII`, and a tab reaching a table shifted
  every column after it.
- A model in a category the tree does not know now skips the price class with a warning
  instead of failing the whole report.

## [0.1.0]

### Added

- `gearprice price` — price bands for one model, a price class against its
  category, and a verdict on an asking price with `--asking`.
- `gearprice classes` — what each price class means in money for a whole
  category.
- `gearprice models` — find the catalogue model behind a search, so a price can
  be pinned to one exact piece of gear rather than to matching words.
- `gearprice listings` — what is for sale now, labelled by band, with `--deals`
  for anything below the going rate.
- `gearprice categories` — Reverb's category tree.
- Exact percentiles on markets of any size, by counting rather than downloading,
  which is not subject to Reverb's 2,500-listing paging cap.
- Table, JSON and CSV output; any Reverb-supported display currency.
- A response cache, a request budget, retry with backoff, and `gearprice update`.

[Unreleased]: https://github.com/woksin/gearprice/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/woksin/gearprice/releases/tag/v0.1.0
