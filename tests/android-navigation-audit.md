# Android navigation audit

The October 2026 audit found genuine framework state defects, rather than a Fleet Reader
placeholder destination intentionally inserted into navigation.

## Ownership and layouts

Fleet Reader owns the selected feed/smart-feed section and article/reader visibility.
Day's selector creates the selected section's page; its gated detail creates the
article reader. On a compact phone these compose into one Android host: feeds →
article list → reader. On a wide window the sidebar and section are visible together,
and the reader is composed alongside the article list. Closing the reader should
retain the selected section. Changing a section is current selection, not a visit
that the user must unwind through previous feeds.

The Android host uses Material NavigationView for section selection,
SlidingPaneLayout for the platform's measured compact/wide decision, and retained
Day-owned views inside AndroidX fragments. FragmentManager manages nested page
transitions; the Rust model owns which pages exist. This is a reasonable foundation,
but both sides must acknowledge each other's changes exactly once.

## Defects found

1. Native article Back reported `already_popped`, and its Rust owner removed the
   article page without issuing another Popped patch. Android's global `nativePops`
   counter only decremented on Popped, so it leaked. Recorded traces reached two
   leftover acknowledgements. Later model-driven pops consumed that counter and
   skipped native history removal. The view was removed but its history entry
   survived, producing an empty historical page.
2. Adaptive section selection itself was recorded in fragment history. This made
   tablet section changes participate in Back despite being sidebar selections.
   Presentation changes mixed with leaked pop acknowledgements accumulated stale
   section entries. Wide selection's automatic initial-section rule could also
   reselect a section immediately after Back cleared it.
3. SlidingPaneLayout visibility could diverge from model depth. Free pane dragging
   could reveal the sidebar without clearing the selected destination; layout and
   interrupted pane-opening animations also needed explicit root closure.
4. The early fragment committed callback reconciled before settled fragment state.
   Reacting to it synchronously risked premature bookkeeping and reentrant work.
5. Clearing sidebar selection unchecked only top-level menu items. Destinations in
   submenus retained the old selected appearance even after returning to feeds.

## Implemented rules

- Adaptive base sections replace detail content without adding fragment history.
- Compact section Back/Up clears selection through Day and closes the pane. Wide
  base sections do not intercept system Back; the activity's normal Back behavior
  applies when there is no reader, guard, or nested page.
- Nested pages keep native fragment history and Material shared-axis transitions.
- Native-pop acknowledgements identify their page and expire on either its Popped
  acknowledgement or its removal. Deferred callbacks ignore already-released pages.
- Pane and search visibility follow logical page depth, independently of native
  history count. Pane dragging is locked; programmatic open/close follows selection.
- Early fragment notifications defer reconciliation until the transaction settles.
- Clearing selection includes grouped submenu destinations, removing stale highlights.

This follows Android's [two-pane guidance](https://developer.android.com/develop/ui/views/layout/twopane):
replace selected detail without adding selection history and explicitly integrate
pane state with Back. Compact section Back uses a dispatcher callback and has no
seekable fragment predictive preview; genuine nested fragment pages retain that
support. An armed application guard also has no fragment predictive preview.

## Scope and remaining architectural limits

The activity's FragmentManager is still shared by hosts, with entry-name prefixes
separating their history. This is more complex than child fragment managers, and
arbitrary independently stacked nested hosts need additional acceptance coverage.
PageFragment retains a live Rust-owned view; Android recreation rebuilds the Day
model and removes restored fragment shells rather than restoring those views.
Configuration/presentation changes and process-death restoration therefore have
different paths and should not be assumed equivalent. Root host/chrome tracking is
still based on one active host per activity; this audit is not a general multi-host
or multi-window redesign.

For diagnosis enable `adb shell setprop log.tag.DayNavigation DEBUG`; logcat then
records titles, native entry counts, pending model pops, page acknowledgements and
pane state. Disable with `INFO` after testing. Real-touch regression commands and
coverage are documented in [native-interactions.md](native-interactions.md).

## Local evidence

The original Android trace showed two stale native-pop acknowledgements after
article Back and later three section history entries during presentation changes.
With the fix, six real section taps and feed context long press passed. Four mixed
article Back/toolbar-Up → section-Up → Settings-Back cycles passed; the final-build
repeat also passed. Wide All Articles/Starred/Settings changes retained zero native
history entries; wide reader Back retained its section; narrowing an open reader
returned through reader → list → feeds. Root assertions inspect the actual native
pane position, not just a title or hidden accessibility child. Committed native edge
swipes passed for section → feeds and reader → list → feeds. Phone and tablet
screenshots were inspected. Framework mock tests passed (228), Android backend
Clippy passed, and the framework readiness gate passed. These checks do not cover
cancelled predictive gestures, real foldable hinges, or process-death restoration.
