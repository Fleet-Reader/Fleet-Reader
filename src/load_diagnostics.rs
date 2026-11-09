// SPDX-License-Identifier: Apache-2.0
//! Developer diagnostics, not UI text. Memory is process-wide, not an article allocation
//! count, and samples do not include a separate WebKit/browser renderer process.
use std::{
    cell::Cell,
    sync::atomic::{AtomicU64, Ordering},
};

const TARGET: &str = "day_news::load";
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct Load {
    id: u64,
    article: u64,
    kind: &'static str,
    trigger: &'static str,
    #[cfg(not(target_arch = "wasm32"))]
    started: std::time::Instant,
    memory_start: Option<u64>,
    memory_peak: Cell<Option<u64>>,
    finished: bool,
}

impl Load {
    pub fn start(kind: &'static str, article: u64, trigger: &'static str) -> Self {
        let memory_start = memory();
        let load = Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            article,
            kind,
            trigger,
            #[cfg(not(target_arch = "wasm32"))]
            started: std::time::Instant::now(),
            memory_start,
            memory_peak: Cell::new(memory_start),
            finished: false,
        };
        load.event("start", serde_json::json!({}));
        load
    }

    pub fn article(&self) -> u64 {
        self.article
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn event(&self, event: &str, details: serde_json::Value) {
        if !day::log_enabled!(target: TARGET, day::log::Level::Info) {
            return;
        }
        let current = memory();
        self.memory_peak.set(self.memory_peak.get().max(current));
        #[cfg(not(target_arch = "wasm32"))]
        let elapsed_ms = Some(self.started.elapsed().as_secs_f64() * 1000.0);
        // Day's raw DOM backend has no std::Instant clock. Do not report a stale
        // animation-frame timestamp as elapsed network time.
        #[cfg(target_arch = "wasm32")]
        let elapsed_ms: Option<f64> = None;
        day::info!(target: TARGET, "{}", serde_json::json!({
            "load_id": self.id, "article_id": self.article,
            "kind": self.kind, "trigger": self.trigger, "event": event,
            "elapsed_ms": elapsed_ms,
            "memory_metric": memory_metric(), "process_memory_bytes": current,
            "process_memory_start_bytes": self.memory_start,
            "process_memory_delta_bytes": delta(self.memory_start, current),
            "sampled_peak_process_memory_bytes": self.memory_peak.get(),
            "details": details,
        }));
    }

    pub fn finish(mut self, outcome: &str, details: serde_json::Value) {
        self.finished = true;
        self.event(outcome, details);
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        if !self.finished {
            self.event("cancelled", serde_json::json!({}));
        }
    }
}

fn memory() -> Option<u64> {
    day::log_enabled!(target: TARGET, day::log::Level::Info)
        .then(day_core::lifecycle::resident_memory)
        .flatten()
}

fn delta(start: Option<u64>, end: Option<u64>) -> Option<i128> {
    Some(i128::from(end?) - i128::from(start?))
}

fn memory_metric() -> &'static str {
    if cfg!(any(target_os = "macos", target_os = "ios")) {
        "physical_footprint"
    } else if cfg!(target_arch = "wasm32") {
        "wasm_linear_memory"
    } else if cfg!(windows) {
        "working_set"
    } else {
        "resident_set"
    }
}

/// Per-window article selection -> snapshot -> generated HTML -> native load callback.
#[derive(Clone, Default)]
pub struct ArticleLoad(std::rc::Rc<std::cell::RefCell<Option<Load>>>);

impl ArticleLoad {
    pub fn select(&self, id: Option<u64>) {
        let mut pending = self.0.borrow_mut();
        if pending.as_ref().map(Load::article) == id {
            return;
        }
        *pending = id.map(|id| Load::start("article", id, "selection"));
    }

    pub fn event(&self, id: u64, event: &str, details: serde_json::Value) {
        if let Some(load) = self.0.borrow().as_ref().filter(|load| load.article() == id) {
            load.event(event, details);
        }
    }

    pub fn finish(&self, id: u64, outcome: &str) {
        let mut pending = self.0.borrow_mut();
        if pending.as_ref().is_some_and(|load| load.article() == id) {
            pending
                .take()
                .unwrap()
                .finish(outcome, serde_json::json!({}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn completed_and_aborted_loads_each_emit_one_terminal_record() {
        // Isolate the logger from tests that launch Day and install its process logger.
        const CHILD: &str = "DAY_NEWS_TEST_LOAD_LOGGER";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "load_diagnostics::tests::completed_and_aborted_loads_each_emit_one_terminal_record"])
                .env(CHILD, "1").output().unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        struct Logger(std::sync::Mutex<Vec<serde_json::Value>>);
        impl day::log::Log for Logger {
            fn enabled(&self, metadata: &day::log::Metadata<'_>) -> bool {
                metadata.target() == TARGET
            }
            fn log(&self, record: &day::log::Record<'_>) {
                if self.enabled(record.metadata()) {
                    self.0
                        .lock()
                        .unwrap()
                        .push(serde_json::from_str(&record.args().to_string()).unwrap());
                }
            }
            fn flush(&self) {}
        }
        static LOGGER: Logger = Logger(std::sync::Mutex::new(Vec::new()));
        day::log::set_logger(&LOGGER).unwrap();
        day::log::set_max_level(day::log::LevelFilter::Info);
        let completed = Load::start("reader_view", 1, "automatic");
        let completed_id = completed.id();
        completed.finish("extracted", serde_json::json!({"content_bytes": 42}));
        let cancelled = Load::start("reader_view", 2, "explicit");
        let cancelled_id = cancelled.id();
        drop(cancelled);
        let records = LOGGER.0.lock().unwrap();
        for (id, expected) in [(completed_id, "extracted"), (cancelled_id, "cancelled")] {
            let events = records
                .iter()
                .filter(|r| r["load_id"] == id)
                .collect::<Vec<_>>();
            assert_eq!(events.len(), 2);
            assert_eq!(events[0]["event"], "start");
            assert_eq!(events[1]["event"], expected);
            assert!(events[1]["elapsed_ms"].as_f64().unwrap() >= 0.0);
            assert!(events[1].get("process_memory_bytes").is_some());
        }
    }

    #[test]
    fn process_memory_delta_preserves_decreases_and_missing_samples() {
        assert_eq!(delta(Some(200), Some(150)), Some(-50));
        assert_eq!(delta(Some(150), Some(200)), Some(50));
        assert_eq!(delta(None, Some(200)), None);
        assert_eq!(delta(Some(200), None), None);
    }

    #[test]
    fn stale_completions_do_not_finish_the_new_article() {
        let loads = ArticleLoad::default();
        loads.select(Some(1));
        loads.select(Some(2));
        loads.finish(1, "loaded");
        assert_eq!(loads.0.borrow().as_ref().unwrap().article(), 2);
        loads.finish(2, "loaded");
        assert!(loads.0.borrow().is_none());
    }
}
