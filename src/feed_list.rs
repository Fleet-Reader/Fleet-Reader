//! Shared, persistent sidebar filtering, and the sidebar's feed collections: one section per
//! collection in the user's order, each sorted its own way. The default collection ("Feeds"
//! until renamed) holds the feeds in no collection; it has no folder in the store, and its
//! name, place and sort are preferences. Article scopes and smart feeds stay independent.
use crate::res;
use day::prelude::*;
use daynews_core::{FeedRow, FeedSort, FolderRow};

const UNREAD_ONLY_KEY: &str = "news.feeds.unread_only";

#[derive(Clone, Copy)]
pub struct FeedList {
    pub unread_only: Signal<bool>,
}

impl Ambient for FeedList {
    fn create() -> Self {
        let unread_only = Signal::new(false);
        // Restore before any window reads it, and persist every change for the app's
        // lifetime, including changes made while revealing a subscribed publisher.
        day::prefs::bind(UNREAD_ONLY_KEY, unread_only);
        Self { unread_only }
    }
}

pub fn toggle() {
    let state = FeedList::app();
    let on = !state.unread_only.get_untracked();
    state.unread_only.set(on);
}

pub fn visible(unread: i64, unread_only: bool) -> bool {
    !unread_only || unread > 0
}

/// Feed ▸ Group by Feeds: flips the setting; `settings::install` mirrors it into
/// daynews-core's flag for the timeline.
pub fn toggle_grouping() {
    let pref = &crate::settings::GROUP_BY_FEED;
    pref.set(!pref.get_untracked());
}

#[cfg(all(
    test,
    any(target_os = "macos", target_os = "linux", target_os = "windows")
))]
mod tests {
    use super::*;

