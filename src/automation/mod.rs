//! CLI-agnostic scheduled automations (Codex-parity, sessions-owned).

pub mod findings;
pub mod runner;
pub mod schedule;
pub mod schema;
pub mod store;

pub use findings::prompt_result_hint;
pub use runner::{fire_automation, tick_scheduler};
pub use schedule::{humanize_schedule, next_fire_after, SchedulePreset};
pub use schema::{slugify_id, Automation, AutomationRun, AutomationStatus, RunStatus};
pub use store::{
    delete_automation, ensure_root, list_all_runs, list_automations, list_runs, load_automation,
    load_or_create_jitter_salt, load_state, mark_all_read, mark_run_read, save_automation,
    unread_count,
};
