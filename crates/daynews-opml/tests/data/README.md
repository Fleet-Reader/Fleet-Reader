# OPML samples

Fixtures for the `daynews-opml` tests, kept in the repository so the suite runs on any
machine. The tests previously read one developer's personal export from `~/Desktop`, which
meant they compiled nowhere else.

## Authored here

| File | What it covers |
|---|---|
| `daynews.opml` | The sample subscription list `dayscript/import.yaml` hands to the file picker: the live originals of the seven captured parser fixtures (`crates/daynews-feed/tests/data/README.md` records each source), grouped into two folders with one subscription left at the top level, so an import exercises both shapes at once |

## Vendored from Miniflux

The other three files come from [Miniflux](https://github.com/miniflux/v2)'s OPML parser tests
(`internal/reader/opml/parser_test.go`), licensed under [Apache-2.0](LICENSE-Apache-2.0.txt).

    Copyright The Miniflux Authors. All rights reserved.
    SPDX-License-Identifier: Apache-2.0

Miniflux keeps them as Go raw-string literals. They are stored here as standalone `.opml`
files, with the literal's trailing indentation trimmed so each file ends at `</opml>`; the
document content is otherwise unchanged.

| File | What it covers |
|---|---|
| `mySubscriptions.opml` | A flat 13-feed subscription list — the OPML 2.0 specification's own example, entity-encoded titles and site URLs included, plus the `description`/`type`/`version`/`language` attributes this parser ignores |
| `categories.opml` | Feeds inside folders, which `Opml::feeds` reports as paths |
| `untitled.opml` | Subscriptions with no recorded name — the state a reader is in before a feed's first refresh |

Two of them declare `encoding="ISO-8859-1"`, which real exports still carry. The payloads are
ASCII, so reading them as UTF-8 is lossless.
