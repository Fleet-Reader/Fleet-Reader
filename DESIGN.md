# Fleet Reader — design

A feed reader on the [Day](https://daybrite.dev) framework, modeled on
[NetNewsWire](https://github.com/Ranchero-Software/NetNewsWire): subscriptions on the left, a
timeline in the middle, the article on the right — collapsing to push navigation on a phone.

Targets: `macos-appkit`, `macos-gtk`, `macos-qt`, `windows-winui`, `ios-uikit`, `android-material`,
`web-dom`, `harmony-arkui`.

> `web-dom` builds and runs the whole shell, but a browser may only fetch feeds that send CORS
> headers, and most publishers do not — so the web build reads what it is allowed to reach
> rather than any URL you paste. Its article pane uses the same-origin application resource
> channel (see *Reader*). `harmony-arkui` joined when `day-piece-webview` grew
> its ArkUI renderer — the app builds, installs, and runs in the collapsed phone layout on the
> Oniro emulator. Reader resource-channel behavior is checked by the walkthrough's `web_eval`
> assertions on the CI emulator leg.

## Crates

Split so that everything except the last is testable without a UI or a network.

The libraries live under `crates/`; the UI crate is the repository root.

| crate | owns | depends on |
|---|---|---|
| `crates/daynews-opml` | OPML parse + serialize, nested folders | quick-xml |
| `crates/daynews-time` | the wall clock and the reader's UTC offset, asked of the host | libc (unix), day-toolkit-dom (web) |
| `crates/daynews-feed` | fetching and parsing RSS/RDF/Atom/JSON Feed, normalized | feed-rs, day-part-http, `daynews-time` |
| `crates/daynews-sync` | provider contracts, Feedbin adapter, synchronization engine, HTTP fixtures | day-part-http, serde, chrono, httpdate |
| `crates/daynews-credentials` | native credential vaults | keyring, day-bridge |
| `crates/daynews-db` | the store: models, relations, FTS5, queries | day-persistence, day-model, day-macros, `daynews-time`, `daynews-sync` |
| `crates/daynews-core` | the view-model: signals, refresh orchestration, OPML import/export | `daynews-db`, `daynews-feed`, `daynews-opml`, day-core |
| `fleet-reader` | the UI | `daynews-core`, day, day-piece-webview |

### Dependency choices

Three non-obvious ones, since the house rule is to justify every dependency:

- **feed-rs.** Syndication is four formats plus a long tail of namespaced extensions and at least
  three date encodings, several written wrong by popular publishers. The feeds this was built
  against span YouTube (Atom), Reddit, Mastodon, Discourse, Blogspot, Medium and GitHub, which
  disagree about nearly everything. Policy (which field wins, how ids are derived) stays ours
  in `normalize`.
- **quick-xml.** OPML is XML; hand-rolling means hand-rolling entity decoding and attribute
  quoting. It reads *and* writes, so one dependency covers import and export.
- **No time-zone database at all**, which is why `crates/daynews-time` exists. "Today" is a claim
  about the reader's calendar, so the cut-off is local midnight — which needs the machine's UTC
  offset and its DST rules — and the app also needs a wall clock everywhere it runs, since
  `SystemTime::now()` aborts on `wasm32`. Both come from the host, which already has the rules
  installed and keeps them current: POSIX `localtime_r` (macOS, Linux, iOS, Android, OpenHarmony),
  Win32's `SystemTimeToTzSpecificLocalTime`, and the browser's `Intl` behind day-toolkit-dom's `tzoffset`
  page fact on web. That is ~150 lines and one `libc` edge, against ~200 KB of bundled IANA data
  that would need re-releasing every time a government moves a clock.

SQLite itself is no longer a direct dependency: day-persistence bundles the engine (compiled
from C source, which is what lets Android link — the NDK sysroot ships no `-lsqlite3`) and
owns the FTS5 build.

## The store

One SQLite file, opened lazily through day-persistence's engine
([docs/persistence.md](https://daybrite.dev/docs/persistence)). The schema is *declared*, not
migrated by hand: `#[derive(Model)]` on each type in `crates/daynews-db/src/lib.rs` states the
tables, relations and their delete rules, and the engine applies the schema and its migrations
itself — no directory of `.sql` files staged beside a read-only app bundle.

Search is FTS5 over generated shadow tables — an `fts(...)` attribute on the model declares
which columns are indexed, and the engine keeps them in step, so nothing hand-writes triggers.
Titles, authors and summaries are indexed beside the bodies, which live in their own row so a
timeline window faults metadata only. User text is quoted per token with a prefix `*` on the
last one, which makes search feel live and means punctuation is searched for rather than parsed
as query syntax.

The property everything else rests on: **an article already present is left completely alone on
refresh.** That is what preserves read state, and it is why `daynews-feed` works so hard to derive
a stable id (feed id → article URL → hash of title+date).

## Threading

Native builds own the database through `day-persistence::DatabaseWorker`. Open/migrate,
article/body materialization, FTS, retention, OPML import, read/star edits, undo and feed commits
run on that worker. The UI holds owned sidebar/timeline/reader values in signals. It does not
share the worker's reactive model handles. The web build retains the compatible synchronous
OPFS implementation in `daynews-core::legacy` until an async transferable protocol exists.

Four network exchanges can run concurrently. Parsing HTTP responses already runs off the UI
thread; parsed items move into one atomic worker transaction with metadata and HTTP validators.
The import checks subscription existence at commit time so a late response cannot resurrect an
unsubscribed feed. Existing articles retain their read status and bodies. Imports and retention
are excluded from user undo. An existence-query failure aborts import rather than overwriting
stored read state. Status translations are formatted on the UI thread.

Each window owns cancellable subscriptions for its scope/search and selected article. Retraction
of an effect aborts the old receiver before a new one can publish; completions capture the
originating scene. Reader content stays visible while the next selected article loads, then
the worker snapshot replaces it without an intermediate empty state. Explicitly closing the
reader or changing scope still clears it. Next Unread waits for its cross-scope candidate
before changing the scene. Sidebar filtering uses `Nav::retain_selection_when` so hiding the
last-read feed preserves its current document and toolbar. Sidebar unread
counts use one indexed grouped SQL query, replacing one query per feed. The `(feed,is_read)`
index supports these counts. Projection equality and latest-value delivery reduce unnecessary UI
work; SQL tracing is installed only when trace logging is actually enabled or
`DAY_NEWS_LOG_SQL` is set. The latter writes engine-traced, parameter-expanded statements to
stderr in both debug and release builds, independently of `DAY_LOG`, including worker queries,
transactions, and migrations. Unset it for normal operation without trace formatting overhead.

Find Articles (Cmd-F) activates the native search field at the right of the desktop toolbar.
Its edits drive the same FTS query as programmatic search. Title and body indexes are queried
on the worker. Row scrolling follows a changed selected article ID; inserting newer rows above
the selection does not repeatedly jump the scrolling list.

The worker drains accepted edits at termination. Errors are delivered to the UI instead of
being reported as success. Unit tests cover search punctuation/title/body matches, concurrent
refresh/read edits, per-search subscriptions, sticky unread rows, sidebar counts and deletion
of the currently displayed article. Framework tests cover transaction/cancellation/close races.
The synchronous database API is still used by the headless store tests and the web tier.

## Coordinating publisher refresh with WebDAV

Native Refresh first joins or starts one day-sync pass, waits at most ten seconds, then
selects active subscriptions from the database worker and fetches publishers (at most four
concurrently). The selection happens after the wait: newly received subscriptions enter the
same refresh, and removed subscriptions are excluded. Each queued fetch rechecks that its
subscription still exists and is active before contacting the publisher.

The wait is an ordering preference, not a connectivity requirement. A failed, paused, or
slow sync leaves publisher refresh available; the refresh result says library sync is pending.
A timed-out sync continues independently. Retained read/star choices apply transactionally
when matching articles arrive, including when sync completes after a publisher fetch. Article
content remains local, so completing a publisher refresh does not require a second sync pass.

Any sync pass can discover new or reactivated subscriptions. These receive an initial fetch,
even with automatic polling disabled; subsequent fetches follow the Refresh Feeds setting.
During an existing refresh, new subscriptions join a following batch without duplicating
feeds already selected. Otherwise, a targeted refresh starts without another sync pass.

Automatic polling and foreground activation respect per-feed due times, the selected refresh
interval, and sync retry backoff. Manual Refresh may explicitly retry sync, but preserves an
account's paused state. With no due publishers, the minute scheduler does not initiate a sync
pass. Independent lightweight state sync still publishes reading/subscription edits and checks
for remote changes. Edits made during a pass retain their follow-up request; a successful pass
with deferred work schedules another attempt, while failed attempts retain server backoff.

Regression coverage lives in `native/refresh/tests.rs` and `native/webdav.rs`: real SQLite
replicas exercise subscription changes and retained article choices; controlled futures cover
joining a pass, timeout, late completion, failure, and preserving follow-up requests.

## Account synchronization

Feedbin is an optional legacy provider, enabled only by the `feedbin` Cargo feature. Normal builds do not load its session, access its credentials, or offer its account controls. Existing account records, article identities, and queued intentions are preserved. Disabling the feature does not migrate them to WebDAV.

With that feature enabled, Settings → Accounts validates the login, previews
subscription counts, and asks whether to upload eligible local subscriptions (off by default).
The initial import adopts matching feed URLs and uses Feedbin's read/unread state for matched
articles. Unmatched local history remains local. Subscribing, renaming, unsubscribing, reading,
and marking a scope read/unread all use the same durable journal. There is one active account
per installation. Collections (the store's folders) sync as Feedbin taggings, described below;
stars, tags, and reader preferences remain local. Web DOM retains its
existing local-only behavior and does not expose the account screen.

### Collections and taggings

A collection is a folder in the store, and a feed belongs to at most one. Feedbin's equivalent is
a tagging, and a feed may carry several there. Moving a feed queues one coalesced intent per feed
that replaces its taggings; renaming or deleting a collection renames or deletes the tag, and only
when the service has that tag. Each cycle downloads the complete taggings list: a linked feed
goes to its alphabetically first tag's folder (created when missing) and leaves its folder when it
has no tagging. A feed with a move still pending keeps its local folder, and pending renames and
deletes apply to the downloaded names first. Downloads never delete a folder and never touch
unlinked feeds. Collection order and each collection's sort stay local, since Feedbin has neither.

### Provider boundary and reconciliation

`daynews-sync` contains a protocol-independent `Provider`, normalized subscriptions/entries,
read snapshots or deltas, paginated results, per-operation outcomes, and a `Storage` interface.
IDs, pagination cursors, and durable delta checkpoints are opaque strings. A provider restores
its last committed checkpoint at the start of each cycle; the worker commits the next checkpoint
only after all imports succeed. Feedbin uses complete subscription and unread-ID
snapshots plus a transactionally saved ETag for the first article page. An unchanged page
returns 304 without redownloading bodies; subscription membership changes invalidate this
validator so older history from a newly followed feed is still downloaded. A second protocol test uses nonnumeric IDs, explicit
read deltas, and an opaque checkpoint. Adding another backend requires its adapter, authentication
presentation, and registration in the core provider factory; article commands and database
reconciliation do not depend on Feedbin endpoints or numeric IDs. Dynamic binary plugins are
not part of this interface.

`daynews-db::sync` owns account records, remote-to-local identity links, and coalesced intents.
Credentials never enter SQLite. A local article edit and its monotonically revised intent are
saved in the same database-worker transaction. The cycle sends pending changes first, imports
subscriptions, applies reading state, downloads article pages and missing unread entries, and
then reconciles reading state again. Local pending intent overlays remote snapshots. Each
acknowledgement removes only its exact revision, so an older response cannot erase a newer edit.
A failed individual operation remains queued without blocking unrelated downloads. A completed
import persists its checkpoint even when an independent upload is rejected, so that failed
intent cannot cause another full history scan. Canceled or incomplete imports never advance
the checkpoint. Progress counts newly inserted articles separately from downloaded records;
known entry links are only rewritten when their identity or confirmed state actually changes. Bulk marking
captures the current article IDs; articles arriving later are not accidentally marked read.

Pending local read/unread intent wins until acknowledged; after acknowledgement the server's
current state wins. Feedbin supplies no global edit timestamps for stronger conflict ordering.
Remote unsubscribe removes the feed from active views while retaining cached articles and identity
records. An offline client's stale rename does not resurrect it. Explicit subscribe and undo can
restore it. New subscriptions invalidate the known-history stopping point, so older read articles
from a newly followed feed are also imported. Article matching adopts only a unique URL match
inside the corresponding feed; titles and dates are not identity. Unmatched remote articles get
account-scoped IDs. Synced feeds are excluded from direct publisher polling.

Remote imports and journal metadata have non-user authors, keeping acknowledgements out of undo
history. Undo/redo restores visible state and reconciles it into fresh intents rather than
rewinding network state. Tests exercise the real undo stack and a worker transaction that fails
after changing both article state and the journal.

### Native lifecycle, credentials, and HTTP

`daynews-core::native::sync` owns one session across all app windows. It restores a connected
account on launch, synchronizes while active every minute, coalesces local edits, and offers
manual refresh. Only one cycle runs at a time. Generation checks discard completions from a
previous login or disconnected session. Authentication failures pause automatic retries until
sign-in; transport failures back off, and HTTP Retry-After is respected. These are foreground
application tasks, not OS background-fetch registrations.

`daynews-credentials` uses Keychain on Apple platforms, Credential Manager on Windows, and Secret
Service on Linux. The Linux backend uses Rust DBus/crypto implementations, avoiding extra C
build dependencies; a running, unlocked Secret Service is required at runtime. Android stores
AES-GCM ciphertext in private preferences with an Android Keystore key and account-bound AAD.
HarmonyOS uses Asset Store Kit. The mobile bridges report storage errors without including
secret values. There is no plaintext fallback. Disconnect disables synchronization and deletes
the saved credential while preserving downloaded data and the remote account.

Feedbin v2 uses HTTPS Basic authentication through `day-part-http::Client` and the platform's
native HTTP stack. Sync requests disable caching and request network reloads: the live rename
probe caught an immediate subscription read returning a cached old title. Redirect following is
disabled, pagination input cannot become an arbitrary
credential-bearing URL, requests time out, and buffered responses have a 64 MiB limit. Read-state
writes are batched at 1,000 IDs. Errors and cycle statistics expose categories/counts/timing,
not authorization headers, response bodies, or passwords. `keyring` supplies mature native vault
implementations; `chrono` parses provider timestamps, and `httpdate` handles Retry-After dates.

### Reproducible validation

The trial account was tested through the production Day HTTP client and the native AppKit UI:
authentication, a sample Rust Blog subscription, 401 imported articles, read/unread writes,
idempotent repeated writes, subscription rename/create/delete, and credential restoration after relaunch. The read-state probe
restores the original state. NetNewsWire's Feedbin caller, account delegate, and status-send tests
informed the durable-journal and partial-acknowledgement behavior; this implementation is original.

`daynews-sync::fixture::Server` implements sanitized synthetic request/response exchanges based
on those live responses: auth 200/401, subscription 201/302/204, entry pagination via Link headers,
unread ID arrays, per-ID write acknowledgements, and the taggings and tags endpoints that carry
collections. It uses Day HTTP `Simulation`, not a fake
Provider, and never embeds account credentials or downloaded article content. The tests under
`daynews-sync/tests/protocol.rs` and `daynews-db/tests/synchronization.rs` therefore exercise the
production HTTP adapter, including two replicas, restart, partial failure, rollback, undo,
invalid snapshots, batching, and redirect refusal. They run with `cargo test --workspace`. CI also runs both account dayscripts after the existing
reader walkthrough on native targets; their steps explicitly skip web DOM. The two HTTP fixture
sets compose in one Day simulation.

For native UI validation, launch `dayscript/feedbin-sync.yaml` with `DAY_NEWS_SYNC_TEST=1` and a
fresh `DAY_DATA_DIR`. Relaunch against that same store with `dayscript/feedbin-resume.yaml`
to verify restoration, disconnect, and invalid credentials. Also set
`DAY_NEWS_SYNC_TEST_SECURE_STORE=1` on both launches to exercise the real native credential vault;
otherwise the explicit fixture mode supplies only the synthetic test password. Ordinary launches
never enable interception or that fixture credential. AppKit, iOS, Android, GTK and Qt flows have
been driven and inspected locally. HarmonyOS is compile-only locally; Windows and Linux vaults
are cross-checked, with native runtime validation left to those hosts. The live example accepts
an external owner-only JSON credential file; never pass passwords on the command line or commit
live scripts, account data, or credentials.

## Reader

The article pane uses a native WebView with a per-window application resource provider.
The app renders its document on the UI thread and publishes an owned HTML snapshot; native
resource callbacks read that snapshot without accessing reactive UI state. Every window has
its own resource namespace. The browser resource channel serves the same document on web-dom
without `file://` navigation, and resource-mode link handling dispatches reader actions across
platforms. The document uses no external CSS or fonts. Feed HTML is sanitized at the parse
boundary rather than trusting the renderer.

### Load Full Content

`ArticleExtractor` (`src/extraction.rs`) returns an owned `ExtractedArticle` from an article URL.
It is independent of selection, commands, database storage, and presentation; an authorized
HTTP service or an offscreen engine can replace `LocalReadability`. The current provider uses
Day's native HTTP stack with the FleetReader user agent, validates status/content type, decodes the
response charset, and resolves links against the final redirect URL. It rejects documents above
5 MiB before parsing and limits Readability to 50,000 elements. This is a parse limit, not a
streaming download limit. The command has a 30-second deadline.

Bundled Readability 0.6.0 and DOMPurify 3.4.16 run in the existing reader's JS engine on a
detached document; the publisher page is never navigated to or executed. HTML is sanitized both
before extraction and before returning it to the reader. All bundled scripts use generated
asset accessors. JS-only and authenticated pages are not supported, and web HTTP requests
remain subject to CORS. A backend without JS evaluation disables the command.

`reader_scripts` persists copied UTF-8 source in the versioned `news.reader.scripts.v1`
preference: one app default and a sorted map of exact origins. The provider selects a script
using the final fetched URL. Editors in Settings and the feed overview share these defaults;
a revision signal cancels active extraction and invalidates per-window overlays on save/remove.
The optional synchronous `extract(document, url, Readability)` hook runs inside a local function
scope with a sanitized document clone and the unchanged bundled Mozilla constructor. `null`
delegates to Mozilla on the original detached document. Returned content is sanitized again;
invalid results and script exceptions use localized error guidance. This is trusted user code,
not an isolation boundary; a synchronous infinite loop cannot be interrupted by the Rust timeout.
`tests/reader-extraction.cjs` covers the adapter and `dayscript/reader-scripts.yaml` covers the
native editors, precedence, removal, errors, and invalidation of an already displayed article.

`ReaderView` is ambient per-window state. The command toggles a temporary full-text overlay;
it never writes over stored RSS content. Repeating it while loading cancels, while repeating it
after completion switches between the two versions without refetching. Changing article ID or
URL cancels the task and clears the overlay. Window disposal cancels pending work. Errors leave
the RSS version visible, with localized retry guidance. Reader styles and external-link handling
are shared by both modes. Command-Shift-R belongs to Load Full Content; Refresh Feed no longer claims
that shortcut. A toolbar command exposes the same action on mobile.

Reader documents are served by a per-window `ResourceProvider`, not temporary files. The
provider only reads a thread-safe snapshot of HTML localized on the UI thread. Resource-mode
link handling dispatches inline actions on all WebView hosts using the same contract as the
piece demo; the reader does not depend on ordinary-document link interception support.

Inline Reader Mode defaults to on. The RSS preview document contains a localized “Load Full Article” bar and a full-content slot. Activation goes through the WebView link policy and
validates the selected article before starting extraction. Sanitized full content is appended
in the same live document; the feed preview and document state remain intact. Turning inline
mode off retains the replacement reader presentation. Auto-load Reader Mode defaults to off;
when enabled, extraction starts when its load bar enters the viewport, once per
article. Returning manually to the RSS preview does not immediately restart automatic loading.

Settings → Reader holds the app-wide defaults (`READER_INLINE`, `READER_AUTO`). Each feed
dashboard exposes independent inline and auto-load overrides, persisted by stable feed ID in
Day preferences. Feeds without overrides
inherit the defaults, and changes propagate across windows. Selection changes/disposal retain
the cancellation and stale-response checks above. The feed controls sit under the feed's name
as one aligned form (Day's `form` and `labeled` in a grid: two columns, one on a compact
width), and the dashboard's chart space contracts to make room for it inside the fixed
viewport.

NetNewsWire uses signed Feedbin extraction requests with a client ID and secret; no unauthenticated
or User-Agent-whitelist integration is provided. Feedbin access requires credentials authorized
for this app. Provider replacement requires no change to the reader state or rendering.

`tests/reader-extraction.cjs` exercises the bundled scripts with synthetic publication HTML.
`tests/reader-view-server.py` plus `dayscript/reader-view.yaml` cover native success, cached
toggle, cancellation on selection, and HTTP failure using an isolated database.

Reader snapshot updates compare article identity explicitly before resetting extraction:
marking the same article read or starred preserves its live document, downloaded content,
and scroll position. `watch` callbacks may run even when the projected identity is unchanged.
Subscription sheets remain alive until the new feed projection and selection settle, then
dismiss; DOM observers must not be disposed while this asynchronous selection is pending.
The floating feed heading is hidden at the first row, where the static heading already names
the group, to avoid covering the first article title. Android explicitly allows cleartext
traffic so a user-provided HTTP feed is reachable; HTTPS feeds continue using HTTPS.

Browser extraction fetches its generated Readability/sanitizer assets from the application
origin; native backends use the synchronous bundle resource opener. The DOM WebView reports
completed loads so the same visibility-triggered reader policy is armed on every toolkit.

See [native interaction regression tests](tests/native-interactions.md) for the six locally
tested targets, isolated-store setup, native-input coverage, and test limitations.

## Looking like a reader

The layout was measured against NetNewsWire's own macOS window rather than from memory. What
that comparison changed:

- **Row order.** Title, then summary, then a feed·date footer — headline first, provenance last.
  The feed name had been on top, which buried the one line a reader actually scans.
- **Type.** Day's semantic steps only (`Body` / `Footnote` / `Caption`), never a hardcoded point
  size: those steps follow the reader's accessibility text-size setting, and a timeline that
  ignores it is unusable for the people who change it.
- **Read state.** A read article dims its title rather than only dropping its dot. Selection is
  the platform's own: the timeline is a native `list`, so the table draws the highlight and owns
  the arrow keys, and rows keep their content colors instead of inverting by hand.
- **Separators** between rows, drawn by the HOST at the row boundary (`.separators()`). At
  this density adjacent titles and summaries otherwise merge into one undifferentiated column of
  text — but a hairline drawn *inside* the row is the wrong tool: it never lines up with the
  native selection, and it sits still while a swipe slides the row past it.
- **Swipe actions** on both edges where the platform has them (macOS row actions, iOS swipe
  actions): trailing toggles read with a filled/outlined circle, leading stars. The offer is
  pulled at gesture time, so the button names the flip it is about to make.
- **A heading** over the list — the scope's name and its unread count — so the pane says where
  you are instead of opening with a bare row of controls.
- **Sidebar glyphs**, drawn as vectors in `resource/images/sidebar_*.png` (template PNGs, tinted
  by the app per row — a warm sun, a blue unread dot, a gold star — so the smart feeds read
  apart at a glance). Not SF Symbols: that license does not cover redistributing them onto
  Android, GTK and Qt.
- **Window size.** 1440x900, near NetNewsWire's own. Three panes at 960 leave the timeline and
  the article both too narrow to read. Note that `[window]` in `Day.toml` is inert — only
  `day metadata` reads it; the size that takes effect is the one in `day::launch`.

## Sidebar and menus

The sidebar opens on four smart feeds — Today, All Unread, Starred, All Articles — above one row
per subscription and one per tag. Unread counts are real badges (`.badge(…)`), right-aligned and
de-emphasized by each toolkit, and the three blocks sit under their own `.section(…)` headers;
both were gaps in Day's `nav` when this app started and were built in the framework rather
than faked in the row label. The mobile renderers display the same badge strings, including
All Articles' total count. Counts use Day's tracked locale-aware decimal formatter so digit
grouping changes with the locale. On mobile, a Settings gear in the trailing toolbar opens
Day's modal preferences surface; Settings is no longer a feed navigation row. The unread-only
filter uses a funnel with an unread dot and updates the rows without activating a destination.

`watch` fires on CHANGE, so the shell applies the opening scope itself (`OPENING_SECTION`).
Without that the sidebar highlighted Today while the timeline still showed the scope the store
opened with — invisible on a store whose newest unread articles are all from today, and obvious
on a stale one.
The selection watcher and sidebar toolbar factory retain their owning reactive scope and enter
it before resolving a scene or registering commands. Reactions can drain outside that scope:
rebuilding the toolbar while web Settings has focus otherwise panics because the preferences
surface has no `NewsScene`, and another window's context could redirect a selection command.
The timeline heading reads its captured scene's scope rather than looking up a scene during
reactive updates.
The navigation regression covers both an unrelated scene and no ambient/focused scene, and the
browser walkthrough closes the modal preferences surface before resuming feed commands and
checks the feed heading after returning from Settings.

Desktop gets a File menu (New Feed, New Collection, New Window, Refresh, Import/Export Subscriptions,
Close Window), a Go menu (Previous/Next Article ⌘[/⌘], Next Unread ⌘/, then the three smart feeds),
a Feed menu (Collection, Rename Feed…, Refresh Feed, Mark All as Read, Unsubscribe) and an Article menu. These are one live `app_menu` model,
so all four desktop toolkits get the same bar from the same code, and dayscript drives them by
Fluent key on every one. Next Unread walks the visible timeline forward and wraps, then falls back
to any unread article, so it keeps working when the current scope is exhausted.

The Feed menu acts on the feed the sidebar has selected, and does nothing when a smart feed, a tag
or a page is selected, the same rule the Article menu follows with no open article. Each feed row
also carries a context menu with the same three commands (`item(…).context_menu(…)`), aimed at the
row that was right-clicked or long-pressed. Both call the same daynews-core functions, so the
walkthrough's Feed-menu steps cover the row menu's actions too; `menu:` steps reach only the app
menu, so the row menu itself is checked by hand.

Renaming a feed has three ways in: Feed ▸ Rename Feed… (disabled with no feed selected), Rename…
in the feed row's context menu, and the Rename… button in the Name row of the feed's overview
form. All three open one prompt (`feed_list::rename_feed`) that starts from the current title and
ignores an empty or unchanged answer. The new title is the feed's custom title: refreshes keep
it, and a connected Feedbin account gets it as the subscription's title. `dayscript/feed-rename.yaml`
drives each way in; on AppKit, whose sidebar is a native outline, the row menu step is skipped.

## What real feeds taught us

Each of these is a test, because each was a wrong assumption first:

- **Subscriptions can have no title.** A subscription that has never been fetched records no
  name at all, so readers show the host until the first refresh supplies one. The vendored OPML
  fixtures include that state, and `daynews-opml`'s tests assert the fallback.
- **Microblog items have no `<title>` at all.** Mastodon posts carry only a body, so a title is
  derived from the content.
- **Feed text is escaped twice.** A description holding escaped HTML has its own entities escaped
  again, so `&` arrives as `&amp;amp;`. The second decode is applied only when tags were actually
  found, so prose that merely mentions `&amp;` is left alone.
- **WordPress lists the feed itself first**, so "open website" needs a link that is not the feed's
  own URL.
- **The whole article can live in `description`.** RSS 2.0 allows HTML there, and many feeds send
  no `content:encoded`, so an HTML description with nothing richer beside it becomes the article
  body; flattened to text, the reader ran every paragraph together. A plain-text description stays
  a summary.
- **`&#149;` means a bullet.** Numeric references 128–159 name C1 control characters in
  Unicode, but publishers write them for the Windows-1252 characters at those byte positions
  (Merriam-Webster separates its pronunciations with `&#149;`), and browsers read them that way.
  The XML layer resolves the reference before the parser sees it, so `daynews-feed` remaps the
  raw controls in titles, summaries and bodies; a control left alone draws as a box on Android.

## Not yet built

Additional synchronization providers (iCloud/Reader), per-feed refresh intervals,
starred-article sync, images cached offline, and article pagination beyond the 500-row timeline
cap.

Two want Day itself to grow first: an HTML-content API on `day-piece-webview`, without which
the web build's article pane stays blank (it has no filesystem for the generated document), and
self-sizing list rows — `RowHeight::Automatic` is a fixed default on every backend today, which
is why the timeline pins a uniform pitch and why a wrapped title can clip its footer on Android.

### Feed refresh and sidebar identity

The mobile feeds root uses Day's `Nav::refreshable` with native UIKit/Android pull controls.
Its busy state combines RSS refresh and account synchronization, so the indicator ends on
completion, failure or cancellation, and pulls during active work do not start duplicates.
Explicit refresh commands and pulls resume paused account sync; automatic timers respect
cancellation. Other platforms retain their Refresh command. Empty article scopes remain
navigable and display a localized synchronizing message while work is active. Smart-feed
reselection clears the reader just like feed reselection; Day presents the resident middle
column on every compact native row activation, including an already-selected route.
`tests/ios-navigation` exercises actual taps and slow-network pulls; route injection alone
cannot establish that UIKit brought the destination onscreen.

`daynews-feed::fetch_cached_with_progress` returns either a parsed feed with replacement HTTP
validators and a SHA-256 fingerprint, or `NotModified`. A 304 merges supplied validators with
stored ones; a byte-identical 200 skips parsing, sanitization and article existence queries,
but still replaces validators (and clears ones absent from that 200). This avoids stale
validators when a publisher changes ETag/Last-Modified without changing feed bytes. `daynews-core` stores those validators on
`Feed` alongside the articles, using nullable columns for existing libraries. 304 updates the
last-check timestamp and clears errors without touching bodies/read/star state. Failed HTTP
or parsing attempts never advance validators or fingerprints. The fingerprint is persisted
atomically with successful article imports, and nullable fields migrate existing libraries.
Equal metadata values do not emit redundant model writes. Empty imports skip their existence
query; duplicate GUIDs within one response are inserted once. HTTP 429/503 with a valid
Retry-After (seconds or HTTP date) persists a deadline; automatic and manual feed checks skip
that origin until the deadline expires, including across restarts. A successful check clears
it. This is per subscription, not a host-wide circuit breaker. The shared HTTP client reuses connections;
native parsing and sanitization use worker threads. Four shared permits bound all feed
requests, while an in-flight ID set prevents overlapping subscription/manual refreshes.
Deleted subscriptions are checked before requests and before storing results. RAII clears
activity and releases permits on completion or cancellation. `SheetsState::updating_feeds`
is an active-ID map whose optional fraction drives `Nav::icon_progress`, separately from
row labels, badges and icons. The icon overlay spins while connecting or when the body length
is unknown/encoded, then fills a circular ring for a trustworthy decoded content length.
Progress updates are throttled to 100 ms (mode changes and completion are immediate).
Finishing, failure, removal and cancellation clear the overlay without changing row geometry.
At orderly exit, Day cancels pending main-loop tasks before delivering `WillTerminate` to the
app. This drops `UpdatingFeed` guards while their signals are alive, before `shutdown` drains
accepted database writes and closes the worker. Stopping the refresh timer alone does not
cancel the separate refresh tasks it launched. The native regression
`quit_cancels_pending_feed_guards_before_closing_the_store` exercises four pending guards,
database closure, and actual thread-local destruction in a subprocess.
AppKit/UIKit animate only native Core Animation layers; download ticks never rebuild rows.
Other toolkits currently omit this optional per-icon decoration; aggregate refresh remains
visible on every platform.

`feed_icons` owns discovery, the local thumbnail cache and its separate four-worker queue.
All cached thumbnails are loaded before queued network discovery. Each source consists of
feed/home/icon URLs; source changes invalidate queued results. Batched publications avoid a
sidebar rebuild for every individual image. Untrusted responses and decoded image dimensions
are bounded. Successful thumbnails remain visible after transient rediscovery errors.
AppKit/UIKit display absolute-file icons in original color; generated bundled RSS symbols
remain the fallback. `feed_list` owns the persistent unread-only signal used by both the
menu and toolbar. Filtering changes visible subscription rows, not the scene's article scope.
The app-wide signal binds to `news.feeds.unread_only` before the sidebar reads it, restoring
and saving both boolean states. `feed_list::tests::unread_filter_survives_process_restart`
checks enabled and disabled restoration in separate processes.


## Worker migration validation (2026-10-01)

The migration was checked against isolated test libraries, not the user's active database.
Fleet Reader's AppKit walkthrough passed 127 executed steps (2 platform skips); UIKit passed 97
(32 skips). Native AppKit Cmd-F and actual keyboard typing were verified separately, since
synthetic toolbar events alone do not exercise NSSearchField's action delivery.

Fleet Reader builds passed for AppKit, UIKit, Android, GTK, Qt, web and HarmonyOS. Harmony was
compile/package checked only; no emulator was run. Its local build needed NODE_PATH pointing
to the installed Hvigor modules and a temporary versioned SDK root because the machine's SDK
symlink was stale. Windows/XAML was not built on this macOS host.

Framework validation includes 38 worker tests, plus existing model, persistence, core and spec
suites. The worker suite also passed ten consecutive runs to vary thread scheduling. Fleet Reader's
77 headless tests include six native worker/projection regressions. The Showcase query script
passed 48 steps; Sketch's editor script passed 640 executed steps (15 skips). Stanza-Redux's
27 tests, Sketch's 109 tests and day-lite's 13 tests passed. Compatibility checks also passed
for App-Fair, Games-Fair, Day-Tunes, Day-Bench, Day-Rise, Day-Skies and Day-Trader. App-Fair needed
small updates from the retired selector/stack builders to nav/nav_stack and an explicit image
conversion. These checks do not imply that the apps retaining synchronous containers have
migrated every query to a worker.

### Reading navigation and link actions

Previous/Next Article resolve against the current scene's lightweight timeline snapshot,
including search filters, and use the selected ID rather than the asynchronously loaded body.
Rapid commands therefore advance from the latest selection. They never wrap or cross scopes;
missing selections disable movement, while no selection starts at the first/last visible row.
The existing selection binding scrolls and highlights the native row. Both commands live in
Go and in the reader toolbar; Next Unread has a distinct double-chevron image.

On iOS, Android and HarmonyOS, the reader uses `day::pager`: a native horizontal
page container with three stable slots for the current, previous and next articles. UIKit
uses `UIPageViewController` (the same native scrolling architecture as NetNewsWire), Android
uses AndroidX `ViewPager`, and Harmony uses ArkUI `Swiper`. A drag reveals the neighboring
live document and can reverse or cancel interactively. Desktop retains its single reader.

`reader_pager::Window` reconciles the scene's current filtered timeline by article ID. Slots
shared by successive windows retain their WebView, scroll, and extracted Reader Mode content;
only the far neighbor is recycled. This bounds retained article pages to three, rather than
accumulating a reading history. The native pager owns animation frames. Its busy signal freezes
slot reassignment until settlement; epoch-tagged completions reject obsolete presentations.
Only a completed change calls `open_article`, so preparing a neighbor does not select it or mark
it read. The asynchronous `preview_article` reads a snapshot through the persistence worker;
stale/cancelled results cannot fill a reassigned slot. Missing/failed snapshots have localized
empty states. Preview HTML may load publisher images, but automatic full-article extraction
is enabled only for the selected, visible page.

Each page has a scoped `ReaderView` and projected scene. The window's `ReaderView::shown`
forwards toolbar/menu actions to the selected page. Hiding a page cancels pending extraction
without discarding completed content. Returning to an adjacent article preserves its position;
a page recycled out of the three-slot window starts fresh. The old document touch recognizer
is removed, so native paging cannot also trigger a second JavaScript navigation. Native back
edges, vertical WebView scrolling and native touch arbitration remain platform-owned.
On iOS, Day's opaque navigation-bar appearances keep scrolled article text out of the
status/title/control area, including compact landscape bars. The native system background
tracks light/dark appearance. `dayscript/reader-header.yaml` captures the top, middle and
bottom of a scrolled demo article for visual regression checks in both appearances.

`dayscript/reader-swipes.yaml` tests the shared selection/retention contract; actual gestures,
cancellation and boundaries are covered by `tests/ios-navigation` and
`tests/android-reader-paging.py`. Harmony runtime gesture validation belongs in CI.

Copy Article Link uses the displayed article URL and Day's asynchronous clipboard API, started
inside the user action for browser permissions. It is available in Article and the secondary
reader toolbar. Article context menus are built when summoned, show state-appropriate actions,
and snapshot the clicked row's ID and URL so recycled cells cannot retarget an open menu.
Clipboard failures use a localized alert. Missing/blank URLs disable link commands.

Refresh cache regressions live in `crates/daynews-feed/tests/conditional.rs` (real loopback
requests with validator rotation on identical 200 and 304), the feed crate's response tests,
and `crates/daynews-db/tests/store.rs` (durability and unchanged-field writes). Run
`dayscript/conditional-cache.yaml` against `tests/feed-refresh-server.py` with an isolated
`DAY_DATA_DIR`; `/stats` verifies request counts and the busy-origin pause.

Conditional requests save response bodies, not the request itself. Servers without validators
still transfer bodies, although equal fingerprints avoid repeat imports. Last-check metadata
is still durably recorded and native worker observations may recompute unchanged projections.
The current policy does not yet skip automatic polls based on HTTP freshness or adapt the
interval to a feed's publishing frequency. Web cross-origin validators require the publisher
(or proxy) to expose ETag/Last-Modified through CORS and allow conditional request headers.

### Adaptive refresh and feed groups

Refresh Feeds now defaults to Automatic; existing explicitly chosen intervals remain intact.
A minute tick queries indexed per-feed deadlines, not articles. Successful imports retain
32 distinct valid publication dates independently of article retention. The median of the
last eight publication gaps determines cadence; recent frequent feeds poll every 30 minutes,
daily feeds about every four hours, and weekly/monthly or stale feeds at most daily. Missing
dates default to two hours. Quiet feeds gradually slow, new publications accelerate polling,
and failures back off exponentially (one hour initially, capped at a day). Retry-After remains
a minimum delay for both automatic and manual requests. Deadlines and samples survive restart.
Existing caches without samples receive one unconditional download to initialize the policy,
then resume conditional requests and unchanged-body shortcuts.

Group by Feeds is a persisted preference, also available in the Feed menu and desktop list
toolbar. The existing bounded result window is stably sorted by subscription priority, with
chronological articles inside each group. Each group is a section of Day's native list
(`list(..).sections(..)`), whose header the platform pins while its articles scroll; headers are
never selectable and never change article indexes.

### Collections

Feeds sit in collections, the store's folders. The sidebar shows one section per collection in
the user's order, titled "Name (feeds)". The default collection ("Feeds" until renamed) holds the
feeds in no collection; it has no folder, so its name, place and sort are preferences
(`news.collections.default.*`), and it never syncs. Dropped last in the order it stays after
every collection, including later ones. A feed's Collection menu (on its sidebar row, under Feed ▸ Collection, and as the
feed overview's picker) lists None, every collection and New Collection…, which asks for a name.
Settings ▸ Feeds lists the collections, the default one marked with a Default capsule: drag them
to reorder, rename them, delete any but the default (a deleted collection's feeds move to the
default collection), and choose each one's Sort Feeds: Priority,
Recently Updated, Name, Unread Count, Total Count or Manual. Every sort is stable, so ties keep
priority order. The counts and newest-article dates come from one grouped query beside the
sidebar snapshot.

Two orders coexist. Priority is global: refresh order and the grouped timeline's group order,
changed by dragging feeds in the default collection while it sorts by Priority and by Move Feed
Up/Down in Priority sections. Manual is per collection (`Feed::collection_position`; the default
collection keeps one too), changed by dragging within a Manual collection and by Move Feed
Up/Down there. The sidebar uses Day's `Nav::regroup_items`: a feed
drags into any other collection (joining it), and within its own section only where the user sets
that order; other sorted sections refuse the drop.
The separate Subscriptions page is removed. File → New Feed and the sidebar add button open
an attached URL prompt, prefilled only with a valid clipboard HTTP(S) URL. A direct feed is
parsed once; websites advertise candidates through Link headers, alternate links, feed metadata,
or conventional feed anchors. Relative links honor the final response URL and HTML base.
Missing feeds show an alert; multiple candidates require a choice. Only the selected candidate
is fetched. The parsed response, validators and body hash are imported without a duplicate fetch.
Subscription completion selects that feed and clears the previous article search. OPML controls
remain in Settings → Feeds and the File menu. The common article document used for feed bodies and
readability content shows the full resolved href in a pointer-following tooltip. Tooltip content
uses textContent, not HTML; scrolling, leaving, clicking or blurring hides it.
Regression coverage lives in daynews-db polling/store tests and the grouped-feed DayScript.

### Scope dashboards

An unselected reader is a native viewport-sized dashboard built from day-piece-charts.
Individual feeds show one settings card under the hero: the form (name, collection, inline and
auto-load reader modes, reader script, site data), the feed's automatic-refresh schedule and
countdown, and Refresh Now and Unsubscribe… (which confirms first). Article, unread, reading-time
and contributor counts follow, and the word cloud fills the rest of the page; a feed's overview
has no activity chart, publisher donut or separate forecast card.
Today shows local-hour arrivals; Unread emphasizes backlog age; Starred maps the saved
collection over publication time. Smart-feed dashboards set the word cloud beside a publisher
donut and counts.
Publisher legend links select their feed in the sidebar and navigate to its overview; hovering
a link highlights its row and matching donut segment. Publisher navigation reopens the detail
layer after the scope change, including on compact mobile stacks. The donut and legend
share a horizontal region with reserved widths; legend rows cannot consume the donut
height in a short CI capture window. Stable feed IDs keep identically named
publishers separate. The shared day-piece-charts interaction grammar binds the legend and donut to one
series-projected point parameter. Conditional opacity provides hover emphasis; registered
link handlers select a publisher from either surface, with pointer cursors on its hit target. Publication plots offer date/count guides through composable Inspect bindings.
These readouts use the plotted metadata without further database queries.

The word cloud counts the scope's words straight from the search index: an `fts5vocab`
instance table over `articles_fts` (`articles_fts_terms`, created at open on the writable
connection, since the worker reads on a read-only one) lists each title and summary term with
its article, and one SQL statement counts them per feed. A term in more than half of a feed's
articles (a feed of eight or more) is that feed's boilerplate, "Comments URL" under every link
an aggregator posts, and does not count for it. Stopwords come from the vendored stopwords-iso
lists (`crates/daynews-db/data/stopwords/README.md`): every language whose list covers at least a
quarter as much of the top terms as the best match, falling back to the reader's language, plus
HTML leftovers. The index folds case and accents, so each kept term is shown in its most common
original spelling from the scope's titles and summaries, lowercase when that is seen a third as
often as the top spelling ("école", but "NASA"). The app caches clouds by scope, local day,
article count and language, so read marks and rebuilt overviews never recount. The layout is
Wordle's (and d3-cloud's): words largest first along an Archimedean spiral stretched to the
card, type size by the square root of the count, the whole cloud shrinking until its larger
words fit. Settings ▸ Articles chooses across and up (the default: two words in five upright,
each trying both directions at every step) or across only, and the face: the system's, weighted
by size, or one of nine bundled display faces from Google Fonts
(`resource/fonts/licenses/README.md`). Choosing a word fills the search field with it, which
searches the selected feed or smart feed; a phone returns to the list to show the results.

A search matches article bodies by id: a body shares its article's id, so the body index's
matches are an id list SQLite builds once (`Pred::Raw`), where crossing the `body` relation
compiled to a subquery correlated per article and made a common word five to fifteen times
slower than a rare one. Debug builds optimize the bundled SQLite alone
(`[profile.dev.package.libsqlite3-sys]`): unoptimized it ran the word count and searches about
seven times slower.

On a Mac only Refresh and Show Only Unread Feeds sit over the sidebar; Next Unread, the
overview, New Feed and Group by Feeds sit over the article list. AppKit folds the toolbar's
search item into a magnifier whenever the sidebar's part of the toolbar holds more than two
items beside its toggle, however much room the rest has (measured on macOS 26 with a plain
`NSSearchToolbarItem`), so the light sidebar part keeps the native field open.
All plots describe the full selected scope, independently of the 500-row timeline window
and search. Empty scopes display honest zeroes rather than synthetic chart data. The Feed
overview toolbar action makes dashboards reachable on compact navigation stacks and lets
a reader return from an article to its overview. Selecting an article disposes the dashboard.

Native dashboard projections live on the existing database worker; browser projections use
the existing OPFS transport. Word counts are computed at article import and backfilled once
for older rows in batches of 128, with an indexed nullable metric. Charts never fetch bodies.
A per-dashboard cache keyed by article-store version, scope and local day reuses article
aggregates when a conditional feed check changes only scheduling metadata. Feed deadlines
continue to update. The dashboard owns a scope-lifetime one-second clock for countdown text;
ordinary countdown ticks perform no SQL or HTTP work. A local-midnight change refreshes
the projection once so Today and age bins stay current. Publication bins use host-local
time, and axes, numbers,
units, headings and countdown components use generated localized accessors. Reading time
is an estimate at 240 words/minute, measured from stored article text, not a downloaded full
web page; HTML markup and scripts are excluded from the word estimate.

The layout divides the offered content bounds among header, metrics, three flexible chart
cards and forecast. It has no scroll container, and layout never writes reactive state.
As the viewport shrinks, the header, metrics and forecast yield more space to the chart
cards. This keeps their flexible plots from collapsing after titles and padding consume
the available height, as seen in CI's small desktop windows and Android tablet layout.
The hero description stays on one line so its title and actionable site URL both remain
visible in compact panes, including Harmony's phone layout.
Length distributions use horizontal bars to keep bucket labels readable in narrow panes;
day-piece-charts thins colliding axis labels against measured text bounds. Charts use its
built-in spring transitions for initial appearance and incoming metadata, growing bars,
areas and wedges from their baselines and blending heat-map colors. Countdown ticks do not
restart chart transitions. Walkthrough captures wait for the entrance to settle. Very small panes
necessarily compress chart detail. Vector glyphs use generated resources
and theme tint; charts use Day's common canvas implementation and light/dark chrome.
Group by Feeds defaults on when no preference is present; explicit off remains respected.
Floating group headings use a translucent theme-tinted surface, rounded border and a tinted
vector fallback when the publication has no icon.

Refresh Now appears on individual feed dashboards, in the settings card, and refreshes only
that feed. Smart-feed overviews do not offer this single-source action. The button disables while a refresh is active and the
forecast updates from the resulting stored scheduling metadata. Card borders use nested
background layers, not foreground canvases, so native mouse hit testing reaches controls
and interactive charts. DayScript taps dispatch actions directly and do not test occlusion.

Refresh forecasts use the exact adaptive interval function and persisted next-poll / retry
deadlines. Fixed timers expose their next tick; manual mode reports that no check is scheduled.
A due check reads as waiting for the scheduler rather than a negative countdown, and active
refreshes read as checking. Forecasts describe an estimate while the app is running, not an
OS background wake-up guarantee. Regression coverage: dashboard word-count, scope/cache and
migration tests in daynews-db, layout tests in src/dashboard.rs, plus dayscript/dashboards.yaml
and the dashboard captures in the main walkthrough.

Refresh and Show Only Unread Feeds share the leading sidebar toolbar group. The unread
filter uses Google Material Symbols’ Apache-2.0 mark_email_unread vector, separately from
the article-grouping filter. Bulk Mark All as Read remains in the Article menu with its
shortcut, rather than occupying the toolbar. Walkthrough bulk-marking steps use its
shared command id through the menu model.

The app's status has one bar per window. On a phone it is the navigation's bottom bar, Mail's
(UIKit's toolbar on iPhone, a Material `BottomAppBar` on Android),
shared by the feeds page and the article list (the reader has none): Show Only Unread Feeds
at the leading edge, and at the trailing edge Cancel while a sync can be stopped, otherwise
Mark All as Read over the list (a phone has no Article menu) or New Feed over the feeds.
Between them, the status: a sync's step, a refresh's progress, or when the library was last
updated, over the unread count (the list's, or every feed's on the feeds page). Refresh stays
in the navigation bar. The sidebar's selection, which Back clears, tells the two pages apart:
a phone opens on the list of the section the window starts with (Today), as Day's collapsed
navigation does on every toolkit, so a selection always means its list is showing. Scripts
that press New Feed first return to the feeds (`navigate: { route: "" }`), its only page.
The bar's buttons are four round icons drawn on one grid (`feed_filter`, `feed_filter_on`,
`mark_all_read`, `new_feed_round` and `cancel_round` in resource/vectors), so whichever pair
shows matches in size and weight. The filter is a plain button whose icon fills in while it
filters, as NetNewsWire's does, not a toggle with UIKit's selected background. An iPad,
with no bottom bar, keeps its sync panel beneath the lists.
On a desktop window it is a status bar across the bottom of the whole window, shown by
default and toggled with View ▸ Show/Hide Status Bar (⌥⌘/, since ⌘/ is Next Unread) or the
Articles setting, `news.window.status_bar`: idle, the unread and feed counts and when the
library was last updated; while a sync runs, its progress bar, step, timing and Cancel; while
a refresh runs, its progress. With the status in a bar of its own, the lists drop the refresh
row and the sync panel they show on Android and iPad, and a hidden status bar hides the
status. "Updated 5 minutes ago" is the newer of the feeds' newest `last_fetched` and the
account's last sync, and ages with a minute clock. Article rows and feed headers take the
platform list's own side margins and pad nothing themselves; a feed header's icon fills the
rows' unread-dot gutter, so feed names line up with article titles.

### Settings and About

Settings are `day-piece-settings` (Day's `docs/settings.md`). `src/settings.rs` declares every
preference as a `Pref<T>` static over the platform store (`news.*`, `retention.days`) and the
groups General (Day's appearance and language rows), Articles, Reader, Browser and Feeds, plus
an About built from the manifest. `Preferences::install` in `root()` registers the Settings
window and the App menu's Settings…/About items where a menu bar exists, and the
`settings/<group>` routes. Where there is none, `prefs.nav_item` places a Settings sidebar
row whose page is the settings surface: a root list (About header, one row per group)
that pushes one page per group. Settings that drive state outside the UI (feed grouping,
retention pruning, the refresh timer) are watches on their prefs, installed once in the root
scope. Walkthroughs reach a group with `navigate: settings/<group>` on the desktops, and on a
phone or the web open `settings` and tap the group's `prefs-group-<id>` row. Desktop and web
runs close `day.preferences` afterwards: the web uses a modal cover opened from its app menu,
so navigating the underlying sidebar does not dismiss Settings.

The full-content control is a full-width themed section bar with a double-chevron symbol,
a spinner visible only while downloading, and a localized label. Automatic extraction is
gated by a WebView IntersectionObserver: no publication request starts until the bar
enters the viewport. The native handler rechecks article identity and preferences and
permits one automatic attempt per selection. Inline insertion retains the scroll offset
and excludes the inserted slot from scroll anchoring.
Inline loading uses only the existing web bar's progress indicator. It does not insert a
native status row above the WebView, keeping the viewport height and reading position stable.

The inline bar remains above the extracted article and becomes “Hide Full Article”
with upward chevrons when expanded; the trailing spinner replaces the chevrons during loading.
Full content enters with a short slide and fade, and fades before collapsing. Transitions
respect reduced motion, cancel on reversal, and preserve the sanitized DOM for reopening
without another request. Explicit bar activation persists a per-feed automatic-loading
override: load enables it; hide disables it only when the app-wide auto-load default is off.
When the default is on, hiding only collapses this article. Auto preference changes do not reload the
preview document, and the bar exposes aria-expanded and aria-controls.


Links open in the in-app browser (src/site_browser.rs), modeled on NetNewsWire's: an
unrouted WebView in a large sheet (Day's `large_sheet`; Escape closes it on a desktop) with
Done, the page's title and site, and Open in Browser across the top, and Back and Forward
(enabled from the engine's navigation state, polled while the sheet is up) along the bottom. It is not a full browser: no address to type, only the
page a link opened and where its own links lead. Every in-app link goes through
`site_browser::open_link(url, feed)`: the reader's links (its masthead site name included),
Open in Browser on an article, the timeline row's Open in Browser and an overview's site link.
The page loads under its feed's site profile, the one the reader's Load Full Article reads
cookies from, so a sign-in there unlocks the next load. Settings ▸ Browser ▸ Open web links in
defaults to Embedded Browser; choosing System Default or one of the browsers macOS lists
(`settings::BROWSER` holds `system` or the application id) sends links out instead, and
Incognito Mode and engines without a native WebView always do. When Load Full Article fails, or extracts text that
looks like a teaser (under 600 characters, or not 200 more than the feed's own preview), the
reader shows a note with Open Page, which opens the article in the in-app browser whatever that
setting says. Closing the browser bumps a session counter: every reader of the window drops a
failed or short result, and the visible one loads again. `dayscript/paywall-sign-in.yaml`
walks the flow against tests/site-session-server.py, whose session value is new each run.

The dashboard's Preview or Log In opens the same browser on the feed's site. The dashboard
button, feed context menu and Feed menu share the same guarded action;
the context menu targets its row, while the app menu follows the focused window's selected
feed and is disabled on smart feeds. Incognito mode and unsupported WebViews disable preview.
Sidebar menu handlers and row/button browser actions retain their owning window scope;
refreshing feed rows while a sheet is open must not tie their handlers to that transient sheet.
Day's existing `.sheet()` presentation bounds AppKit sheets to 640×620 points (or the available
window space) and uses UIKit's adaptive page sheet. On other backends the modal cover contains
a centered browser panel capped at 640 points over a translucent backdrop. Narrow windows use
their available width. This uses existing sheet and layout support, without a framework API change.
The per-origin WebProfile is shared with the inert reader host. Native HTTP extraction reads
HttpOnly cookies from that profile for each redirect destination and writes Set-Cookie
responses back to it; cookies are never blindly forwarded across origins. Forget Site creates
a temporary administration view and awaits complete profile clearing before enabling preview.
The Incognito Mode preference rebuilds reader hosts with an ephemeral profile, cancels pending
extraction, closes site previews and disables per-feed reader/login/forget controls. Engines
without real private stores render the safe text preview instead of loading persistent views.
Reselecting the active sidebar feed clears the article and shows its dashboard. Empty native
article selection opens the previously selected article's publisher dashboard and selects
that feed in the sidebar, even when the article was opened from a smart feed. Ordinary scope
changes that clear selection do not redirect navigation.

The Feed overview command opens the selected article’s publisher dashboard, including from
iOS article menus. Without a selected article it opens the current scope dashboard. UIKit
keeps Day’s native navigation sidebar; dragging feeds is available only where NavReorder
is supported, and the Collection menus work everywhere.

On merged content-list toolkits (including UIKit), reselecting a feed closes its article
and opens the article list rather than its overview. Feed rows provide a localized Feed
overview context command that selects that publisher and explicitly opens its dashboard.

## Feed-link activation and verification

RSS/Atom protocol associations (`feed`, `feeds`, `rss`, `atom`, `web+feed`) and RSS/Atom file
MIME types activate the same window-owned subscription sheet as File > New Feed / plus. URLs
are data, never route IDs. Verify performs feed or website discovery, asks for ambiguous choices,
and previews current article titles/summaries. Subscribe remains disabled until the current input
has verified successfully, then commits that response with its HTTP validators without fetching
again and selects the new source. Editing the URL invalidates the ability to subscribe to an old
preview. Clipboard prefill accepts web/feed URLs only. Downloaded feed files are read with a
bounded asynchronous FileUrl lease and use their advertised website for fresh discovery.

Subscribe first checks the library for the same feed (`already_subscribed` in
src/subscriptions.rs). An address already subscribed, compared without its scheme, `www.`, host
case or trailing slash, is the same feed: an alert offers Show Feed or Cancel, never a second
copy. A feed whose previewed stories are mostly (at least half their links) already in one
subscribed feed is that feed under another address, the way a site can link its own feed twice
or one feed can arrive through Feedbin and directly: an alert asks before Subscribe Anyway,
offering Show Feed instead. Several feeds of one site, each with stories of its own, pass with no
question, so the bundled demo feeds (one site, seven feeds) seed without one. Cancel keeps the
sheet and its verified preview. `dayscript/subscription-duplicates.yaml` covers the first case.

### Sync progress, cancellation, and authoritative local reset

The engine emits provider-neutral phase/counter observations before requests and after committed
pages. Feedbin derives optional page totals from the `Link` header's `last` relation; it never
follows that URL or sends credentials to it. Accounts and the timeline show a determinate bar
when the extent is known, otherwise a spinner, with article/page counts, a representative feed,
elapsed time, and a throughput-based stage ETA after a measured sample. Estimates exclude unknown
future work and may change. Each observation produces one escaped log line without credentials,
URLs, or article bodies.

Read-state snapshots compare both the confirmed remote state and the local state (including
pending edits) before writing. An unchanged poll must not dirty every article and mapping:
dirty rows are pinned in the persistence cache until commit. The persistence eviction gate
also backs off when pinned rows prevent eviction, so large changed snapshots do not scan
the entire growing cache on every fault. `unchanged_read_snapshot_does_not_dirty_library_rows`
guards the app-side behavior; day-persistence's eviction regression counts cache scans.

Cancel aborts the HTTP future, invalidates stale callbacks/queued writes, and pauses automatic
triggers until Sync Now (or the next app launch). An already committing transaction may finish;
downloaded data and unacknowledged intents remain. A durable `importing` marker forces an
interrupted scan to continue past a previously imported first page on retry. Only successful
finalization clears that marker and advances the checkpoint.

Clear Local Data and Re-synchronize has a destructive confirmation. It deletes the whole local
library (including local-only feeds, reader bodies, stars, folders, tags, sync mappings and all
pending uploads) in one database-worker transaction, retaining login/account identity and app
preferences. `DatabaseWorker::write_resetting_undo` clears obsolete undo/redo only when that
transaction succeeds; failure preserves both data and history. Reset queues no server mutations.
The next cycle downloads subscriptions and remote reading state without a conditional checkpoint.

`dayscript/feedbin-progress.yaml` exercises cancellation, a scheduler interval while paused,
resume, dismissed reset confirmation, and accepted reset through the ordinary HTTP interface.
Its explicit fixture latency is test-only (`DAY_NEWS_SYNC_TEST_LATENCY_MS`, requires fixture mode).

The article list shows synchronization in a compact bottom footer with single-line activity,
feed and timing labels. The indicator and feed line reserve their geometry across phases.
`news.articles.show_sync_status` (Settings → Articles) persists footer visibility; Accounts
continues to show sync progress and cancellation even when the list footer is hidden.

### Feedbin history windows and incremental fetching

Feedbin-connected accounts use Feedbin's cached RSS/Atom article bodies and stable entry IDs;
local-only feeds still poll publishers directly. Full webpage/reader extraction remains separate.
This follows NetNewsWire's FeedbinAPICaller and FeedbinAccountDelegate implementation.

The persisted Keep Articles preference is installed into the sync session before account startup.
Each cycle captures one cutoff (90 days by default, 0 = unlimited), rounded down to UTC midnight
for at most one day of grace. Feedbin filters ordinary history at the server using `since`, which
means its `created_at` arrival date, not the publisher's potentially backdated `published` date.
Unread IDs outside the window still recover missing bodies in batches of 100. Existing local
stars/tags and reading-state intent are preserved; the window does not delete account history.

Local retention runs at launch and when Keep Articles changes. It removes read, unstarred,
untagged articles by publication age; undated articles use their stored arrival date. A recent
`first_seen_at` does not grant imported history another full retention period. Zero means Keep
Forever. Store regression tests cover shortening 90 to 30 days after importing older articles,
live count invalidation, body deletion, and protected articles.

After a successful scan, the opaque provider checkpoint records the first page's HTTP Date.
Subsequent scans use that server-time watermark, with a one-day lookback on the first incremental
fetch of a provider session and then once daily, following NetNewsWire. Lookback pages are scanned
through known entries so publisher corrections can update content without changing local flags,
tags or arrival dates. Missing Date headers fall back to bounded history and known-page stopping.
Changing a query invalidates its ETag. Increasing retention (including Keep Forever) or changing
subscriptions requests a complete scan of the desired window instead of stopping at a known page.
Every page keeps the same cutoff. Cancellation/failure leaves the durable checkpoint unchanged;
resumption cannot skip older pages. Provider-neutral `prepare_history` owns protocol-specific
query/watermark decisions; the shared engine owns cancellation and transaction boundaries.

Tests in `daynews-db/tests/synchronization.rs` exercise real provider/engine/database flows through
`day-part-http` interception: excluded old read bodies never enter the response stream, old unread
recovery, query-specific validators, widening and cancellation, incremental/launch/daily lookback,
new-feed backfills and content corrections with local read/star/tag state preserved.

On iPhone and iPad, the reader's read/unread command is a plain toolbar button, following
NetNewsWire: an outlined circle means read, and a filled center inside a ring means unread.
It has no selected background. The stored article drives both its icon and localized action
label; taps toggle the database value on the persistence queue, avoiding stale UI snapshots.
Desktop and other backends retain their existing toolbar toggle behavior.


## WebDAV account configuration

Native builds use day-sync v5. Each device has its own writer identity; a shared
library ID identifies the remote sync space. Preserve recovery backups when migrating
older stores. Account configuration and credentials remain local to each installation.

Public WebDAV account details remain visible when a secure-store password is
missing. Updating credentials validates the replacement against the server and
does not require the old password to exist. Activation still requires a restart.
The settings host owns the Accounts page's scrolling, including long WebDAV forms;
there is no nested fixed-height viewport.

WebDAV setup uses shared `form`/`section`/`labeled` controls for aligned account
fields. Connect creates a library or joins the ID entered under Advanced; an
already configured account offers Save password and Sync now. Existing server
and library identifiers are read-only. Advanced holds the library identifier,
alternative transport, pause/resume, and disconnect actions. A native information
dialog explains scope and storage limitations. All presentation strings use
generated localization accessors; account status remains visible during recovery.


### WebDAV collection records (application revision 2)

The existing `day-news:feed-state:1` space profile now supports record revision 2.
Subscriptions and reading choices still require revision 1; collection records require
revision 2. An old build holds newer records without projecting or discarding them.
Upgrading releases those records before importing any local layout. No new server space,
identity, password, or reset is needed.

Stable collection IDs survive rename. Existence/name, sidebar position, sort mode,
feed membership, manual order, and global priority use separate registers, so a rename
and a reorder merge independently. Concurrent deletion wins over rename; other
concurrent choices use the engine's deterministic revision ordering. Deleted IDs are
not recycled. Collection deletion releases feeds before deleting the folder, including
Feedbin-owned feeds, to prevent schema cascades from deleting subscriptions or articles.
Feedbin membership and ordering edits do not publish into the WebDAV profile.

The durable layout archive is independent of article and subscription arrivals. A
membership received before its collection or feed is retained and projected once both
exist. Concurrent equal names receive deterministic ID suffixes; two-phase renaming
preserves the database's unique-name constraint, including name swaps. These labels
contain the user's name and identity, with no translated application text in storage.

At first upgrade, existing named collections and local feed organization are captured
once, transactionally. Empty joining libraries publish no default organization that
could overwrite the creator's layout. Remote held records take precedence over this
bootstrap. Subsequent edits are captured by guarded SQL triggers in the journal
transaction; projection cannot echo itself into local edits. The default ungrouped
section's name, placement and sort remain device preferences.

Integration tests in `crates/daynews-db/tests/webdav.rs` cover concurrent moves,
rename/order independence, deletion safety, unique-name conflicts, dependency arrival
order, one-time bootstrap, restart, late join, mixed-version operation, snapshot
compaction and rollback after an injected journal failure.
