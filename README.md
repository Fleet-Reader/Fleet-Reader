# Fleet Reader

Fleet Reader is a feed reader for following news sites, blogs, and other publications in one place.
Subscribe to the sites you read, browse their latest articles, and keep track of what you've read.

On a computer, your subscriptions, article list, and reader sit side by side. On a phone, tap a
subscription to see its articles, then tap an article to read it.

<p align="center">
  <img src="https://fleet-reader.app/gallery/macos-appkit/en/timeline.png"
       width="760" alt="Fleet Reader on Mac, with subscriptions, articles, and the reader side by side">
</p>

- Follow RSS, Atom, and JSON feeds, and organize subscriptions into collections.
- Browse today's articles or catch up on everything unread.
- Search your articles and star the ones you want to keep.
- Read articles in the app, load full articles when the site allows it, or open them in your browser.
- Adjust text size, fonts, and colors, and choose a light or dark appearance.
- Bring your subscriptions from another reader, or take them with you, using OPML import and export.

Fleet Reader fetches feeds directly from the sites you follow and stores your library on your device.
You can read without an account. Experimental WebDAV sync lets you share subscriptions, read
status, and stars across devices through a server you choose.

Fleet Reader runs on Mac, Windows, Linux, iPhone, iPad, Android, HarmonyOS, and the web.
See the [releases](https://github.com/Fleet-Reader/Fleet-Reader/releases) for downloads.

Built with [Day](https://daybrite.dev). Fleet Reader is open source under the
[Apache 2.0 license](https://github.com/Fleet-Reader/Fleet-Reader/blob/main/LICENSE).

## Recommended feeds

The feeds a new reader is offered are not compiled in. They are the list in
`website/public/recommended-feeds.json`, published at
<https://fleet-reader.app/recommended-feeds.json> and fetched at every launch, so the list can
change without an app release. The format is described at the top of `src/recommended.rs`: each
feed has a stable `id`, a `url`, a `name` and `description` that are either one string or a map
from language tags to strings, an optional `category`, an optional `starter` flag that
preselects it, and optional `locales`, which offer the feed only to readers with a matching
language anywhere in their device's preferred languages. A range such as `en-GB` offers a feed
to one country's readers; a bare language such as `fr` offers it to every reader of French.

`resource/assets/recommended-feeds.json` is the same file, bundled for a first launch with no
network; a test fails when the two differ. The app keeps the last list it fetched. An entry
the app cannot read is skipped without costing the rest of the list.

To try a list before publishing it, serve `website/public/` locally and build the app against
it:

```sh
python3 -m http.server 4324 --bind 127.0.0.1 --directory website/public
adb reverse tcp:4324 tcp:4324   # Android only
FLEET_RECOMMENDED_URL=http://127.0.0.1:4324/recommended-feeds.json day launch -t ios-uikit -t android-material
```
