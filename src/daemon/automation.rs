//! Daemon tick for scheduled automations.

use crate::config::Config;
use tracing::{debug, error, info};

const AUTOMATION_TICK_SECS: u64 = 20;

/// Background loop: evaluate schedules, fire due automations, reconcile running runs.
pub async fn automation_loop(config: Config) {
    // Small delay so cold-boot restore settles first.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(AUTOMATION_TICK_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let config = config.clone();
        // Run sync filesystem + tmux work off the async runtime.
        let result =
            tokio::task::spawn_blocking(move || crate::automation::tick_scheduler(&config)).await;
        match result {
            Ok(Ok(0)) => debug!("automation tick: nothing due"),
            Ok(Ok(n)) => info!("automation tick: fired {n} run(s)"),
            Ok(Err(err)) => error!("automation tick failed: {err}"),
            Err(err) => error!("automation tick join failed: {err}"),
        }
    }
}
