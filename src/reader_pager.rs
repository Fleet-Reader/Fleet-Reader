// SPDX-License-Identifier: Apache-2.0
//! Mobile reader paging. Three stable page slots retain their webviews while the
//! reading order rotates. Preparing a page never calls open_article/set_read.
use crate::reader_view::{ReaderView, ReaderVisibility};
use day::pager::{PagerState, pager};
use day::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageLoad {
    Loading,
    Unavailable,
    Failed,
}

#[derive(Clone, Copy)]
pub(crate) struct PageSlot(pub usize, pub Signal<PageLoad>);
impl PageSlot {
    pub fn web_id(self) -> day::ElementId {
        day::id!(reader_preview, self.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Window {
    slots: [Option<u64>; 3],
    order: Vec<usize>,
    current: usize,
}
impl Window {
    fn reconcile(&self, rows: &[u64], selected: Option<u64>) -> Self {
        let Some(selected) = selected else {
            return Self::default();
        };
        let mut desired = Vec::with_capacity(3);
        let position = rows.iter().position(|id| *id == selected);
        if let Some(previous) = position
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| rows.get(i))
        {
            desired.push(*previous);
        }
        desired.push(selected);
        if let Some(next) = position.and_then(|i| rows.get(i + 1)) {
            desired.push(*next);
        }
        let mut slots = self.slots.map(|id| id.filter(|id| desired.contains(id)));
        for id in &desired {
            if !slots.contains(&Some(*id)) {
                let vacant = slots.iter().position(Option::is_none).unwrap();
                slots[vacant] = Some(*id);
            }
        }
        let order = desired
            .iter()
            .map(|id| slots.iter().position(|s| *s == Some(*id)).unwrap())
            .collect();
        let current = slots.iter().position(|id| *id == Some(selected)).unwrap();
        Self {
            slots,
            order,
            current,
        }
    }
}

pub(crate) fn reader_pager() -> impl Piece {
    let scene = daynews_core::scene();
    let window_view = ReaderView::ambient();
    let busy = Signal::new(false);
    let window = Signal::new(Window::default());
    let presentation = Signal::new(PagerState::default());
    let ids: [Signal<Option<u64>>; 3] = std::array::from_fn(|_| Signal::new(None));
    let bodies =
        std::array::from_fn::<_, 3, _>(|_| Signal::new(None::<daynews_core::StoredArticle>));
    let views: Signal<[Option<ReaderView>; 3]> = Signal::new([None; 3]);

    Effect::new(move || {
        let selected = scene.selected.get();
        let rows = scene
            .articles
            .with(|rows| rows.iter().map(|a| a.id).collect::<Vec<_>>());
        if busy.get() {
            return;
        }
        let before = window.get_untracked();
        let next = before.reconcile(&rows, selected);
        if next == before {
            return;
        }
        day::reactive::batch(|| {
            for (slot, id) in ids.iter().enumerate() {
                id.set_if_changed(next.slots[slot]);
            }
            let epoch = presentation.get_untracked().epoch.wrapping_add(1);
            presentation.set(PagerState {
                order: next.order.clone(),
                selected: next.current,
                epoch,
            });
            window.set(next);
        });
    });
    // Selected article updates (read/star, sync) update the snapshot, not the page identity.
    Effect::new(move || {
        if let Some(article) = scene.article.get() {
            for slot in 0..3 {
                if ids[slot].get() == Some(article.id) {
                    bodies[slot].set_if_changed(Some(article.clone()));
                }
            }
        }
    });
    Effect::new(move || {
        let selected = scene.selected.get();
        let current = ids
            .iter()
            .position(|id| id.get() == selected && selected.is_some());
        let view = current.and_then(|slot| views.get()[slot]);
        window_view.shown.set(view);
    });
    day::reactive::Scope::current().on_cleanup(move || {
        if window_view.shown.try_get().is_some() {
            window_view.shown.set(None);
        }
    });

    let pages = (0..3).map(|slot| {
        let id = ids[slot];
        let article = bodies[slot];
        let visible = Signal::new(false);
        let load = Signal::new(PageLoad::Loading);
        let mut page_scene = scene;
        page_scene.selected = id;
        page_scene.article = article;
        page_scene.reader_open = visible;
        day_core::with_environment(page_scene, move || {
            day_core::with_environment(ReaderVisibility(visible), move || {
                day_core::with_environment(PageSlot(slot, load), move || ReaderView::scoped(move |view| {
                    views.update(|views| views[slot] = Some(view));
                    Effect::new(move || {
                        visible.set(scene.reader_open.get() && scene.selected.get().is_some() && scene.selected.get() == id.get());
                    });
                    Effect::new(move || {
                        let expected = id.get();
                        load.set(PageLoad::Loading);
                        if article.get_untracked().as_ref().map(|a| a.id) != expected { article.set(None); }
                        let Some(expected) = expected else { return; };
                        if article.get_untracked().as_ref().is_some_and(|a| a.id == expected) { return; }
                        let task = day::task(async move {
                            match daynews_core::preview_article(expected).await {
                                Ok(snapshot) if id.try_get() == Some(Some(expected)) => {
                                    if snapshot.is_none() { load.set(PageLoad::Unavailable); }
                                    article.set(snapshot);
                                }
                                Ok(_) => {}
                                Err(error) => {
                                    eprintln!("[reader-pager] preview article={expected} failed: {error}");
                                    if id.try_get() == Some(Some(expected)) { load.set(PageLoad::Failed); }
                                }
                            }
                        });
                        day::reactive::on_run_retrack(move || task.abort());
                    });
                    crate::reader::reader_document(false)
                }))
            })
        }).any()
    }).collect();
    pager(presentation, busy, pages, move |slot| {
        let frozen = window.get_untracked();
        if let Some(id) = frozen.slots[slot]
            && scene.reader_open.get_untracked()
            && scene.selected.get_untracked() == frozen.slots[frozen.current]
            && scene.selected.get_untracked() != Some(id)
        {
            // Run in the real scene: preview scenes never mutate reading state.
            daynews_core::open_article(id);
        }
    })
    .id(day::id!(reader_pager))
    .background(move || crate::theme::palette().bg)
    .grow(Dimension::Both)
    .toolbar(move || {
        scene
            .article
            .get()
            .map(crate::toolbar::article_items)
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn advancing_retains_both_visible_pages_and_recycles_only_the_far_neighbor() {
        let before = Window::default().reconcile(&[1, 2, 3, 4], Some(2));
        let after = before.reconcile(&[1, 2, 3, 4], Some(3));
        for id in [2, 3] {
            assert_eq!(
                before.slots.iter().position(|s| *s == Some(id)),
                after.slots.iter().position(|s| *s == Some(id))
            );
        }
        assert!(!after.slots.contains(&Some(1)));
        assert!(after.slots.contains(&Some(4)));
    }
    #[test]
    fn boundaries_do_not_wrap_and_missing_selection_does_not_jump() {
        assert_eq!(
            Window::default().reconcile(&[1, 2, 3], Some(1)).order.len(),
            2
        );
        assert_eq!(
            Window::default().reconcile(&[1, 2, 3], Some(3)).order.len(),
            2
        );
        let missing = Window::default().reconcile(&[1, 2, 3], Some(9));
        assert_eq!(missing.order.len(), 1);
        assert_eq!(missing.slots[missing.current], Some(9));
        assert_eq!(missing.reconcile(&[], None), Window::default());
    }
}
