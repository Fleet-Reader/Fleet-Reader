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
[GNU General Public License, version 2 or later](LICENSE.txt).

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

## Sync diagnostics for developers

WebDAV sync records bounded, local diagnostics in `webdav-diagnostics.json` alongside
`library.db`. The file retains the last 16 attempts and the last failed attempt separately,
so successful polling and app restarts do not immediately erase the evidence. Each completed
attempt records the app version/platform, HTTP provider, manual/automatic trigger, duration,
error category, HTTP status when available, transport category, ambiguous-write outcome,
server retry delay, next scheduled attempt, replica frontiers, pending uploads, report counters
and the last 16 completed storage operations. Raw platform error messages, credentials,
server URLs, resource names and article content are omitted. The file is capped at 512 KiB;
file reads and atomic replacement writes run off the UI thread. Diagnostic I/O failure does
not turn a successful sync into a failure.

The start of an attempt is persisted before sync begins. A previous start without a recorded
completion is labelled `completion_not_recorded` on the next attempt; this can mean process
termination or failure to save the completion, and is not proof of a network error. Operations
are retained in memory until completion, so a killed or permanently stalled attempt does not
have a persisted per-request trace. Credential/setup failures before a replica starts syncing
are also outside this recorder. These limitations need separate lifecycle/network tracing.

When investigating a failure, preserve this file before resetting an account, together with
the visible status, app build, network type and approximate incident time. Compare frontiers
between devices to distinguish unpublished changes from a stalled download or display update.
Use disposable synthetic data for fault injection; never damage a real reading library.
The day-sync repository's `scripts/stress.py` and reliability section in
`docs/protocol-reference.md` describe deterministic replay and the broader device/server gates.

## Release dependency policy

Tagged App Fair releases use external Rust dependencies from crates.io and a checked-in
`Cargo.lock`. First-party crates within this repository remain workspace/path dependencies.
The tag workflow selects `day-deps: release`: it checks the lock with `cargo metadata --locked`,
rejects Git/local source overrides, preserves every resolved version, and installs the published
Day CLI matching the locked framework. It never updates or regenerates a release lock.
Development/main and scheduled compatibility builds may resolve different versions.

Prepare the release lock in a clean checkout without machine-local `.cargo` patches, run the
app's tests against that crates.io graph, and include the validated lock in the release commit
before tagging. Missing published dependencies must be released first; a Git fallback is not
permitted. After local patched development, regenerate/validate the crates.io lock in isolation
so the release does not inherit machine-local sources. App Fair submission rebuilds use the
same pinned lock and CLI. Committing or pushing still requires the maintainer's permission.
