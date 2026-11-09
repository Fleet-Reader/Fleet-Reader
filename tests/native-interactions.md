# Native interaction regression tests

For the Android cold-launch Up-button regression, run
`python3 tests/android-up.py --adb /path/to/adb --serial emulator-5554` against an English
install. It cold-launches Today three times, uses native toolbar taps from every smart feed,
checks Settings dismissal, and verifies that narrowing a tiled window restores Up. It does
not mark articles or change subscriptions. It rejects stale accessibility snapshots during
launch and verifies the native detail pane no longer covers the root after Up.

UIKit's `NavigationTests.testRootReachableFromSmartFeeds` launches an isolated empty library
and taps Today, All Unread, Starred, and All Articles, asserting a hittable native Back button
and a return to the root each time. Run it with `xcodebuild test` on the NavigationProbe
scheme, selecting `NavigationProbe/NavigationTests/testRootReachableFromSmartFeeds`.

`NavigationTests.testNewFeedRemainsUsableWhenPasteIsDenied` opens New Feed with native taps
in an isolated empty library, denies the system paste prompt when present, types a synthetic
URL, and cancels the form. Put synthetic text on the simulator clipboard from outside the app
to exercise the permission prompt. New Feed waits at most 200 ms for its optional clipboard
suggestion; a pending permission request must not block the UI thread or overwrite later input.

Use throwaway stores; these scripts mark articles read/unread and change synthetic feeds' reader
preferences. Never run them against a personal subscription database.

Start `python3 tests/reader-view-server.py 28772`. For Android, additionally run
`adb reverse tcp:28772 tcp:28772`. The fixture serves synthetic RSS previews and publication
HTML, including a long preview and a delayed full article. CORS is enabled for browser tests.

For a desktop target:

```sh
day launch -t macos-appkit --env DAY_DATA_DIR=/tmp/day-news-regression \
  --script dayscript/seed-demo.yaml \
  --script dayscript/reading-navigation.yaml \
  --script dayscript/article-deselection.yaml \
  --script dayscript/reader-filter-selection.yaml \
  --script dayscript/reader-regressions.yaml \
  --script dayscript/native-reader-interactions.yaml
```

Use `macos-gtk` or `macos-qt` for the other desktop toolkits. Use `ios-uikit` with
`--ios-simulator <UDID>` and add `dayscript/feed-navigation.yaml` to check single-tap
navigation and feed overview. The iPad's split layout can legitimately keep the dashboard
visible alongside the article list. Clipboard permission must be granted for `seed-demo`,
or seed the isolated simulator store from a consistent backup of the synthetic desktop store.
On Android use an app-private store path such as
`/data/user/0/org.appfair.app.FleetReader/files/regression`. Browser tests use `web-dom` with the configured
Day web driver and a fresh browser profile; their database lives in browser storage.

The long-reader script checks visibility-triggered loading, no top loading banner, preserved
viewport/scroll, retained content after read/star changes, spinner visibility, collapse/expand,
and reversed animation. Screenshots capture appended and re-expanded content. Browser loading
may finish before a later script command can observe its transient busy state, so that
transient-state check is restricted to the five native targets.

Run `dayscript/walkthrough.yaml` after seeding for the full application flow and dashboard
screenshots. `dayscript/grouped-feeds.yaml` additionally checks desktop feed ordering and the
floating header after scrolling; the initial article must remain unobscured.

Dayscript injects framework events, not physical pointer or keyboard input. Complement it
with the real GTK selection-model test and AppKit NSTableView keyboard test in day/. For a long
reader page in an isolated macOS test app, set `window.scrollTo(0,0)` through `day drive`, run
`swift tests/macos-reader-scroll.swift <app-pid>`, and assert `scrollY > 100` through `web_eval`.
The helper requires macOS Accessibility permission and sends a real wheel event to the app's
visible content pane. This check passed on AppKit, GTK, and Qt. It does change desktop focus.

For mobile article paging, run `dayscript/reader-swipes.yaml` after seeding. It uses
programmatic pager selection to check commit and document retention on all three mobile
backends; it does not synthesize a native drag. For real Android gestures, run:

```sh
python3 tests/android-reader-paging.py --adb /path/to/adb --day /path/to/day --serial emulator-5554
```

Select that same emulator when launching the isolated app. The test checks a cancelled drag,
repeated forward/back swipes, retained JavaScript state and scroll offset, all slot rotations,
and both boundaries. On UIKit, seed `/tmp/day-news-native-pager` with `seed-demo.yaml`, then
run the XCTest runner's `NavigationTests/testArticlePaging` (setup below). The test explicitly
relaunches that fixture store, taps Field Notes and its first article, and asserts WebView text
so offscreen timeline labels cannot pass. It uses actual screen-coordinate drags, including
a slow cancelled gesture and forward/backward traversal of all six articles. Capture a screenshot during a long drag to inspect
both live pages. Check vertical scrolling, links, text selection, pinch zoom and system Back
on device as part of acceptance. Harmony builds locally; its native gesture acceptance runs
in CI, not the local Intel emulator.

