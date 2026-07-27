//! Centralized multi-agent skills management (skillshare control plane).
//!
//! skillshare owns the store + multi-target sync; this module inventories paths,
//! detects drift, and shells out to skillshare for init/sync/audit/ui.

pub mod drift;
pub mod paths;
pub mod scan;
pub mod skillshare;

pub use drift::{detect_drift, presence_matrix_row, DriftItem, DriftKind, DriftReport};
pub use paths::SkillAgent;
pub use scan::{collect_inventory, SkillPackage, SkillsInventory};
pub use skillshare::{
    install_hint, run_audit, run_init, run_sync, run_ui, status, SkillshareStatus,
};

use serde::Serialize;
use std::path::Path;

/// Combined snapshot for CLI JSON / TUI load.
#[derive(Debug, Clone, Serialize)]
pub struct SkillsSnapshot {
    pub skillshare: SkillshareStatus,
    pub inventory: SkillsInventory,
    pub drift: DriftReport,
}

pub fn snapshot(home: &Path) -> SkillsSnapshot {
    let skillshare = status(home);
    let inventory = collect_inventory(home);
    let drift = detect_drift(home, &inventory);
    SkillsSnapshot {
        skillshare,
        inventory,
        drift,
    }
}
