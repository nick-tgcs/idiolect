use std::time::Duration;

use idiolect_common::config::HistoryConfig;
use idiolect_ports::storage::MetadataStorePort;
use thiserror::Error;
use tokio::sync::watch;

/// Default interval between background pruning passes.
pub const DEFAULT_PRUNE_INTERVAL: Duration = Duration::from_secs(3600);

#[derive(Debug, Error)]
pub enum MaintenanceError<S> {
    #[error("storage error: {0}")]
    Storage(S),
}

/// Background maintenance: periodically prunes history older than the configured
/// retention window until a shutdown signal is received.
///
/// A `retention_days` of `0` disables pruning entirely (the loop simply waits for
/// shutdown), matching the master plan's "pruning can be disabled" requirement.
pub struct MaintenanceUseCase<S> {
    storage: S,
    config: HistoryConfig,
    interval: Duration,
    shutdown: watch::Receiver<()>,
}

impl<S> MaintenanceUseCase<S>
where
    S: MetadataStorePort,
    S::Error: std::fmt::Display,
{
    #[must_use]
    pub fn new(storage: S, config: HistoryConfig, shutdown: watch::Receiver<()>) -> Self {
        Self {
            storage,
            config,
            interval: DEFAULT_PRUNE_INTERVAL,
            shutdown,
        }
    }

    /// Overrides the pruning interval (primarily for tests).
    #[must_use]
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Runs the pruning loop until the shutdown channel fires. Returns `Ok(())`
    /// on a clean shutdown — shutdown is a normal outcome, not an error.
    ///
    /// Transient pruning failures are logged and the loop continues; a single
    /// failed pass must not tear down background maintenance.
    pub async fn run_pruning_loop(mut self) -> Result<(), MaintenanceError<S::Error>> {
        if self.config.retention_days == 0 {
            // Pruning disabled: stay alive until shutdown so the task lifecycle
            // matches the enabled case.
            let _ = self.shutdown.changed().await;
            return Ok(());
        }

        let mut ticker = tokio::time::interval(self.interval);
        // `interval` yields its first tick immediately; the daemon already prunes
        // once on startup, so consume that tick to avoid an immediate duplicate.
        ticker.tick().await;

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(error) = self.storage.prune_history(self.config.retention_days) {
                        eprintln!("maintenance: prune_history failed: {error}");
                    }
                }
                _ = self.shutdown.changed() => return Ok(()),
            }
        }
    }

    /// Runs a single pruning pass and returns the number of entries removed.
    /// Returns `0` without touching storage when pruning is disabled.
    pub async fn run_pruning_once(&mut self) -> Result<u64, MaintenanceError<S::Error>> {
        if self.config.retention_days == 0 {
            return Ok(0);
        }
        self.storage
            .prune_history(self.config.retention_days)
            .map_err(MaintenanceError::Storage)
    }
}
