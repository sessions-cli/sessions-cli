pub mod automation;
pub mod events;
pub mod manifest_sync;
pub mod metrics;
pub mod persist;
pub mod server;
pub mod spool;
pub mod state;
pub mod telemetry_worker;
pub mod tmux;

pub use server::run_daemon;
