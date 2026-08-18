<div align="center">

<img src="assets/banner.svg" alt="gearprice — what it costs, and what class it is" width="820">

<br>

[![Rust](https://img.shields.io/badge/rust-1.88%2B-b7410e?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)](#platforms)
[![CI](https://github.com/woksin/gearprice/actions/workflows/ci.yml/badge.svg)](https://github.com/woksin/gearprice/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/woksin/gearprice?color=86efac)](https://github.com/woksin/gearprice/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-green)](#license)

**What guitar gear costs on Reverb right now — and what class of gear it actually is.**

</div>

---

## Two questions, two answers

Someone is asking $1,900 for a Les Paul Standard. Two different things are worth knowing,
and most tools answer neither cleanly.

**Is that a good price?** That is a question about this model's own market — what everyone
else is asking for the same guitar. gearprice answers it with a **band**.

**What kind of guitar is this, in money terms?** That is a question about the market
around it — where this model sits among every other electric guitar for sale. gearprice
answers it with a **class**.

A $2,300 Les Paul is expensive for a guitar and completely unremarkable for a Les Paul.
Those are different facts and you need both.

```console
$ gearprice price "Gibson Les Paul Standard 60s" --asking 1900
```

```
GEARPRICE  Gibson Les Paul Standard '60s (2019 - Present)
════════════════════════════════════════════════════════════════════════════════════════════
  Model                 Gibson · electric-guitars
                        https://reverb.com/p/gibson-les-paul-standard-60s-2019-present
  Market                174 used listings · USD · asking prices, not sold prices
  Method                read all 174 listings

  Class                 PRO  above the category median
                        64th percentile of 108,402 used Electric Guitars listings

  Asking $1,900         LOW  cheaper than three quarters of the market
                        $323 below the median asking price

Price bands  (what sellers are asking for this model right now)
  Band                          Range   Share  Meaning
  ──────────────────────────────────────────────────────────────────────────────────────────
  steal                  under $1,895     10%  below almost every other asking price
  low                 $1,895 – $2,050     15%  cheaper than three quarters of the market
  fair                $2,050 – $2,499     50%  in the middle half — the going rate
  high                $2,499 – $2,799     15%  dearer than three quarters of the market
  premium                 over $2,799     10%  above almost every other asking price

Distribution
           low          p25       median          p75         high         mean
        $1,599       $2,050       $2,223       $2,499       $7,636       $2,341

  Where the listings sit  (middle 90% — the tails are in low and high above)
       $1,800 – $1,946          20  ██████████████
       $1,946 – $2,092          20  ██████████████
       $2,092 – $2,239          42  ██████████████████████████████
       $2,239 – $2,385          24  █████████████████
       $2,385 – $2,531          28  ████████████████████
       $2,531 – $2,677           9  ██████
       $2,677 – $2,824           8  ██████
       $2,824 – $2,970           8  ██████
```

No account, no API key, no configuration. It reads Reverb's public marketplace.

---

## What it sold for, not just what it costs

Every band gearprice draws comes from **what people actually paid**, where Reverb has a
record of it. Asking prices are shown beside them as the negotiating position, because
that is what they are:

```
  Sold                      $1,819  median of 100 sales · 2026-06-03 to 2026-08-18
  Asking                    $2,238  +23% above sold — the 91st percentile of what anyone paid
  Room                         -6%  typical, but 26 of 100 went at or above the ask
```

The gap is not small, and it is not the same for every model:

| Model | Asking median | Sold median | Gap |
|---|---|---|---|
| Gibson Les Paul Standard '60s | $2,238 | $1,819 | +23% |
| Fender American Professional II Stratocaster | $1,573 | $1,250 | +26% |
| Marshall JMP Major (guitar) | $5,384 | $2,900 | +86% |

Reverb's public Price Guide is retired — `/api/priceguide` answers `403` to everyone — but
the sold history moved rather than vanished, and gearprice reads it from
`/api/comparison_shopping_pages/{id}/transactions`. That endpoint reports both what each
item was listed at and what it went for, which is where the negotiating-room figure comes
from too.

**Where a model has no sold record**, the bands fall back to asking prices and the report
says so on its face. Same when there are only a handful of sales: three sales are worth
knowing about and not worth five percentile bands.

**Where the record runs back years**, the recent median is quoted beside the overall one.
The Marshall Major has sold thirty-nine times since 2014 at a median of $2,900 — and
$4,100 in the last year. Quoting the twelve-year figure alone to somebody buying this
week would be useless:

```
  Sold                      $2,900  median of 39 sales · 2014-08-27 to 2026-01-14
  Sold, last year           $4,100  median of the 4 most recent — read this one
```

---

## Install

**Homebrew**

```bash
brew install woksin/gearprice/gearprice
```

**From a release** — download the binary for your platform from
[the latest release](https://github.com/woksin/gearprice/releases/latest) and put it on
your `PATH`. Every asset is listed in `SHA256SUMS` on the same release.

**From source** — needs Rust 1.88 or newer.

```bash
git clone https://github.com/woksin/gearprice
cd gearprice
./install.sh              # or: .\install.ps1 on Windows
```

Once installed, `gearprice update` upgrades in place, verifying the download against the
release's published checksums first.

---

## Commands

### `gearprice deal` — is this listing worth it?

The one to reach for when you are looking at something. Paste the address:

```console
$ gearprice deal https://reverb.com/item/95521465-1969-marshall-major-200-watt-amp
```

```
GEARPRICE  1969 Marshall Major 200 Watt Amp…VERY RARE
════════════════════════════════════════════════════════════════════════════════════════════
  Asking                    $4,300  HIGH
                                    over three quarters of recent sales
  Sold, last year           $4,100  this one is $200 above
  Room                         -9%  typical off the ask · 13 of 39 went at or above it
  An offer at               $3,909  would be the usual discount off this asking price
  Listed for              5 months  a long wait at this price
```

### `gearprice price` — what one model costs

The main one. Resolves what you typed to a model in Reverb's catalogue, measures its
market, and reports the bands and the class.

```bash
gearprice "Fender American Professional II Stratocaster"   # price is the default
gearprice price "Fender American Professional II Stratocaster"
gearprice price "Boss DS-1" --asking 45          # is 45 a good price?
gearprice price "Les Paul Standard" --condition new
gearprice price "ES-335" --year-min 1960 --year-max 1969
gearprice price "Jazzmaster" --currency NOK --region NO
```

Resolving to a catalogue model is what makes the numbers mean anything. A text search for
`Boss DS-1` returns the pedal — and also a T-shirt, a knob set, a footswitch cover and a
page of service notes. Those land in the cheap end and turn "steal" into "not the thing
you were looking for". Pinning to the catalogue model searches the products Reverb has
identified as that piece of gear. Use `--raw` if you genuinely want the text search.

### `--ships-to` — what you can actually buy, delivered

An asking price is not what a guitar costs you. A $1,750 listing from Tokyo with $656 of
postage is dearer than a $1,900 one an hour away — and a large part of any market will not
ship to you at all.

```bash
gearprice price "Gibson Les Paul Standard 60s" --ships-to NO --currency NOK
```

```
  Market                32 used listings · NOK · asking prices, not sold prices
  Ships to              Norway (NO) · only sellers who send there, priced delivered

Cheapest delivered
   19,860 NOK  steal     Very Good      Gibson Les Paul Standard…
                          21,350 NOK (+1,490 NOK post)
```

174 listings become 32. Of the 108,373 used electric guitars on Reverb, 31,028 ship to
Norway — so for anyone outside the United States, the unfiltered market is mostly a
market they cannot buy from.

Reverb applies the filter itself, so it composes with the counting engine and narrows a
market of any size. The postage comes from Reverb's own region tree: Norway resolves to
the seller's Europe rate rather than falling through to the everywhere-else price, which
on one listing is the difference between $120 and $362. Where a seller has the carrier
price it at checkout, that is reported as unknown rather than as free.

### `gearprice classes` — what a price class means in money

```console
$ gearprice classes electric-guitars
```

```
  Class                            Range   Share  Meaning
  ──────────────────────────────────────────────────────────────────────────────────────────
  entry                       under $600     20%  cheaper than four fifths of its category
  mid                      $600 – $1,495     30%  below the category median
  pro                    $1,495 – $3,800     30%  above the category median
  premium                $3,800 – $8,750     15%  dearer than four fifths of its category
  boutique                   over $8,750      5%  in the dearest twentieth of its category
```

Over 108,402 used listings. Same command works for any category:

```bash
gearprice classes effects-and-pedals
gearprice classes electric-guitars/semi-hollow
gearprice classes amps --condition new
```

### `gearprice models` — which version is it?

Reverb splits a model by era and by variant, and the differences are not cosmetic:

```console
$ gearprice models "marshall major"
```

```
  Version                                        Used Asking from        Sold   Sales
  ──────────────────────────────────────────────────────────────────────────────────────────
  Marshall JMP Model 1967 "Major" 200-Watt Gu…      6      $4,300      $4,100      39
  Marshall JMP Model 1978 "Major" 200-Watt Ba…      1      $3,990      $2,645       6
```

Both are called "Major", both are 200 watts, and one sells for half again what the other
does. Model 1967 is the guitar amp and 1978 is the bass amp — Marshall's model numbers,
not years, which is a trap Reverb's own year field falls into. `--quick` drops the sold
column and the eight lookups behind it, when all you want is an id.

When several models fit and there is somebody there to answer, gearprice asks which one
rather than printing ids to copy. `--no-input` turns that off, a pipe never triggers it,
and `--model-id` always answers it in advance.

### `gearprice listings` — what is for sale, labelled

```bash
gearprice listings "Boss DS-1" --deals
```

Every listing carries the band it falls in, so `--deals` means "below the going rate for
this model" rather than "cheap-sounding".

### `gearprice track` — what a market has done since

Every other command is a photograph. What a guitar cost in March is not recoverable from
Reverb afterwards at any price, so the only way to know whether a market is moving is to
have written it down at the time.

```bash
gearprice track "Gibson Les Paul Standard 60s"     # take a reading
gearprice track "..." --no-record                  # look without adding one
gearprice track --list                             # everything being tracked
```

```
  Tracking              5 readings · used · USD

  Median                    $2,223  down $177 (7.4%) since 2026-03-11
  Listings                     174  -24 on the market since then

Readings
  When               Median   Listings   Typical wait
  ────────────────────────────────────────────────────
  2026-03-11         $2,400        198        70 days
  2026-05-15         $2,350        190        53 days
  2026-07-09         $2,280        181        40 days
  2026-08-18         $2,223        174        42 days  ← now
```

Readings are appended to `~/.local/share/gearprice/history.jsonl`, one JSON object per
line — kept with your data rather than in the cache, because emptying the cache must not
throw away the one thing that cannot be fetched again. A reading is only ever compared
against others of the same question: same currency, same condition, same destination. A
median in kroner beside one in dollars is not a price movement.

### Settings you would otherwise retype

```console
$ gearprice config --example > ~/.config/gearprice/config.toml
```

```toml
currency = "NOK"
ships_to = "NO"
```

Flags beat environment variables beat the file beats the defaults. `gearprice config`
shows where the file is and what is actually in force. A typo in it costs that line and
names it, never the run.

### Odds and ends

Long reports go to a pager when there is a terminal to read them, `less -F -I -R -X` or
whatever `PAGER` says. `--no-pager` prints straight out, and a pipe never pages.
`gearprice completions zsh` (or `bash`, `fish`, `powershell`, `elvish`) prints a completion
script. And the first argument can be the gear itself — `gearprice "les paul"` is
`gearprice price "les paul"`.

### `gearprice categories` — the category tree

Slugs for `--category` and for `gearprice classes`. Either a top-level slug
(`electric-guitars`) or a subcategory (`solid-body`, or `electric-guitars/solid-body` when
a name appears under more than one parent).

---

## Typing it wrong

Nobody types catalogue titles. Reverb's own search does not cope: `gibsen les pual
standrd` returns **nothing at all**, and neither does `strat am pro ii`.

```console
$ gearprice price "gibsen les pual standrd"
```

```
GEARPRICE  Gibson Les Paul Standard '60s (2019 - Present)
════════════════════════════════════════════════════════════════════════════════════════════
  Model                 Gibson · electric-guitars
  Market                174 used listings · USD · asking prices, not sold prices
  Read as               gibson les paul standard (you typed “gibsen les pual standrd”)

  Close matches         these fit what you typed about as well — price one with --model-id
                        Gibson Les Paul Standard '50s (2019 - Prese… --model-id 104711    183 used
                        Gibson Les Paul Standard 1990 - 2001         --model-id 97323     135 used
```

It gets there in three steps, paying only for the ones it needs.

**1. Search what you typed.** If that comes back confident, nothing else happens.

**2. Otherwise, fix the spelling** against Reverb's own vocabulary — every brand and model
name in its catalogue, 2,794 and 12,764 of them, fetched once and cached. No word list is
hardcoded here, so it cannot rot as the catalogue grows.

Three things make the corrections land:

- *Transpositions count as one edit.* `pual` is a transposition of `paul` but two
  substitutions from it, so plain Levenshtein quietly prefers `dual`.
- *Frequency breaks ties.* Where two words are equally close, the one Reverb uses more
  wins — which is also what stops `tubescreamer` becoming `tubedreamer`, a fuzz pedal
  listed exactly three times, instead of `tube screamer`.
- *Run-together words come apart*, but only when no real word explains them: `standrd` is
  a misspelling of `standard`, not a request for `stand rd`.

**3. Rank what came back** on how much of your query the title accounts for, how much of
the title you did not ask for, and how much of that model is actually on the market. Model
codes match through punctuation, so `ds1` finds `DS-1` and `d28` finds `D-28`.

All of which is why these land where they should:

| You type | You get |
|---|---|
| `ephiphone sheraton` | Epiphone Sheraton (2023 - Present) |
| `gretch white falken` | Gretsch G7593 White Falcon I 2003 - 2012 |
| `musicman stingrey` | Music Man StingRay |
| `peavy 6505` | Peavey 6505 MH "Mini Head" |
| `martin d28` | Martin D-28 |
| `jazz bass amercan pro` | Fender American Professional II Jazz Bass |

**When it is a close call, it says so.** `epiphone casino` is not one guitar — the 2023
model is a $650 guitar and the USA Casino is a $2,700 one. gearprice prices the likeliest,
lists the ones that fit about as well with the `--model-id` to price them instead, and
never pretends a coin toss was a conclusion. And when the best match is a poor one, it
says that too, rather than dressing a bass gig bag up as the answer.

---

## How the percentiles are worked out

This is the part worth explaining, because the obvious approach does not work.

Reverb will not hand over more than **2,500 listings** for any one search — `per_page`
caps at 50 and paging stops at page 50. A category like used electric guitars has over a
hundred thousand. You cannot download the market, so you cannot sort it and read off the
quartiles.

But every search reports a `total` that respects its filters — including price bounds.
So `count(price_max = P)` is the cumulative distribution function of the market evaluated
at P, and it costs one small request with no listings transferred at all.

That turns a percentile from a download into a search. The p-th percentile of n listings
is the lowest price P where `count(P) ≥ ceil(p·n)`, and an interpolating search finds it
in a logarithmic number of probes. gearprice takes a short log-spaced ladder first,
memoises every probe into one shared curve, and runs all the percentile searches
concurrently against it — counting a six-figure category takes Reverb well over a second
per request, so doing them in sequence is a minute of waiting and doing them together is
half of it. The curve they leave behind is the histogram, free.

The result is exact, not sampled: it is the same number a full enumeration would produce,
to the nearest whole currency unit, from about fifty requests on a market of any size.
The `classes` table above — five boundaries across 108,402 listings — is 58 requests and
about 30 seconds cold, instant thereafter from cache.

Below 600 listings gearprice reads the market instead, since at that size downloading is
cheaper than probing and gives more: the mean, the condition mix and the listings
themselves. Both paths report the same percentiles, and the report says which one ran.

---

## Where the numbers can mislead you

Stated plainly, because a price is only as good as what you know about it.

- **Sold prices are what people paid, not what a thing is worth.** They are a record of
  completed Reverb sales, so private and shop sales elsewhere are invisible, and a model
  that rarely trades has a thin record. Where there is no record at all, the bands are
  asking prices and read high — the report says which it is using.
- **A model is not a condition.** A `used` band spans mint to fair. A mint example at the
  top of the `fair` band may be the better buy than a beaten one at the bottom. Narrow it
  with `--condition excellent` when the market is big enough to support it.
- **Bands describe the listings, not the guitars.** A refinished, repaired or
  parts-assembled instrument sits in the same market as a clean one and drags the bottom
  down. The cheapest listing is very often cheap for a reason the price alone will not
  tell you.
- **Small markets are noisy.** Six listings do not have a meaningful 90th percentile. The
  report always shows how many listings it measured — read that number before the bands.
- **Currency conversion is Reverb's.** `--currency` asks Reverb to convert; gearprice does
  no conversion of its own and holds no exchange rates.
- **Live means live.** Two runs a week apart will differ, because the market did.

---

## Reference

### Global options

| Option | Default | |
|---|---|---|
| `-c`, `--currency CODE` | `USD` | Any currency Reverb converts to. `GEARPRICE_CURRENCY` |
| `--format table\|json\|csv` | `table` | |
| `--no-cache` | | Ignore cached responses |
| `--cache-ttl MINUTES` | `360` | How long a cached response stays good. `GEARPRICE_CACHE_TTL` |
| `--request-budget N` | `400` | Hard ceiling on API requests for one run |
| `--no-progress`, `--no-color` | | Also honours `NO_COLOR` and `GEARPRICE_NO_PROGRESS` |
| `--no-pager` | | Print long reports straight out. A pipe never pages anyway |
| `--no-input` | | Never stop to ask which model was meant |

### Filters

Available on `price` and `listings`.

| Option | |
|---|---|
| `--condition` | `all`, `used`, `new`, `b-stock`, `mint`, `mint-inventory`, `excellent`, `very-good`, `good`, `fair`, `poor`, `non-functioning`. Default `used` |
| `--sold-sample N` | How many recent sales to read. Default 100 |
| `--no-sold` | Skip the sold history, saving a request or two |
| `--model-id ID` | Price one exact catalogue model, from `gearprice models`. `price` and `listings` only |
| `--raw` | Search the words given instead of resolving them to a model. `price` and `listings` only |
| `--strict` | Never widen a thin catalogue market with a text search. `price` only |
| `--category SLUG` | A slug from `gearprice categories`, or `root/leaf` |
| `--make NAME` | One brand |
| `--year-min`, `--year-max` | |
| `--region CODE` | Where the item is. Country code, e.g. `US`, `GB`, `NO` |
| `--ships-to CODE` | Where it must ship to. Narrows the market to sellers who send there and prices it delivered. `GEARPRICE_SHIPS_TO` |

The condition list is closed on purpose. Reverb ignores a filter value it does not
recognise and answers `200` with the entire unfiltered set — ask it for `brand-new` and
you get every listing, new and used alike, with nothing to tell you the filter was
dropped. Only the values Reverb honours can be constructed, so that failure cannot happen.
The same applies to `--category`: an unknown slug is an error naming the nearest real one,
rather than a silent search of all 641,515 listings on the site.

As a second line of defence, when gearprice reads a market it checks the conditions that
came back against the one it asked for, and warns if they disagree — so if Reverb ever
changes its vocabulary under a released binary, the report says so instead of quietly
reporting the wrong population.

### Environment

| Variable | |
|---|---|
| `GEARPRICE_CURRENCY` | Default display currency |
| `GEARPRICE_SHIPS_TO` | Default shipping destination |
| `GEARPRICE_CONFIG` | Configuration file. Defaults to `$XDG_CONFIG_HOME/gearprice/config.toml` or `~/.config/gearprice/config.toml` |
| `PAGER` | The pager for long reports. `less -F -I -R -X` by default |
| `GEARPRICE_HISTORY` | Tracking history file. Defaults to `$XDG_DATA_HOME/gearprice/history.jsonl` or `~/.local/share/gearprice/history.jsonl` |
| `GEARPRICE_CACHE_DIR` | Cache location. Defaults to `$XDG_CACHE_HOME/gearprice` or `~/.cache/gearprice` |
| `GEARPRICE_CACHE_TTL` | Default cache lifetime in minutes |
| `GEARPRICE_NO_PROGRESS` | Suppress the spinner |
| `GEARPRICE_API_ROOT` | Point at a different API root. For the test suite and for proxies |
| `NO_COLOR` | Suppress colour |

### Scripting

`--format json` gives a stable schema — the report gearprice owns, not Reverb's response
shape passed through.

```bash
gearprice price "Gibson Les Paul Standard 60s" --format json | jq '.class.segment'
# "pro"

gearprice price "Boss DS-1" --format json | jq '.sold.median, .sold.asking_premium'
# what people paid, and how far above it the asking prices run

gearprice price "gibsen les pual standrd" --format json | jq '.interpreted_as, .model.title'
# "gibson les paul standard"
# "Gibson Les Paul Standard '60s (2019 - Present)"

gearprice price "Boss DS-1" --format json \
  | jq '.market.percentiles[] | select(.percentile == 50) | .price'
# 49.99

gearprice classes effects-and-pedals --format csv > pedal-classes.csv
```

Band names (`steal`, `low`, `fair`, `high`, `premium`) and class names (`entry`, `mid`,
`pro`, `premium`, `boutique`) are stable identifiers. `interpreted_as` is present only
when the spelling had to be corrected, and `alternatives` only when something else fit
about as well — so a script can treat either as a reason to stop and check. Every report carries a `source`
field naming what the numbers are, and a `diagnostics` block with the request count, the
cache hits and any warnings.

CSV output prefixes any value a spreadsheet would evaluate as a formula with a quote —
listing titles are written by sellers, and a title beginning `=` should be a string, not
an execution.

---

## Being a good guest

gearprice uses Reverb's public read endpoints, which need no credentials. It tries to
deserve that.

Requests are paced, retried with backoff, and capped per run by `--request-budget`. A
`Retry-After` on a rate limit is obeyed. Every response is cached for six hours by
default, so repeat runs and adjusted flags are usually free — `gearprice cache` shows
what is held and `gearprice cache --clear` empties it.

---

## Platforms

macOS (Apple Silicon and Intel), Linux (x86-64 and arm64), Windows (x86-64 and x86).
Rust 1.88 or newer to build from source.

## License

MIT. See [LICENSE](LICENSE).

gearprice is not affiliated with, endorsed by, or connected to Reverb.com. It reads their
public API as any visitor would.
