// SPDX-License-Identifier: Apache-2.0
//! Isolate the subprocess chains used to verify preference persistence.
//!
//! Each chain gets a private store: the children run with `DAY_PREFS_DIR` naming a fresh
//! directory, which `day::prefs` uses in place of the in-process store a test build has
//! otherwise (docs/prefs.md), so parallel restart tests never share a file.
use std::process::Command;

pub(crate) struct PreferenceTest {
    directory: std::path::PathBuf,
}

impl PreferenceTest {
    pub(crate) fn new(key: &'static str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let directory = std::env::temp_dir().join(format!(
                "day-news-prefs-{key}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&directory) {
                Ok(()) => return Self { directory },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create isolated preference directory: {error}"),
            }
        }
    }

    pub(crate) fn child(&self, test: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", test]);
        // Override only child environments, never the multithreaded test runner's.
        // All restarts in one test retain the same private store.
        command.env("DAY_PREFS_DIR", &self.directory);
        command
    }
}

impl Drop for PreferenceTest {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
