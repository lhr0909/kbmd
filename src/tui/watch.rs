use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

const DEBOUNCE: Duration = Duration::from_millis(100);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) struct LiveReload {
    _watcher: Option<RecommendedWatcher>,
    receiver: Receiver<notify::Result<Event>>,
    last_change: Option<Instant>,
    next_reconciliation: Instant,
    disconnected_reported: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ReloadTick {
    pub reload: bool,
    pub errors: Vec<String>,
}

impl LiveReload {
    /// Starts a native watcher when possible. The caller can keep running on the polling
    /// reconciliation path when `setup_error` is present.
    pub fn start(path: &Path) -> (Self, Option<String>) {
        let (sender, receiver) = mpsc::channel();
        let (watcher, setup_error) = match notify::recommended_watcher(sender) {
            Ok(mut watcher) => match watcher.watch(path, RecursiveMode::Recursive) {
                Ok(()) => (Some(watcher), None),
                Err(error) => (
                    None,
                    Some(format!(
                        "File watching is unavailable ({error}); polling every 2s"
                    )),
                ),
            },
            Err(error) => (
                None,
                Some(format!(
                    "File watching is unavailable ({error}); polling every 2s"
                )),
            ),
        };
        (
            Self {
                _watcher: watcher,
                receiver,
                last_change: None,
                next_reconciliation: Instant::now() + RECONCILE_INTERVAL,
                disconnected_reported: false,
            },
            setup_error,
        )
    }

    pub fn tick(&mut self, now: Instant) -> ReloadTick {
        let mut result = ReloadTick::default();
        loop {
            match self.receiver.try_recv() {
                Ok(Ok(event)) => {
                    if event.need_rescan() || !matches!(event.kind, EventKind::Access(_)) {
                        // Reset on every event for a trailing-edge debounce. Atomic replacement
                        // normally arrives as a short create/rename/remove burst.
                        self.last_change = Some(now);
                    }
                }
                Ok(Err(error)) => result.errors.push(format!("File watch error: {error}")),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self._watcher.is_some() && !self.disconnected_reported {
                        result.errors.push(
                            "File watcher stopped; polling reconciliation remains active"
                                .to_owned(),
                        );
                        self.disconnected_reported = true;
                    }
                    break;
                }
            }
        }

        if self
            .last_change
            .is_some_and(|changed| now.saturating_duration_since(changed) >= DEBOUNCE)
        {
            self.last_change = None;
            result.reload = true;
        }
        if now >= self.next_reconciliation {
            self.next_reconciliation = now + RECONCILE_INTERVAL;
            result.reload = true;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use notify::event::ModifyKind;

    use super::*;

    #[test]
    fn native_events_use_a_trailing_edge_debounce() {
        let (sender, receiver) = mpsc::channel();
        let now = Instant::now();
        let mut reload = LiveReload {
            _watcher: None,
            receiver,
            last_change: None,
            next_reconciliation: now + RECONCILE_INTERVAL,
            disconnected_reported: false,
        };
        sender
            .send(Ok(Event::new(EventKind::Modify(ModifyKind::Any))))
            .unwrap();

        assert!(!reload.tick(now).reload);
        assert!(
            !reload
                .tick(now + DEBOUNCE - Duration::from_millis(1))
                .reload
        );
        assert!(reload.tick(now + DEBOUNCE).reload);
        assert!(!reload.tick(now + DEBOUNCE).reload);
    }

    #[test]
    fn polling_reconciliation_runs_without_a_watcher() {
        let (_sender, receiver) = mpsc::channel();
        let now = Instant::now();
        let mut reload = LiveReload {
            _watcher: None,
            receiver,
            last_change: None,
            next_reconciliation: now + RECONCILE_INTERVAL,
            disconnected_reported: false,
        };

        assert!(!reload.tick(now).reload);
        assert!(reload.tick(now + RECONCILE_INTERVAL).reload);
        assert!(!reload.tick(now + RECONCILE_INTERVAL).reload);
    }
}