    #[test]
    fn unread_filter_survives_process_restart() {
        const CHILD: &str = "DAY_NEWS_TEST_UNREAD_FILTER";
        if let Ok(mode) = std::env::var(CHILD) {
            let state = FeedList::app();
            match mode.as_str() {
                "enable" => {
                    assert!(!state.unread_only.get_untracked());
                    toggle();
                }
                "disable" => {
                    assert!(state.unread_only.get_untracked());
                    toggle();
                }
                "enabled" => assert!(state.unread_only.get_untracked()),
                "disabled" => assert!(!state.unread_only.get_untracked()),
                _ => panic!("unknown synthetic test mode"),
            }
            day::reactive::flush_sync();
            return;
        }

        let preferences = crate::test_prefs::PreferenceTest::new(UNREAD_ONLY_KEY);
        for mode in ["disabled", "enable", "enabled", "disable", "disabled"] {
            let output = preferences
                .child("feed_list::tests::unread_filter_survives_process_restart")
                .env(CHILD, mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

/// The sidebar section id of the default collection.
pub const UNFILED_SECTION: &str = "feeds";

/// The default collection's id wherever collections are listed; no folder's id is 0.
pub const DEFAULT_COLLECTION: u64 = 0;

/// The default collection as a collection row: its name (the localized "Feeds" until
/// renamed), sort and place, read from its preferences.
pub fn default_collection() -> FolderRow {
    use crate::settings::{
        DEFAULT_COLLECTION_NAME, DEFAULT_COLLECTION_POSITION, DEFAULT_COLLECTION_SORT,
    };
    let name = DEFAULT_COLLECTION_NAME.get();
    FolderRow {
        id: DEFAULT_COLLECTION,
        name: if name.trim().is_empty() {
            res::str::nav_feeds_section().format()
        } else {
            name
        },
        sort: FeedSort::from_key(&DEFAULT_COLLECTION_SORT.get()),
        position: DEFAULT_COLLECTION_POSITION.get(),
    }
}

/// Every collection in sidebar order, the default one in its place among them. `folders`
/// arrive in position order; a folder sharing the default's position comes first.
pub fn collections(folders: &[FolderRow], default: &FolderRow) -> Vec<FolderRow> {
    let at = folders
        .iter()
        .take_while(|f| f.position <= default.position)
        .count();
    let mut all = folders.to_vec();
    all.insert(at, default.clone());
    all
}

/// The live collections, the default one included.
pub fn current_collections() -> Vec<FolderRow> {
    let default = default_collection();
    daynews_core::state()
        .folders
        .with(|folders| collections(folders, &default))
}

/// One sidebar section: a collection's feeds in its sort order. The default collection's are
/// the feeds in no collection.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedGroup {
    pub collection: FolderRow,
    pub feeds: Vec<FeedRow>,
}

impl FeedGroup {
    /// The store folder a feed joins in this section: `None` for the default collection.
    pub fn folder_id(&self) -> Option<u64> {
        (self.collection.id != DEFAULT_COLLECTION).then_some(self.collection.id)
    }

    /// The sidebar section id: `collection:<id>`, or [`UNFILED_SECTION`].
    pub fn section_id(&self) -> String {
        match self.folder_id() {
            Some(id) => format!("collection:{id}"),
            None => UNFILED_SECTION.into(),
        }
    }

    /// "News (12)": the collection's name and how many feeds it holds, read or not.
    pub fn title(&self) -> String {
        res::str::nav_collection_section(res::str::NavCollectionSection {
            name: self.collection.name.clone(),
            count: day::format_decimal(self.feeds.len() as f64, 0),
        })
        .format()
    }

    /// The order this section's feeds follow.
    pub fn sort(&self) -> FeedSort {
        self.collection.sort
    }
}

/// Group `feeds` (in priority order, as the store publishes them) by collection, in the
/// collections' order with `default` among them, each sorted its own way.
pub fn groups(feeds: &[FeedRow], folders: &[FolderRow], default: &FolderRow) -> Vec<FeedGroup> {
    collections(folders, default)
        .into_iter()
        .map(|collection| {
            let mut members: Vec<FeedRow> = feeds
                .iter()
                .filter(|feed| {
                    if collection.id == DEFAULT_COLLECTION {
                        // A folder that has gone reads as none.
                        feed.folder_id
                            .is_none_or(|id| !folders.iter().any(|f| f.id == id))
                    } else {
                        feed.folder_id == Some(collection.id)
                    }
                })
                .cloned()
                .collect();
            sort_feeds(&mut members, collection.sort);
            FeedGroup {
                collection,
                feeds: members,
            }
        })
        .collect()
}

/// Sort a collection's feeds. Every sort is stable, so ties keep priority order.
fn sort_feeds(feeds: &mut [FeedRow], sort: FeedSort) {
    match sort {
        FeedSort::Priority => {}
        FeedSort::RecentlyUpdated => feeds.sort_by_key(|f| std::cmp::Reverse(f.newest)),
        FeedSort::Name => feeds.sort_by_cached_key(|f| f.title.to_lowercase()),
        FeedSort::UnreadCount => feeds.sort_by_key(|f| std::cmp::Reverse(f.unread)),
        FeedSort::TotalCount => feeds.sort_by_key(|f| std::cmp::Reverse(f.total)),
        FeedSort::Manual => {
            feeds.sort_by(|a, b| a.collection_position.total_cmp(&b.collection_position))
        }
    }
}

/// The live groups.
pub fn current_groups() -> Vec<FeedGroup> {
    let st = daynews_core::state();
    let default = default_collection();
    st.feeds
        .with(|feeds| st.folders.with(|folders| groups(feeds, folders, &default)))
}

fn group_of(groups: &[FeedGroup], feed: u64) -> Option<(usize, usize)> {
    groups.iter().enumerate().find_map(|(g, group)| {
        group
            .feeds
            .iter()
            .position(|f| f.id == feed)
            .map(|i| (g, i))
    })
}

/// How a feed's sidebar row drags: within its section only where the user sets the order
/// there (a Manual collection, or the default collection sorted by priority, whose drags set
/// priority), and into any other collection always.
pub fn drag(feed: u64) -> NavDrag {
    let groups = day::reactive::untrack(current_groups);
    match group_of(&groups, feed) {
        Some((g, _)) => match groups[g].sort() {
            FeedSort::Manual => NavDrag::Reorder,
            FeedSort::Priority if groups[g].folder_id().is_none() => NavDrag::Reorder,
            _ => NavDrag::Regroup,
        },
        None => NavDrag::Fixed,
    }
}

/// A sidebar drop: the feed joins the section it landed in, and where that section keeps the
/// user's order, it lands before `before`.
pub fn dropped(feed: u64, section: Option<&str>, before: Option<u64>) {
    let groups = day::reactive::untrack(current_groups);
    let Some((from, _)) = group_of(&groups, feed) else {
        return;
    };
    let Some(to) = groups
        .iter()
        .position(|g| Some(g.section_id().as_str()) == section)
    else {
        return;
    };
    let folder = groups[to].folder_id();
    if from != to || groups[to].sort() == FeedSort::Manual {
        daynews_core::set_feed_folder(feed, folder, before);
    } else {
        // The default collection sorted by priority: the drop moves the feed in that order.
        let target = before.or_else(|| {
            groups[to]
                .feeds
                .iter()
                .rev()
                .find(|f| f.id != feed)
                .map(|f| f.id)
        });
        move_in_priority(feed, target, before.is_none());
    }
}

/// Move `feed` in the global priority order next to `target`: before it, or after it.
fn move_in_priority(feed: u64, target: Option<u64>, after: bool) {
    let Some(target) = target else { return };
    let feeds = daynews_core::state().feeds.get_untracked();
    let others: Vec<u64> = feeds
        .iter()
        .map(|f| f.id)
        .filter(|id| *id != feed)
        .collect();
    if let Some(at) = others.iter().position(|id| *id == target) {
        daynews_core::move_feed(feed, at + usize::from(after));
    }
}

/// Whether Move Feed Up (`delta` -1) or Down (+1) can act: the feed's section keeps an order
/// the user sets, and a neighbor lies that way.
pub fn can_move(feed: u64, delta: isize) -> bool {
    neighbor(&current_groups(), feed, delta).is_some()
}

fn neighbor(groups: &[FeedGroup], feed: u64, delta: isize) -> Option<u64> {
    let (g, i) = group_of(groups, feed)?;
    if !matches!(groups[g].sort(), FeedSort::Priority | FeedSort::Manual) {
        return None;
    }
    let j = i.checked_add_signed(delta)?;
    groups[g].feeds.get(j).map(|f| f.id)
}

/// Move Feed Up / Move Feed Down: one step within its section, in the order that section
/// shows (its manual order, or priority).
pub fn move_relative(feed: u64, delta: isize) {
    let groups = current_groups();
    let Some(next) = neighbor(&groups, feed, delta) else {
        return;
    };
    let Some((g, i)) = group_of(&groups, feed) else {
        return;
    };
    let group = &groups[g];
    if group.sort() == FeedSort::Manual {
        // Before the row above, or before the one after the row below.
        let before = if delta < 0 {
            Some(next)
        } else {
            group.feeds.get(i + 2).map(|f| f.id)
        };
        daynews_core::set_feed_folder(feed, group.folder_id(), before);
    } else {
        move_in_priority(feed, Some(next), delta > 0);
    }
}

/// What a feed's Collection menu or picker shows: no collection, one of them, or the entry
/// that asks for a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionChoice {
    None,
    Folder(u64),
    New,
}

/// The Collection picker's options: None, every collection, then New Collection….
pub fn collection_options() -> Vec<(CollectionChoice, String)> {
    let mut options = vec![(CollectionChoice::None, res::str::collection_none().format())];
    daynews_core::state().folders.with(|folders| {
        options.extend(
            folders
                .iter()
                .map(|f| (CollectionChoice::Folder(f.id), f.name.clone())),
        )
    });
    options.push((CollectionChoice::New, res::str::collection_new().format()));
    options
}

fn folder_of(feed: u64) -> Option<u64> {
    daynews_core::state().feeds.with(|feeds| {
        feeds
            .iter()
            .find(|f| f.id == feed)
            .and_then(|f| f.folder_id)
    })
}

/// A feed's collection as a picker binding. Choosing New Collection… shows that entry while
/// the name prompt is open, then the feed's collection again, whatever the answer.
#[derive(Clone, Copy)]
pub struct CollectionBinding {
    feed: u64,
    asking: Signal<bool>,
}

impl CollectionBinding {
    pub fn new(feed: u64) -> Self {
        Self {
            feed,
            asking: Signal::new(false),
        }
    }
    fn current(&self) -> CollectionChoice {
        match folder_of(self.feed) {
            Some(id) => CollectionChoice::Folder(id),
            None => CollectionChoice::None,
        }
    }
}

impl day::reactive::Binding<CollectionChoice> for CollectionBinding {
    fn read(&self) -> CollectionChoice {
        if self.asking.get() {
            CollectionChoice::New
        } else {
            self.current()
        }
    }
    fn peek(&self) -> CollectionChoice {
        day::reactive::untrack(|| self.read())
    }
    fn write(&self, choice: CollectionChoice) {
        match choice {
            CollectionChoice::New => {
                let asking = self.asking;
                asking.set(true);
                begin_new_collection(Some(self.feed), move || asking.set(false));
            }
            CollectionChoice::Folder(id) => choose(self.feed, Some(id)),
            CollectionChoice::None => choose(self.feed, None),
        }
    }
}

/// The feed overview's Collection control: a pop-up of None, every collection and New
/// Collection…, separated into those three groups.
pub fn collection_picker(feed: u64) -> impl Piece {
    let collections = daynews_core::state().folders.with(Vec::len);
    let separators = if collections == 0 {
        vec![1]
    } else {
        vec![1, collections + 1]
    };
    picker(CollectionBinding::new(feed), collection_options)
        .menu()
        .separators_before(separators)
        .id(day::id!(dashboard_collection_picker))
}

fn choose(feed: u64, folder: Option<u64>) {
    if folder_of(feed) != folder {
        daynews_core::set_feed_folder(feed, folder, None);
    }
}

/// Ask for a collection's name and create it, moving `feed` into it when given. `done` runs
/// once the prompt closes, whatever the answer. A name already in use picks that collection.
pub fn begin_new_collection(feed: Option<u64>, done: impl FnOnce() + 'static) {
    day::task(async move {
        let name = prompt(res::str::new_collection_title())
            .placeholder(res::str::new_collection_placeholder())
            .await;
        if let Some(name) = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            if name == default_collection().name {
                // The default collection's name: the feed leaves its collection for it.
                if let Some(feed) = feed {
                    choose(feed, None);
                }
            } else {
                daynews_core::create_folder(&name, feed);
            }
        }
        done();
    });
}

/// A feed row's Collection submenu: None, each collection (the feed's own checked), and New
/// Collection….
pub fn collection_menu(feed: u64, current: Option<u64>) -> MenuEntry {
    let mut entries = vec![
        menu_item(res::str::collection_none())
            .id(day::id!(feed_collection_none))
            .checked(current.is_none())
            .action(move || choose(feed, None)),
        menu_separator(),
    ];
    daynews_core::state().folders.with_untracked(|folders| {
        entries.extend(folders.iter().map(|f| {
            let id = f.id;
            menu_item(f.name.clone())
                .id(day::id!(feed_collection, id))
                .checked(current == Some(id))
                .action(move || choose(feed, Some(id)))
        }));
    });
    if entries.len() > 2 {
        entries.push(menu_separator());
    } else {
        entries.pop();
    }
    entries.push(
        menu_item(res::str::collection_new())
            .id(day::id!(feed_collection_new))
            .action(move || begin_new_collection(Some(feed), || {})),
    );
    sub_menu(res::str::collection_menu(), entries)
}

/// A collection's Sort Feeds choice as a picker binding; the default collection's is a
/// preference.
#[derive(Clone, Copy)]
struct SortBinding(u64);

impl SortBinding {
    fn current(&self) -> FeedSort {
        if self.0 == DEFAULT_COLLECTION {
            return FeedSort::from_key(&crate::settings::DEFAULT_COLLECTION_SORT.get());
        }
        daynews_core::state().folders.with(|folders| {
            folders
                .iter()
                .find(|f| f.id == self.0)
                .map_or(FeedSort::Priority, |f| f.sort)
        })
    }
}

impl day::reactive::Binding<FeedSort> for SortBinding {
    fn read(&self) -> FeedSort {
        self.current()
    }
    fn peek(&self) -> FeedSort {
        day::reactive::untrack(|| self.current())
    }
    fn write(&self, sort: FeedSort) {
        if self.0 == DEFAULT_COLLECTION {
            crate::settings::DEFAULT_COLLECTION_SORT.set(sort.key().to_string());
        } else {
            daynews_core::set_folder_sort(self.0, sort);
        }
    }
}

fn sort_options() -> Vec<(FeedSort, day::LocalizedText)> {
    FeedSort::ALL
        .into_iter()
        .map(|sort| {
            let title = match sort {
                FeedSort::Priority => res::str::sort_priority(),
                FeedSort::RecentlyUpdated => res::str::sort_recently_updated(),
                FeedSort::Name => res::str::sort_name(),
                FeedSort::UnreadCount => res::str::sort_unread_count(),
                FeedSort::TotalCount => res::str::sort_total_count(),
                FeedSort::Manual => res::str::sort_manual(),
            };
            (sort, title)
        })
        .collect()
}

/// Settings ▸ Feeds ▸ Collections: every collection in sidebar order, dragged to reorder. The
/// default collection is marked as such and can be renamed and moved but not deleted. The
/// selected collection shows its Sort Feeds menu; the buttons below add, rename and delete.
pub fn collections_editor() -> impl Piece {
    let st = daynews_core::state();
    let selected: Signal<Option<u64>> = Signal::new(None);
    // A deleted collection takes the selection with it.
    Effect::new(move || {
        if let Some(id) = selected.get()
            && !current_collections().iter().any(|c| c.id == id)
        {
            selected.set(None);
        }
    });
    let collections = list(
        items(current_collections, |c: &FolderRow| c.id),
        move |slot: ItemSlot<FolderRow, u64>| {
            let id = slot.key();
            let default = id == DEFAULT_COLLECTION;
            row((
                vector(if default {
                    res::vectors::dashboard_all
                } else {
                    res::vectors::dashboard_feed
                })
                .tint(move || {
                    if default {
                        crate::theme::palette().accent
                    } else {
                        crate::theme::palette().text_muted
                    }
                })
                .decorative()
                .frame(16.0, 16.0),
                label(move || slot.field(|c| c.name.clone())).single_line(),
                when(move || default, default_badge),
                spacer(),
                label(move || {
                    let n = st.feeds.with(|feeds| {
                        feeds
                            .iter()
                            .filter(|f| {
                                if default {
                                    f.folder_id.is_none_or(|folder| {
                                        !st.folders.with(|all| all.iter().any(|c| c.id == folder))
                                    })
                                } else {
                                    f.folder_id == Some(id)
                                }
                            })
                            .count()
                    });
                    res::str::feeds_count(n as f64).format()
                })
                .font(Font::Caption)
                .color(move || crate::theme::palette().text_muted)
                .single_line(),
                when(
                    move || selected.get() == Some(id),
                    move || {
                        row((
                            label(res::str::sort_feeds())
                                .font(Font::Caption)
                                .single_line(),
                            picker(SortBinding(id), sort_options())
                                .menu()
                                .separators_before([5])
                                .id(day::id!(collection_sort, id)),
                        ))
                        .spacing(Space::XS)
                        .align(VAlign::Center)
                    },
                ),
            ))
            .spacing(Space::S)
            .align(VAlign::Center)
            .padding(Insets::symmetric(8.0, 0.0))
            .id(day::id!(collection_row, id))
            .grow(Dimension::Width)
        },
    )
    .row_height(RowHeight::Uniform(36.0))
    .on_select(move |id: u64| selected.set(Some(id)))
    .selected_rows(move || {
        let id = selected.get();
        current_collections()
            .iter()
            .position(|c| Some(c.id) == id)
            .into_iter()
            .collect()
    })
    .reorderable(Reorder::new(|from, to| {
        let mut order = day::reactive::untrack(current_collections);
        if from < order.len() {
            let moved = order.remove(from);
            order.insert(to.min(order.len()), moved);
            reorder_collections(&order);
        }
    }))
    .id(day::id!(collections_list))
    .height(200.0)
    .grow(Dimension::Width);
    let chosen = move || {
        selected.get().and_then(|id| {
            current_collections()
                .into_iter()
                .find(|c| c.id == id)
                .map(|c| (id, c.name))
        })
    };
    column((
        collections,
        row((
            button(res::str::collection_add())
                .action(|| begin_new_collection(None, || {}))
                .id(day::id!(collection_add)),
            button(res::str::collection_rename())
                .enabled(move || chosen().is_some())
                .action(move || {
                    if let Some((id, name)) = chosen() {
                        rename_collection(id, name);
                    }
                })
                .id(day::id!(collection_rename)),
            // The default collection stays: it is where feeds in no collection live.
            button(res::str::collection_delete())
                .enabled(move || chosen().is_some_and(|(id, _)| id != DEFAULT_COLLECTION))
                .action(move || {
                    if let Some((id, name)) = chosen() {
                        delete_collection(id, name);
                    }
                })
                .id(day::id!(collection_delete)),
        ))
        .spacing(Space::S),
    ))
    .spacing(Space::S)
    .align(HAlign::Leading)
    .grow(Dimension::Width)
}

/// The default collection's mark in Settings: a small accent capsule.
fn default_badge() -> impl Piece {
    label(res::str::collection_default())
        .font(Font::Caption2)
        .weight(FontWeight::Semibold)
        .color(move || crate::theme::palette().accent)
        .single_line()
        .padding(Insets::symmetric(6.0, 1.0))
        .background(move || crate::theme::palette().accent.with_alpha(0.14))
        .corner_radius(6.0)
        .id(day::id!(collection_default_badge))
}

/// Store a new collection order: each folder's position is its index, and the default
/// collection's preference takes its own. Dropped last, the default collection stays after
/// every collection, including ones created later.
fn reorder_collections(order: &[FolderRow]) {
    use crate::settings::{DEFAULT_COLLECTION_LAST, DEFAULT_COLLECTION_POSITION};
    let mut positions = Vec::with_capacity(order.len());
    for (index, collection) in order.iter().enumerate() {
        if collection.id == DEFAULT_COLLECTION {
            DEFAULT_COLLECTION_POSITION.set(if index + 1 == order.len() {
                DEFAULT_COLLECTION_LAST
            } else {
                index as f64
            });
        } else {
            positions.push((collection.id, index as f64));
        }
    }
    daynews_core::set_folder_positions(positions);
}

/// Ask for a feed's new title, starting from the one it shows: Feed ▸ Rename Feed…, its sidebar
/// row's Rename…, and the Rename… button on its overview. The title is the feed's own from then
/// on (later refreshes keep it) and goes to the sync service with the subscription. An empty or
/// unchanged answer changes nothing.
pub fn rename_feed(feed: u64) {
    let Some(current) = daynews_core::state()
        .feeds
        .with_untracked(|feeds| feeds.iter().find(|f| f.id == feed).map(|f| f.title.clone()))
    else {
        return;
    };
    day::task(async move {
        let Some(new) = prompt(res::str::rename_feed_title())
            .placeholder(res::str::feed_name_placeholder())
            .initial(current.clone())
            .await
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty() && *n != current)
        else {
            return;
        };
        daynews_core::rename_feed(feed, &new);
    });
}

