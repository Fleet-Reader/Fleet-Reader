// SPDX-License-Identifier: Apache-2.0
use daynews_sync::{Error, progress::Progress};
#[derive(Clone, Debug, Default)]
pub struct SyncStatus {
    pub username: String,
    pub connected: bool,
    pub busy: bool,
    pub preview: bool,
    pub remote_feeds: usize,
    pub local_feeds: usize,
    pub pending: usize,
    pub last_sync: Option<i64>,
    pub error: Option<Error>,
    pub progress: Option<Progress>,
    pub elapsed_seconds: u64,
    pub remaining_seconds: Option<u64>,
    pub paused: bool,
    pub cancellable: bool,
}
