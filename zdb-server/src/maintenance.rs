use std::time::Duration;

use crate::actor::ActorHandle;

/// Periodically runs compaction + stale node detection.
pub async fn maintenance_loop(actor: ActorHandle, interval_secs: u64) {
    let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
    // Skip the first immediate tick
    interval.tick().await;

    loop {
        interval.tick().await;
        tracing::debug!("maintenance: starting scheduled run");
        match actor.compact(false, false, None).await {
            Ok(_report) => tracing::debug!("maintenance: completed"),
            Err(e) => tracing::warn!("maintenance: failed: {e}"),
        }
    }
}