/// Ask for a collection's new name. A name another collection has is refused, and said so.
/// The default collection's name is a preference; its localized name clears it.
fn rename_collection(id: u64, name: String) {
    day::task(async move {
        let Some(new) = prompt(res::str::rename_collection_title())
            .placeholder(res::str::new_collection_placeholder())
            .initial(name.clone())
            .await
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty() && *n != name)
        else {
            return;
        };
        let taken = day::reactive::untrack(current_collections)
            .iter()
            .any(|c| c.id != id && c.name == new);
        if taken {
            alert(res::str::collection_name_taken(new.as_str()))
                .button(res::str::collection_ok(), ())
                .present()
                .await;
        } else if id == DEFAULT_COLLECTION {
            let localized = res::str::nav_feeds_section().format();
            crate::settings::DEFAULT_COLLECTION_NAME.set(if new == localized {
                String::new()
            } else {
                new
            });
        } else {
            daynews_core::rename_folder(id, &new);
        }
    });
}

/// Confirm, then delete a collection; its feeds stay subscribed.
fn delete_collection(id: u64, name: String) {
    if id == DEFAULT_COLLECTION {
        return;
    }
    day::task(async move {
        let confirmed = confirm(res::str::delete_collection_title(name.as_str()))
            .message(res::str::delete_collection_message())
            .confirm_label(res::str::collection_delete())
            .destructive()
            .present()
            .await;
        if confirmed {
            daynews_core::delete_folder(id);
        }
    });
}