Interactive pager validation (October 18, 2026): 145 app tests and two framework pager tests
passed. Native XCTest traversed all six fixture articles forward and back, tested cancellation
and both boundaries, and asserted the visible WebView. Android real-input tests additionally
verified repeated reversal and retained document/scroll state. Full walkthroughs passed on
UIKit (158 executed steps), Android (163), and AppKit (242); screenshots were inspected.
The updated long-article scroll/navigation regression passed 48 Android steps.
Harmony and web builds passed. Host, UIKit, Android and Harmony lint checks passed using the
repository's documented cross-target flags. Harmony runtime, physical devices, and the
remaining desktop backends were not exercised for this change.

For article scroll reset, start `python3 tests/reader-view-server.py 28763`, then launch with
a fresh isolated store and `--script dayscript/reader-scroll-navigation.yaml`. It opens long
synthetic articles and checks Next Unread (the Cmd-/ command), Next, Previous, and direct
selection start at the top on desktop, while starring preserves the document and scroll
position. Mobile paging intentionally preserves the position of retained adjacent pages;
`reader-swipes.yaml` and the native pager tests cover that different contract.

Android's article scrolling was checked with an actual
`adb shell input swipe`. Such tests do not establish IME, VoiceOver/TalkBack, physical-device,
or every theme/locale behavior. HarmonyOS and Windows runtime tests require CI or their hosts.

For real Android navigation taps, start the English fixture app on its root list and run
`python3 tests/android-navigation.py --adb /path/to/adb --serial emulator-5554`. The script
checks single taps on all four smart feeds, Settings and Reader View Fixtures, then a native
long press on the fixture feed. It asserts the toolbar destination, because Android's
accessibility hierarchy also exposes sidebar labels that are hidden behind the current pane.
This caught bugs that injected dayscript selection events did not: the row tap recognizer
only reselected the current route, and empty context menus erased native click listeners.
Android now uses Material's native navigation widget; sidebar drag-reordering is disabled
there so navigation and long-press feed menus take precedence. Ordinary lists still reorder.

## Local run, October 6, 2026

| Target | Expanded interaction suite | Full walkthrough / combined suite |
| --- | --- | --- |
| macos-appkit | 171 passed | 222 passed |
| macos-gtk | 169 passed | 219 passed |
| macos-qt | Covered in combined suite | 279 passed, including grouping and reader cases |
| ios-uikit (iPad simulator) | 121 passed | 220 passed, including remaining reader cases |
| android-material (emulator) | 174 passed | 173 passed, including reader cases |
| web-dom (Chromium) | Covered in combined suite | 384 passed |

Counts describe separate runs and are not additive. Platform-inapplicable assertions are
skipped explicitly. All runs use synthetic subscriptions. Final targeted runs repeat affected
reader cases after the framework cleanup fixes.

Fresh UIKit Rust compilation succeeds, but Xcode simulator packaging stalls in its input-option
resolution; iPad runtime coverage therefore uses the existing simulator executable and does
not validate the final framework changes on iPad. HarmonyOS Rust compilation succeeds; local
HAP packaging is blocked by the host's missing `@ohos/hvigor-ohos-plugin`. No HarmonyOS emulator
tests were attempted. Android's full walkthrough passed on retry after reducing concurrent
browser load and using the already-seeded fixture store.

### Android back-stack audit (October 2026)

See [android-navigation-audit.md](android-navigation-audit.md) for ownership,
root cause, phone/tablet rules, and remaining architecture limits. On the isolated
English fixture app, return to the compact feeds root and run:

```sh
python3 tests/android-navigation.py --adb /path/to/adb --back-cycles 4
python3 tests/android-navigation.py --adb /path/to/adb --tablet
python3 tests/android-navigation.py --adb /path/to/adb --edge-swipes
```

The first alternates article toolbar Up and system Back, then leaves the section
and opens Settings. It checks the native pane is actually offscreen at the feeds
root, so the blank interstitial cannot pass merely because its title says Fleet Reader.
The second temporarily widens the emulator, checks section changes create no history,
closes/reopens a reader, and narrows back to verify reader → list → feeds. It restores
the prior size override and logging property in `finally`; use an isolated emulator
with a compact initial size and density low enough for 2400×1600 to tile.
The third tests committed native edge Back swipes from a section and from a reader;
it requires Android gesture navigation to be enabled.

Local validation: six native destination taps and feed long press; four mixed article
Back/Up cycles; wide section replacement, wide reader Back and wide-to-compact
reader navigation, and committed native edge Back swipes. Phone and tablet screenshots inspected. Predictive gesture
cancellation, physical foldables and process-death restoration require additional
acceptance coverage; system Back key coverage does not establish those behaviors.

### Smart-feed activation and native pull-to-refresh (October 2026)

