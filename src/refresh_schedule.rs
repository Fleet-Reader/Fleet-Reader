//! One app-wide refresh timer, driven by the Refresh Feeds setting and independent of any
//! window's lifetime.
use day::prelude::Signal;
use day_piece_settings::prelude::*;
use futures_util::future::{AbortHandle, Abortable};
use std::cell::{OnceCell, RefCell};

pref_enum! {
    /// How the feeds refresh on their own: Day News's per-feed cadence, never, or on a fixed
    /// interval. Stored under short tags.
    pub enum Refresh {
        Automatic = "auto",
        Manual = "manual",
        Minutes30 = "30",
        Hour = "60",
        Hours2 = "120",
        Hours4 = "240",
        Hours8 = "480",
    }
}

/// The Refresh Feeds setting.
pub static REFRESH: Pref<Refresh> = Pref::new("news.refresh.minutes", || Refresh::Automatic);

/// The `minutes()` of [`Refresh::Automatic`]: the per-feed scheduler, not one interval.
const AUTOMATIC: u64 = u64::MAX;

impl Refresh {
    /// Minutes between whole-library refreshes: 0 for Manual, [`AUTOMATIC`] for the per-feed
    /// cadence.
    pub fn minutes(self) -> u64 {
        match self {
            Refresh::Automatic => AUTOMATIC,
            Refresh::Manual => 0,
            Refresh::Minutes30 => 30,
            Refresh::Hour => 60,
            Refresh::Hours2 => 120,
            Refresh::Hours4 => 240,
            Refresh::Hours8 => 480,
        }
    }
}

thread_local! {
    static NEXT: OnceCell<Signal<Option<i64>>> = const { OnceCell::new() };
    static TIMER: RefCell<Option<AbortHandle>> = const { RefCell::new(None) };
}

/// Start the timer for the stored setting and restart it on every change. Call once, in the
/// root scope.
pub fn install() {
    start();
    REFRESH.watch(|_| start());
}

pub fn stop() {
    next().set(None);
    TIMER.with(|timer| {
        if let Some(handle) = timer.borrow_mut().take() {
            handle.abort();
        }
    });
}

pub fn start() {
    stop();
    let minutes = REFRESH.get_untracked().minutes();
    if minutes == 0 {
        return;
    }
    let (handle, registration) = AbortHandle::new_pair();
    TIMER.with(|timer| *timer.borrow_mut() = Some(handle));
    day::task(async move {
        let _ = Abortable::new(
            async move {
                if minutes == AUTOMATIC {
                    daynews_core::refresh_due();
                }
                loop {
                    let delay = if minutes == AUTOMATIC {
                        60_000
                    } else {
                        (minutes * 60 * 1000) as u32
                    };
                    next().set(Some(
                        daynews_time::now_unix().saturating_add(i64::from(delay / 1000)),
                    ));
                    day::sleep(delay).await;
                    if minutes == AUTOMATIC {
                        daynews_core::refresh_due();
                    } else {
                        scheduled();
                    }
                }
            },
            registration,
        )
        .await;
    });
}

fn next() -> Signal<Option<i64>> {
    NEXT.with(|slot| *slot.get_or_init(|| Signal::new_in(day::reactive::Scope::root(), None)))
}
/// Actual scheduling mode (in minutes, see [`Refresh::minutes`]) and fixed timer deadline, for
/// the dashboard's estimate. Tracked.
pub fn schedule() -> (u64, Option<i64>) {
    (REFRESH.get().minutes(), next().get())
}
pub fn automatic(value: u64) -> bool {
    value == AUTOMATIC
}

fn scheduled() {
    #[cfg(not(target_arch = "wasm32"))]
    daynews_core::refresh_scheduled();
    #[cfg(target_arch = "wasm32")]
    daynews_core::refresh_all();
}

/// Resume lightweight state sync on activation; feed refresh follows the user's cadence.
#[cfg(not(target_arch = "wasm32"))]
pub fn activated() {
    daynews_core::sync_auto_now();
    match REFRESH.get_untracked() {
        Refresh::Manual => {}
        Refresh::Automatic => daynews_core::refresh_due(),
        _ if next()
            .get_untracked()
            .is_some_and(|at| at <= daynews_time::now_unix()) =>
        {
            scheduled();
            start();
        }
        _ => {}
    }
}