#[cfg(test)]
mod group_tests {
    use super::*;

    fn feed(id: u64, folder: Option<u64>, title: &str, unread: i64) -> FeedRow {
        FeedRow {
            id,
            title: title.into(),
            unread,
            folder_id: folder,
            has_error: false,
            site_url: None,
            feed_url: String::new(),
            icon_url: None,
            last_fetched: Some(0),
            total: unread * 2,
            newest: Some(id as i64),
            collection_position: (10 - id) as f64,
        }
    }
    fn folder(id: u64, sort: FeedSort) -> FolderRow {
        FolderRow {
            id,
            name: format!("C{id}"),
            sort,
            position: id as f64,
        }
    }
    fn default_at(position: f64, sort: FeedSort) -> FolderRow {
        FolderRow {
            id: DEFAULT_COLLECTION,
            name: "Feeds".into(),
            sort,
            position,
        }
    }
    fn last() -> FolderRow {
        default_at(crate::settings::DEFAULT_COLLECTION_LAST, FeedSort::Priority)
    }
    fn ids(group: &FeedGroup) -> Vec<u64> {
        group.feeds.iter().map(|f| f.id).collect()
    }

    #[test]
    fn collections_come_first_in_their_order_and_sort_their_own_feeds() {
        let feeds = [
            feed(1, Some(20), "beta", 1),
            feed(2, Some(10), "Alpha", 5),
            feed(3, None, "gamma", 0),
            feed(4, Some(20), "alpha", 3),
            feed(5, Some(99), "orphan", 0),
        ];
        let folders = [folder(20, FeedSort::Name), folder(10, FeedSort::Manual)];
        let g = groups(&feeds, &folders, &last());
        assert_eq!(g.len(), 3);
        assert_eq!(ids(&g[0]), [4, 1], "Name sort, case-insensitive");
        assert_eq!(ids(&g[1]), [2]);
        assert_eq!(
            ids(&g[2]),
            [3, 5],
            "unfiled, and a missing folder reads as none"
        );
        assert_eq!(g[2].section_id(), UNFILED_SECTION);

        let folders = [folder(20, FeedSort::UnreadCount)];
        assert_eq!(ids(&groups(&feeds, &folders, &last())[0]), [4, 1]);
        let folders = [folder(20, FeedSort::RecentlyUpdated)];
        assert_eq!(ids(&groups(&feeds, &folders, &last())[0]), [4, 1]);
        let folders = [folder(20, FeedSort::Manual)];
        assert_eq!(ids(&groups(&feeds, &folders, &last())[0]), [4, 1]);
        let folders = [folder(20, FeedSort::Priority)];
        assert_eq!(ids(&groups(&feeds, &folders, &last())[0]), [1, 4]);
    }

