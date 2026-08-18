# Security

## Reporting a vulnerability

Please use the repository's **Security → Report a vulnerability** form so a
report and any proof of concept stay private until a fix is available. Do not
open a public issue.

## What gearprice talks to

`gearprice` makes requests to exactly two hosts, both over HTTPS:

- `api.reverb.com` — the public Reverb marketplace API, for every price, sold-history
  and category lookup. These endpoints need no credentials, and gearprice sends
  none. It has no notion of a Reverb account, never signs in, and never reads a
  credential store, so there is nothing of yours for it to leak.
- `api.github.com` and the GitHub release CDN — only when you run
  `gearprice update` or `gearprice update --check`. Nothing else contacts them.

`GEARPRICE_API_ROOT` redirects the marketplace requests, which exists for the
test suite and for running behind a proxy. Point it somewhere you trust: a
hostile server can only feed gearprice wrong prices, but wrong prices are the
whole output.

## Self-update

`gearprice update` downloads the release asset for your platform and verifies it
against the `SHA256SUMS` published on the same release before replacing the
running executable. A mismatch aborts the update.

## What lands on disk

Responses are cached under `~/.cache/gearprice` (or `XDG_CACHE_HOME`, or
`GEARPRICE_CACHE_DIR`). The cache holds public marketplace data — listing
titles, prices, shop names — and nothing about you. `gearprice cache --clear`
empties it.

## CSV output

Listing titles and shop names are written by sellers. Values that a spreadsheet
would otherwise evaluate as a formula are prefixed with a quote in CSV output,
so a crafted listing title cannot execute when the file is opened.
