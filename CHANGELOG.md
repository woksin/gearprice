# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
