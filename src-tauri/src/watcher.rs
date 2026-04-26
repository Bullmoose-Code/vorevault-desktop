//! Recursive file watcher with per-path debounce.

use crossbeam_channel::Receiver;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Pure-logic debounce buffer: tracks the most recent event time per path,
/// emits a path as "ready" only after `debounce` elapses with no further events.
#[derive(Debug)]
pub struct Debouncer {
    debounce: Duration,
    pending: HashMap<PathBuf, Instant>,
}

impl Debouncer {
    pub fn new(debounce: Duration) -> Self {
        Self { debounce, pending: HashMap::new() }
    }

    /// Record a fresh event for `path`. Returns nothing — the caller polls
    /// `take_ready(now)` periodically to drain ready paths.
    pub fn note_event(&mut self, path: PathBuf, now: Instant) {
        self.pending.insert(path, now);
    }

    /// Return all paths whose most-recent event was at least `debounce` ago,
    /// removing them from the pending set.
    pub fn take_ready(&mut self, now: Instant) -> Vec<PathBuf> {
        let threshold = now.checked_sub(self.debounce).unwrap_or(now);
        let ready: Vec<PathBuf> = self.pending
            .iter()
            .filter(|(_, &t)| t <= threshold)
            .map(|(p, _)| p.clone())
            .collect();
        for p in &ready {
            self.pending.remove(p);
        }
        ready
    }

    /// How many paths are currently held in the pending map.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[derive(Debug)]
pub enum WatcherError {
    Notify(notify::Error),
}

impl std::fmt::Display for WatcherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatcherError::Notify(e) => write!(f, "notify: {}", e),
        }
    }
}

impl std::error::Error for WatcherError {}

impl From<notify::Error> for WatcherError {
    fn from(e: notify::Error) -> Self {
        WatcherError::Notify(e)
    }
}

/// Start the file watcher in a background thread. Returns a `Receiver`
/// that emits paths as they become "ready" (debounce elapsed, no further
/// events for that path).
///
/// The watcher uses notify's recommended platform-native backend and is
/// recursive over the given root.
pub fn start(root: PathBuf, debounce_ms: u64) -> Result<Receiver<PathBuf>, WatcherError> {
    use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

    // Two channels: notify→thread (raw events), thread→consumer (ready paths).
    let (raw_tx, raw_rx) = crossbeam_channel::unbounded::<Event>();
    let (ready_tx, ready_rx) = crossbeam_channel::unbounded::<PathBuf>();

    // Build the watcher; closure forwards events via the raw channel.
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(ev) = res {
            let _ = raw_tx.send(ev);
        }
    })?;

    watcher.watch(&root, RecursiveMode::Recursive)?;

    // Background thread owns the watcher (so it isn't dropped) + the debouncer.
    std::thread::spawn(move || {
        let _watcher = watcher;
        let mut deb = Debouncer::new(Duration::from_millis(debounce_ms));
        let tick = crossbeam_channel::tick(Duration::from_millis(500));

        loop {
            crossbeam_channel::select! {
                recv(raw_rx) -> msg => match msg {
                    Ok(ev) => {
                        let kind_ok = matches!(
                            ev.kind,
                            EventKind::Create(_) | EventKind::Modify(_)
                        );
                        if !kind_ok { continue; }
                        for p in ev.paths {
                            if p.is_file() {
                                deb.note_event(p, Instant::now());
                            }
                        }
                    }
                    Err(_) => break,
                },
                recv(tick) -> _ => {
                    let now = Instant::now();
                    for path in deb.take_ready(now) {
                        if ready_tx.send(path).is_err() { return; }
                    }
                }
            }
        }
    });

    Ok(ready_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: u64) -> Instant {
        Instant::now() + Duration::from_secs(secs)
    }

    #[test]
    fn empty_debouncer_emits_nothing() {
        let mut d = Debouncer::new(Duration::from_secs(5));
        assert!(d.take_ready(t(100)).is_empty());
    }

    #[test]
    fn single_event_becomes_ready_after_debounce() {
        let mut d = Debouncer::new(Duration::from_secs(5));
        d.note_event(PathBuf::from("/a"), t(0));
        assert!(d.take_ready(t(3)).is_empty());
        assert_eq!(d.pending_count(), 1);
        let ready = d.take_ready(t(5));
        assert_eq!(ready, vec![PathBuf::from("/a")]);
        assert_eq!(d.pending_count(), 0);
    }

    #[test]
    fn re_event_resets_the_debounce_for_that_path() {
        let mut d = Debouncer::new(Duration::from_secs(5));
        d.note_event(PathBuf::from("/a"), t(0));
        d.note_event(PathBuf::from("/a"), t(3));
        assert!(d.take_ready(t(5)).is_empty());
        let ready = d.take_ready(t(8));
        assert_eq!(ready, vec![PathBuf::from("/a")]);
    }

    #[test]
    fn different_paths_have_independent_timers() {
        let mut d = Debouncer::new(Duration::from_secs(5));
        d.note_event(PathBuf::from("/a"), t(0));
        d.note_event(PathBuf::from("/b"), t(3));
        let mut ready = d.take_ready(t(6));
        ready.sort();
        assert_eq!(ready, vec![PathBuf::from("/a")]);
        let ready = d.take_ready(t(9));
        assert_eq!(ready, vec![PathBuf::from("/b")]);
    }

    #[test]
    fn take_ready_is_idempotent_for_already_ready_paths() {
        let mut d = Debouncer::new(Duration::from_secs(5));
        d.note_event(PathBuf::from("/a"), t(0));
        let _ = d.take_ready(t(5));
        assert!(d.take_ready(t(10)).is_empty());
    }
}
