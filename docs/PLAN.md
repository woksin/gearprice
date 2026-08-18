# Plan

Where gearprice stands, what is wrong with it, and what to build next.

Written 2026-08-18, at the end of the session that found the sold-price endpoint.

---

## 1. The correction that comes first

**gearprice currently tells people something untrue.** The README, `SECURITY.md`, the
`report::SOURCE` constant and three commit messages all state that sold prices are
unobtainable because Reverb retired its Price Guide. The Price Guide *is* dead —
`/api/priceguide` answers `403 "This endpoint is no longer publicly available"` to
everyone — but sold data simply moved:

```
GET /api/comparison_shopping_pages/{id}/transactions?per_page=50&page=1
```

It is public, needs no credentials, and returns real transactions:

```json
{ "date": "2026-01-14", "condition": "Good", "source": "Reverb", "order_id": 24781595,
  "price_ask":   { "amount": "5000.00", "amount_cents": 500000, "currency": "USD" },
  "price_final": { "amount": "4100.00", "amount_cents": 410000, "currency": "USD" } }
```

The consequence is not cosmetic. Asking prices run well above what people pay:

| Model | Asking median | Sold median | Gap |
|---|---|---|---|
| Gibson Les Paul Standard '60s | $2,223 | $1,868 | −16% |
| Fender American Professional II Stratocaster | $1,573 | $1,250 | −21% |
| Epiphone Casino (2023 – Present) | $653 | $500 | −23% |
| Marshall JMP Major (guitar) | $5,384 | $2,900 | −46% |

Every band gearprice prints is anchored to the wrong number. **Until the rebuild lands,
the README must not claim sold prices are unavailable.**

---

## 2. Sold prices as the primary number

### What the endpoint gives

- Fields: `date`, `condition`, `source`, `order_id`, `price_ask`, `price_final`.
- Paginated: `per_page` caps at 50, `total` and `total_pages` are reported, `page` works.
- Honours `X-Display-Currency` — verified against USD, NOK and EUR.
- Coverage varies enormously and some models have none. Les Paul Standard '60s has 3,073
  sales; the Marshall Major has 39; one probe returned 0. **Absence must read as
  "no sales recorded", never as a price of zero.**

### How much history to read, and why it is not a fixed window

Recency matters — the Major sold for ~$2,895 in 2014 and $4,100 in 2026, so a twelve-year
median is a misleading number. But a fixed *time* window is wrong too, because sale
density varies by three orders of magnitude between models:

- Les Paul Standard '60s: the newest 50 sales span about **six weeks**.
- Marshall Major: 39 sales span **twelve years**.

So take the newest N sales rather than a fixed period, and **report the span they
actually cover**. That self-adjusts: a popular model gets six recent weeks, a rare one
gets everything there is, and the reader is told which they are looking at.

- `SOLD_TARGET_SAMPLE`: 100 sales (2 pages) by default.
- `SOLD_MAX_PAGES`: 6, a hard ceiling of 300 sales.
- `--sold-sample N` to override.
- Always print the date range covered and the count.

### What to compute

- Percentiles of `price_final` — these become the bands.
- Median `price_final / price_ask`, and the share that went at or above asking. On the
  Major: median −9.1%, and 13 of 39 sold at full ask. That is the negotiating-room figure,
  and the spread matters as much as the median — it is not a rule.
- Sold median per condition grade. On the Major this is clean and monotonic
  (Good $2,700 → Very Good $2,986 → Excellent $3,225) where the *asking* prices were
  inverted, which is itself an argument for sold being the primary number.
- Sold median by year, for the trend.

### How the report should read

Sold leads; asking becomes the negotiating position beside it.

```
  Sold        $1,868 median   312 sales, Jul–Aug 2026
  Asking      $2,223 median   +19% above sold
  Room        -9% typical, 1 in 3 go at full ask

What people actually paid
  band     sold range          share   asking equivalent
  ───────────────────────────────────────────────────────
  steal    under $1,450         10%    under $1,700
  fair     $1,680 – $2,050      50%    $2,000 – $2,450
  premium  over $2,300          10%    over $2,750
```

`--asking N` should be judged against **sold**, and say where it sits among prices anyone
actually paid.

### Knock-on work

