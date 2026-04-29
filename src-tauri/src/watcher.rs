//! Recursive file watcher with per-path debounce and live add/remove of
//! watched roots.

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
        Self {
            debounce,
            pending: HashMap::new(),
        }
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
        let ready: Vec<PathBuf> = self
            .pending
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
    #[allow(dead_code)] // used by tests; not consumed by the bin target
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

/// Command sent from `WatcherHandle` to the watcher thread to mutate the
/// active set of watched roots at runtime.
enum WatcherCommand {
    Add(PathBuf),
    Remove(PathBuf),
}

/// Handle exposed to callers (the pipeline) for live add/remove of watched
/// roots without restarting the watcher.
pub struct WatcherHandle {
    cmd_tx: crossbeam_channel::Sender<WatcherCommand>,
}

impl WatcherHandle {
    pub fn add_root(&self, path: PathBuf) -> Result<(), WatcherError> {
        self.cmd_tx
            .send(WatcherCommand::Add(path))
            .map_err(|_| WatcherError::Notify(notify::Error::generic("watcher thread closed")))
    }

    pub fn remove_root(&self, path: PathBuf) -> Result<(), WatcherError> {
        self.cmd_tx
            .send(WatcherCommand::Remove(path))
            .map_err(|_| WatcherError::Notify(notify::Error::generic("watcher thread closed")))
    }
}

/// Start the file watcher in a background thread. Returns a `Receiver`
/// of debounce-ready paths and a `WatcherHandle` for live add/remove.
///
/// Internally uses one `notify::RecommendedWatcher` and registers each root
/// with `RecursiveMode::Recursive`. Add/remove commands flow over a
/// `crossbeam-channel` to the watcher thread, which dispatches `watch` /
/// `unwatch` against the live watcher.
pub fn start(
    roots: &[PathBuf],
    debounce_ms: u64,
) -> Result<(Receiver<PathBuf>, WatcherHandle), WatcherError> {
    use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

    let (raw_tx, raw_rx) = crossbeam_channel::unbounded::<Event>();
    let (ready_tx, ready_rx) = crossbeam_channel::unbounded::<PathBuf>();
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<WatcherCommand>();

    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(ev) = res {
                let _ = raw_tx.send(ev);
            }
        })?;

    for root in roots {
        watcher.watch(root, RecursiveMode::Recursive)?;
    }

    std::thread::spawn(move || {
        let mut watcher = watcher; // own the watcher in this thread
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
                recv(cmd_rx) -> cmd => match cmd {
                    Ok(WatcherCommand::Add(p)) => {
                        if let Err(e) = watcher.watch(&p, RecursiveMode::Recursive) {
                            log::warn!("watcher add_root({:?}) failed: {}", p, e);
                        }
                    }
                    Ok(WatcherCommand::Remove(p)) => {
                        if let Err(e) = watcher.unwatch(&p) {
                            log::warn!("watcher remove_root({:?}) failed: {}", p, e);
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

    Ok((ready_rx, WatcherHandle { cmd_tx }))
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

    use std::fs;
    use std::time::Duration as StdDuration;

    #[test]
    fn start_with_two_roots_emits_events_from_both() {
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let (rx, _handle) = start(
            &[dir_a.path().to_path_buf(), dir_b.path().to_path_buf()],
            200,
        )
        .expect("watcher should start");

        // Create a file in each watched root.
        fs::write(dir_a.path().join("a.txt"), "a").unwrap();
        fs::write(dir_b.path().join("b.txt"), "b").unwrap();

        // Drain up to 2 paths within a generous timeout.
        let mut got: Vec<PathBuf> = Vec::new();
        let deadline = std::time::Instant::now() + StdDuration::from_secs(5);
        while got.len() < 2 && std::time::Instant::now() < deadline {
            if let Ok(p) = rx.recv_timeout(StdDuration::from_millis(200)) {
                got.push(p);
            }
        }

        assert_eq!(
            got.len(),
            2,
            "expected events from both roots, got {:?}",
            got
        );
        let names: Vec<String> = got
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(&"a.txt".to_string()));
        assert!(names.contains(&"b.txt".to_string()));
    }

    #[test]
    fn watcher_handle_remove_root_stops_events_for_that_root() {
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let (rx, handle) = start(
            &[dir_a.path().to_path_buf(), dir_b.path().to_path_buf()],
            200,
        )
        .unwrap();

        // Drain any startup noise.
        while rx.try_recv().is_ok() {}

        handle
            .remove_root(dir_a.path().to_path_buf())
            .expect("remove_root succeeds");

        // Give the watcher thread a moment to process the unwatch command.
        std::thread::sleep(StdDuration::from_millis(300));
        while rx.try_recv().is_ok() {}

        // Now write to both roots.
        fs::write(dir_a.path().join("a.txt"), "a").unwrap();
        fs::write(dir_b.path().join("b.txt"), "b").unwrap();

        // Should only see b.txt, not a.txt, within the debounce window.
        let mut seen_a = false;
        let mut seen_b = false;
        let deadline = std::time::Instant::now() + StdDuration::from_secs(3);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(StdDuration::from_millis(200)) {
                Ok(p) => {
                    let n = p
                        .file_name()
                        .map(|x| x.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if n == "a.txt" {
                        seen_a = true;
                    }
                    if n == "b.txt" {
                        seen_b = true;
                    }
                }
                Err(_) => {}
            }
        }
        assert!(seen_b, "expected event for b.txt");
        assert!(
            !seen_a,
            "should NOT have seen event for a.txt after remove_root"
        );
    }

    #[test]
    fn watcher_handle_add_root_starts_watching_a_new_path() {
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let (rx, handle) = start(&[dir_a.path().to_path_buf()], 200).unwrap();
        while rx.try_recv().is_ok() {}

        handle
            .add_root(dir_b.path().to_path_buf())
            .expect("add_root succeeds");
        std::thread::sleep(StdDuration::from_millis(300));

        fs::write(dir_b.path().join("b.txt"), "b").unwrap();

        let deadline = std::time::Instant::now() + StdDuration::from_secs(3);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            if let Ok(p) = rx.recv_timeout(StdDuration::from_millis(200)) {
                if p.file_name().map(|n| n == "b.txt").unwrap_or(false) {
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "expected event for newly-added root");
    }
}
