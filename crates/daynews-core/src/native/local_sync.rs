//! Local subscriptions with optional day-sync transport. Feedbin sessions are not loaded.
use super::*;
thread_local! {
    static STATUS: Signal<SyncStatus> = Signal::new_in(day_reactive::Scope::root(), SyncStatus::default());
}
pub fn sync_status() -> Signal<SyncStatus> {
    STATUS.with(|s| *s)
}
pub fn sync_install() {
    #[cfg(feature = "experimental-webdav")]
    super::webdav::install();
}
pub fn sync_request_soon() {
    #[cfg(feature = "experimental-webdav")]
    super::webdav::request_soon();
}
pub fn sync_auto_now() {
    // Automatic foreground/refresh triggers must respect transport retry backoff.
    sync_request_soon();
}
pub fn sync_now() {
    #[cfg(feature = "experimental-webdav")]
    super::webdav::sync_now();
}
// The provider progress controls are hidden because there is no Feedbin session.
pub fn sync_cancel() {}

// Feedbin history retention has no effect on direct feed fetching.
pub fn set_sync_retention_days(_days: u32) {}