    #[test]
    fn the_default_collection_sits_where_its_position_puts_it_and_sorts_its_own_way() {
        let feeds = [
            feed(1, None, "b", 1),
            feed(2, Some(10), "x", 0),
            feed(3, None, "a", 9),
        ];
        let folders = [
            folder(10, FeedSort::Priority),
            folder(30, FeedSort::Priority),
        ];
        let g = groups(&feeds, &folders, &default_at(20.0, FeedSort::Name));
        let order: Vec<u64> = g.iter().map(|g| g.collection.id).collect();
        assert_eq!(order, [10, DEFAULT_COLLECTION, 30]);
        assert_eq!(ids(&g[1]), [3, 1], "the default collection sorts by name");
        assert_eq!(g[1].section_id(), UNFILED_SECTION);
        assert_eq!(g[1].folder_id(), None);
        let first = groups(&feeds, &folders, &default_at(0.0, FeedSort::Priority));
        assert_eq!(first[0].collection.id, DEFAULT_COLLECTION);
        assert_eq!(ids(&first[0]), [1, 3]);
    }

    #[test]
    fn move_up_and_down_stay_inside_orders_the_user_sets() {
        let feeds = [
            feed(1, Some(20), "b", 1),
            feed(2, Some(20), "a", 2),
            feed(3, None, "c", 0),
        ];
        let sorted = groups(&feeds, &[folder(20, FeedSort::Name)], &last());
        assert_eq!(
            neighbor(&sorted, 2, 1),
            None,
            "a sorted collection has no manual steps"
        );
        let priority = groups(&feeds, &[folder(20, FeedSort::Priority)], &last());
        assert_eq!(neighbor(&priority, 1, 1), Some(2));
        assert_eq!(
            neighbor(&priority, 2, 1),
            None,
            "never past the section's end"
        );
        assert_eq!(neighbor(&priority, 3, -1), None);
    }
}
