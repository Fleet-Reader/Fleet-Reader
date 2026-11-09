# Demo feeds

The articles Fleet Reader shows in its screenshots and in the walkthrough CI runs on every platform.
They ship inside the app, one directory per language, and `dayscript/seed-demo.yaml` subscribes to
them with `asset:` URLs, so a run needs no network and every platform shows the same articles.

## One set per language

Each directory is named after a locale the app has a catalog for (`resource/locales/<tag>/`), and
each holds the same seven files. A subscription names a feed without its language:

    asset:demo/night-sky.xml

When Fleet Reader reads that feed, it looks in the directory for the app's current language, then in
the one for that language without its region, and finally in `en/`. With the app running in
`zh-CN`, that is `demo/zh-CN/night-sky.xml`, then `demo/zh/night-sky.xml`, then
`demo/en/night-sky.xml`. A screenshot run with `--locale zh-CN` shows Chinese articles as soon as
the app has a `zh-CN` catalog and this folder has a `zh-CN/` set.

`cargo test -p daynews-feed` checks the sets: every app locale has one and every set belongs to an
app locale, each holds the same files, and every feed parses into dated, linked articles.

## The seven feeds

Each feed takes a different shape, so the screenshots exercise the whole parser.

| File | Subject | Shape |
|---|---|---|
| `field-notes.xml` | Animals and plants | RSS 2.0 with `content:encoded` bodies and `dc:creator` |
| `night-sky.xml` | The sky you can see | Atom with full HTML `content` |
| `kitchen-science.xml` | The science of cooking | Plain RSS 2.0, summaries only |
| `atlas.xml` | Geography | RSS 2.0 with escaped HTML in `description` |
| `word-of-the-day.xml` | Words and their origins | RSS 2.0 with CDATA HTML in `description` |
| `workshop.xml` | How everyday things work | RSS 1.0 (RDF) with `dc:date` and `content:encoded` |
| `almanac.json` | Seasonal notes | JSON Feed whose items have no titles, like a microblog |

## Writing a set for a new language

Write for readers of that language rather than translating word for word, and keep to the same
kind of subject: general interest, not tied to current events, and nothing contentious. State only
facts a general reference work states, in your own words, and link each article to a reference page
in that language, such as Wikipedia or Wiktionary, for further reading.

Keep the file names, the number of articles in each feed, and the publication dates, so the
timeline has the same shape in every language. The walkthrough searches for "moon". When a second
language arrives, give that step a localized search term (`toolbar:` takes a Fluent `key:`) and
make sure several feeds in every set contain it.

The articles were written for Fleet Reader and are covered by the app's Apache-2.0 license.
