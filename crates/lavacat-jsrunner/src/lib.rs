//! Opt-in process boundary for issue #4. The controller has no live DOM or JS
//! engine; the runner owns both and performs style/layout in its own process.
//! Layout frames are immutable shared-memory archives, validated before access.
//!
//! This is an architectural prototype alongside the existing window path, not
//! its replacement. Frame painting/input integration, JS heap snapshots, slot
//! recycling, background-page merging and engine packages remain separate work.
#![forbid(unsafe_code)]

#[cfg(feature = "controller")]
mod controller;
mod protocol;
#[cfg(feature = "quickjs")]
mod runtime;
mod snapshot;

#[cfg(feature = "controller")]
pub use controller::RunnerController;
pub use snapshot::{LayoutFrame, LayoutNodeView};

/// Protocol ABI. A distribution must agree before a context can be created.
pub const ABI_VERSION: u32 = 1;
/// Limit for immutable layout frames / DOM checkpoints.
pub const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

/// Engine-independent runner failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerError(pub String);
impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for RunnerError {}
fn error(value: impl std::fmt::Display) -> RunnerError {
    RunnerError(value.to_string())
}

/// Run the QuickJS distribution's private IPC service. DOM and JS stay together
/// on this thread (WHATWG HTML §8.1.4). Stdout is reserved for bootstrap only.
#[cfg(feature = "quickjs")]
pub fn run_quickjs_service() -> Result<(), RunnerError> {
    runtime::serve()
}