`tests/ios-navigation` contains an XCTest runner using actual UIKit taps and pulls.
`testEmptySmartFeeds` expects an isolated, empty English library on the feeds root;
it checks all four smart feeds and repeated Today → Back → Today. It asserts that the
empty label is **hittable**, not merely present in UIKit's retained offscreen columns.
Generate the runner with `xcodegen generate --spec tests/ios-navigation/project.yml`,
then build and run it using `xcodebuild build-for-testing` / `test-without-building`,
an explicit simulator destination and a derived-data directory outside the repository.
Use `-only-testing:NavigationProbe/NavigationTests/testEmptySmartFeeds` for this case.

For `testPullToRefresh`, run `python3 tests/feed-refresh-server.py`, then launch with
an isolated `DAY_DATA_DIR` and `--script dayscript/pull-refresh-seed.yaml`.
On Android also use `adb -s SERIAL reverse tcp:28762 tcp:28762`, select that same device
with `--android-device SERIAL`, and run `tests/android-pull-refresh.py --adb PATH --serial SERIAL`.
On iOS select `-only-testing:NavigationProbe/NavigationTests/testPullToRefresh`.
Run the two gesture suites sequentially because both assert the fixture server's HTTP counts.
The iOS case verifies refresh, completion, a subsequent pull and article navigation;
XCTest waits for native animation quiescence, so the Android case checks a second pull
*during* loading does not duplicate work. Screenshots capture the native active/completed
indicators. Day's `crates/day-pieces/tests/nav_refresh.rs` additionally checks declined
requests, external busy-state writes, disposal and same-route activation without rebuilding.

The slow-feed seed also selects Starred immediately after native Back while refresh is
active, verifies the synchronizing message, then checks it returns to the ordinary empty
state. This reproduced UIKit's exception from pushing the same supplementary controller
twice during a transition. Day now coalesces column requests, waits for transitions, and
checks the native stack before showing the column. The fixed simulator also completed
32 consecutive section/Back cycles without a failure.


For scrolling feed chrome and adaptive Settings, seed the synthetic demo/Feedbin library at
`/tmp/day-news-theme-sync-validation`, then run `python3 tests/seed-mobile-chrome.py`.
The generated `/tmp/day-news-mobile-chrome` library has enough offline fixture feed rows to
scroll on an iPad. Run
`NavigationProbe/NavigationTests/testScrollingFeedChromeAndAdaptiveSettings` on both phone
and tablet simulators. It checks real swipes, search/large-title collapse and restoration,
Settings width, and Done dismissal. `dayscript/tablet-sync-layout.yaml` additionally verifies
that only the article column has a sync footer on a wide mobile display; use the synthetic
Feedbin account with `DAY_NEWS_SYNC_TEST_LATENCY_MS=1500`.

On Android, run `python3 tests/android-mobile-chrome.py --adb /path/to/adb --serial emulator-5554`
after seeding an isolated English library and returning to the root. It uses native swipes to
check scroll-away/quick-return, verifies Today retains Up navigation, then temporarily widens
the emulator to check sidebar search collapse/return with pinned toolbar actions, verifies
article-pane scrolling leaves sidebar search alone, inspects Settings, and restores the
original display size in `finally`.

Run `python3 tests/android-collapsed-navigation.py --adb /path/to/adb --serial emulator-5554`
on a compact English install with enough feeds to scroll. It navigates without opening articles
or changing subscriptions. The first case opens a feed directly from a collapsed root and
fails on the old build with `Destination has no visible Up`. Subsequent cases cover expanded,
partially collapsed, and repeated collapsed entry, scrolling the destination in both directions,
and tapping the actual visible Up control. Assertions check its touch-target bounds and the
native detail pane's position after returning, rather than trusting retained sidebar labels.
The root search must not leak into compact destinations. These cases complement, rather than
replace, the cold-launch, smart-feed, reader, system Back and tablet checks above.


`NavigationProbe/NavigationTests/testTabletToolbarColumns` uses the same isolated iPad
fixture. It verifies that Refresh and Show Only Unread Feeds belong to the middle bar,
that its duplicate Fleet Reader title is absent, that visible buttons have usable widths,
and that filtering does not change the M/N subscription count. UIKit's older plain-bar
fallback also has host unit tests in `day-toolkit-uikit/src/toolbar_policy.rs`. Validate iOS 15
on hardware: the reader should expose its primary actions plus a visible ellipsis menu,
with remaining commands available in that menu rather than squeezed into the bar.

Android favicon regression: use a library with cached PNGs under `feed-icons-v1`, and
inspect both the root feed rows and grouped timeline publisher headings. Bundled RSS
fallback glyphs must remain present for feeds without cached artwork; downloaded logos
must show their original colors. An image view's mere visibility is insufficient: the
old resolver created a correctly sized but blank view for an absolute file path. Verified
on day-ci-medium-phone with the same pre-existing cache before/after replacing the APK;
no cache deletion or re-download was required.