- `report::SOURCE` and `SOURCE_SHORT` are rewritten; they currently exist to carry the
  false caveat.
- `track` records sold medians as well as asking, so the history becomes a record of what
  the gear traded at.
- `classes` stays on asking prices — a category-wide sold distribution would cost a
  transactions call per model, which is not affordable. Say so in the output.
- The counting engine is untouched. It is still the only way to get percentiles over a
  live market of any size, and it is still what `--ships-to`, `classes` and the band
  ranges for asking prices rest on.

---

## 3. Recall — stop missing what the user is looking for

Pinning a search to a catalogue model is right for common gear: a text search for
`Boss DS-1` returns the pedal, a T-shirt, a knob set and a page of service notes, and the
junk lands in the cheap band. It is **wrong for rare gear**, because sellers of rare
things often do not attach their listing to the catalogue entry.

Measured on the Marshall Major: the pin found **6 listings; a widened text search found 9**.
Three genuine amps — including the cheapest at $3,949 — were invisible.

Build:

- When a pinned market is thin (fewer than ~25 listings), also run the widened text
  search, keep listings whose title plausibly matches the model, and report how many the
  widening added: `pinned 6, widened search added 3`.
- Keep the strict pin when the market is large — that is where the junk problem bites.
- `--strict` to force pin-only, `--wide` to force the widened search.

---

## 4. Variants of one model

The Major is the worked example of why this matters:

- **Model 1967** is the *guitar* Major. **Model 1978** is the *bass* Major. Both are
  called "Major", both are 200 watts, and they are different amps with different markets
  (39 sales against 6).
- The catalogue also splits by era, and the plexi-panel and metal-panel years are not the
  same instrument to a buyer.

Build a `gearprice variants <query>` view: every catalogue entry in the family, side by
side, with used count, asking median, **sold median**, and sale count for each. That is
the view that answers "which one am I actually holding, and which one am I pricing".

---

## 5. Mistake-proofing

Each of these came from a real error made during the Marshall Major session:

- **The year field carries the model number.** Reverb populates `year` on these listings
  with `1967`/`1978` — the *model* number, not the year built. A 1969 amp reports 1967.
  Detect when a listing's `year` matches its model designation and prefer the title, or
  say the year is unstated. My own first analysis got this wrong.
- **Production ranges in titles are not years.** `... Guitar Amp Head 1968 - 1974` is the
  run, not the unit. Never read a year out of a `19xx - 19xx` pattern.
- **Per-listing age in thin markets.** The clearing signal needs 4+ dated listings per
  band, so on a 9-listing market it shows nothing — exactly where "has this sat for 19
  months?" matters most. Below the threshold, print the age on each listing row instead.
  One Major had been listed **593 days**.
- **Warn when the sample is too small to carry percentiles.** Six listings do not have a
  meaningful p90. Say so above the table rather than printing five bands as if they mean
  something.
- **Flag a listing whose title disagrees with the model it is pinned to.** A listing
  titled `Vintage 1968 Marshall Plexi Amplifier Head` sits in the Major catalogue entry.
  It may be right; the reader should be told to look.

---

## 6. Interactive picking

When several models fit, gearprice prints `--model-id` values to copy by hand. When stdout
is a terminal, prompt instead — a numbered list, arrow keys, enter. `--model-id` and a
non-interactive stdout keep the current behaviour so scripts and pipes are unaffected.

---

## Order of work

1. **Correct the false claim** in `README.md`, `SECURITY.md` and `report::SOURCE`. Small,
   and it is currently public and wrong. *(Done in the same commit as this plan.)*
2. Sold prices: `src/sold.rs`, the transactions client, and the rebuilt `price` report.
3. Mistake-proofing — small, independent, and each item is already specified above.
4. Recall widening.
5. `variants`.
6. Interactive picking.

## Things worth not breaking

- The counting engine's exactness, and its offline tests against synthetic populations.
- The closed vocabularies. Reverb silently ignores unrecognised values for `condition`,
  `category` and `ships_to`, answering `200` with the whole unfiltered market. Three
  traps found so far; assume the fourth exists and validate every new filter the same way.
- Every number keeps saying what it is. The reason this plan exists is that the tool spent
  four commits confidently reporting asking prices while calling them the best available
  answer.
