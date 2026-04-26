//! Upload orchestration: queue, worker threads, dedupe, retry/backoff.

use crate::db::{Db, UploadedRow};
use crate::uploader::{self, UploadError};
use crossbeam_channel::{Receiver, Sender};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

const NUM_WORKERS: usize = 2;
const BACKOFF: &[Duration] = &[
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(5 * 60),
    Duration::from_secs(30 * 60),
    Duration::from_secs(2 * 60 * 60),
    Duration::from_secs(6 * 60 * 60),
    Duration::from_secs(24 * 60 * 60),
];

pub const SKIPPED_PREFIXES: &[&str] = &["."];
pub const SKIPPED_SUFFIXES: &[&str] = &[".crdownload", ".part", ".tmp", ".partial"];

/// What kind of notification to fire after a successful upload, given the
/// current pipeline state. Pure — drives `notifier::notify`.
#[derive(Debug, PartialEq)]
pub enum NotificationAction {
    None,
    Single,
    Batch(u32),
}

/// Pure decision: should we fire a toast right now, and what kind?
pub fn decide_notification(
    in_flight: u32,
    queued: u32,
    successes_this_drain: u32,
    notifications_enabled: bool,
) -> NotificationAction {
    if !notifications_enabled {
        return NotificationAction::None;
    }
    if in_flight + queued > 0 {
        return NotificationAction::None;
    }
    match successes_this_drain {
        0 => NotificationAction::None,
        1 => NotificationAction::Single,
        n => NotificationAction::Batch(n),
    }
}

/// Decision for what to do with a candidate upload.
#[derive(Debug, PartialEq)]
pub enum UploadDecision {
    Filter,
    AlreadyUploadedSamePath,
    AlreadyUploadedDifferentPath,
    Proceed,
}

/// Cheap filter: filename-based predicates.
pub fn filter_by_name(filename: &str) -> bool {
    if SKIPPED_PREFIXES.iter().any(|p| filename.starts_with(p)) {
        return true;
    }
    if SKIPPED_SUFFIXES.iter().any(|s| filename.ends_with(s)) {
        return true;
    }
    false
}

/// Decide what to do given metadata + DB query results. Pure function; the
/// caller does the actual SHA computation and DB lookups.
pub fn decide(
    filename: &str,
    is_regular_file: bool,
    is_symlink: bool,
    size: u64,
    has_path_size_mtime_match: bool,
    has_sha256_match: Option<bool>,
) -> UploadDecision {
    if !is_regular_file || is_symlink || size == 0 {
        return UploadDecision::Filter;
    }
    if filter_by_name(filename) {
        return UploadDecision::Filter;
    }
    if has_path_size_mtime_match {
        return UploadDecision::AlreadyUploadedSamePath;
    }
    match has_sha256_match {
        Some(true) => UploadDecision::AlreadyUploadedDifferentPath,
        Some(false) => UploadDecision::Proceed,
        None => UploadDecision::Proceed,
    }
}

/// Stream-hash a file via SHA256.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Snapshot of pipeline state, read by the tray to render its menu.
#[derive(Debug, Clone, Default)]
pub struct PipelineState {
    pub watching_path: Option<String>,
    pub queued: usize,
    pub uploading: usize,
    pub failed_paths: Vec<String>,
    pub auth_invalid: bool,
}

/// Handle to a running pipeline. Drop to stop the workers (they'll finish
/// any in-flight upload and then exit).
pub struct Pipeline {
    state: Arc<Mutex<PipelineState>>,
    enqueue: Sender<PathBuf>,
}

/// Bundle of everything a worker thread needs to process one path.
/// Cloned-in via Arc/AppHandle::clone for each worker.
struct WorkerCtx {
    db: Arc<Db>,
    vault_url: String,
    get_token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    state: Arc<Mutex<PipelineState>>,
    work_rx: Receiver<PathBuf>,
    app: AppHandle,
    successes_this_drain: Arc<AtomicU32>,
    notify_lock: Arc<Mutex<()>>,
    watch_folder: String,
}

impl Pipeline {
    pub fn enqueue(&self, path: PathBuf) {
        let _ = self.enqueue.send(path);
    }
    pub fn snapshot(&self) -> PipelineState {
        self.state.lock().unwrap().clone()
    }
}

/// Spawn the pipeline. Reads from the watcher channel + the internal enqueue
/// channel; writes uploads via `uploader::upload_file`; records successes in
/// `db`. Maintains the public `PipelineState` snapshot.
pub fn start(
    watcher_rx: Receiver<PathBuf>,
    db: Arc<Db>,
    vault_url: String,
    get_session_token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    watching_path: String,
    app: AppHandle,
) -> Pipeline {
    let (enqueue_tx, enqueue_rx) = crossbeam_channel::unbounded::<PathBuf>();
    let state = Arc::new(Mutex::new(PipelineState {
        watching_path: Some(watching_path.clone()),
        ..Default::default()
    }));
    let successes_this_drain = Arc::new(AtomicU32::new(0));
    let notify_lock = Arc::new(Mutex::new(()));

    // Forwarder: drain watcher_rx + enqueue_rx into a single work_rx.
    let (work_tx, work_rx) = crossbeam_channel::unbounded::<PathBuf>();
    {
        let work_tx = work_tx.clone();
        std::thread::spawn(move || loop {
            crossbeam_channel::select! {
                recv(watcher_rx) -> p => {
                    match p {
                        Ok(p) => { let _ = work_tx.send(p); }
                        Err(_) => break,
                    }
                }
                recv(enqueue_rx) -> p => {
                    match p {
                        Ok(p) => { let _ = work_tx.send(p); }
                        Err(_) => break,
                    }
                }
            }
        });
    }

    // Worker threads.
    for _ in 0..NUM_WORKERS {
        let ctx = WorkerCtx {
            db: db.clone(),
            vault_url: vault_url.clone(),
            get_token: get_session_token.clone(),
            state: state.clone(),
            work_rx: work_rx.clone(),
            app: app.clone(),
            successes_this_drain: successes_this_drain.clone(),
            notify_lock: notify_lock.clone(),
            watch_folder: watching_path.clone(),
        };
        std::thread::spawn(move || {
            while let Ok(path) = ctx.work_rx.recv() {
                process_one(&ctx, &path);
            }
        });
    }

    Pipeline {
        state,
        enqueue: enqueue_tx,
    }
}

fn process_one(ctx: &WorkerCtx, path: &Path) {
    // Quick metadata + filter pass.
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return,
    };

    let is_symlink = meta.file_type().is_symlink();
    let is_regular = meta.is_file();
    let size = meta.len();
    let mtime_unix = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

    let path_str = path.to_string_lossy().to_string();
    let cheap = ctx
        .db
        .has_path_size_mtime(&path_str, size, mtime_unix)
        .unwrap_or(false);

    match decide(filename, is_regular, is_symlink, size, cheap, None) {
        UploadDecision::Filter => return,
        UploadDecision::AlreadyUploadedSamePath => return,
        UploadDecision::AlreadyUploadedDifferentPath => unreachable!("only with Some(true) sha"),
        UploadDecision::Proceed => {}
    }

    let sha256 = match sha256_file(path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let sha_match = ctx.db.has_sha256(&sha256).unwrap_or(false);

    if sha_match {
        let row = UploadedRow {
            path: path_str.clone(),
            size,
            mtime_unix,
            sha256: sha256.clone(),
            uploaded_at: now_unix(),
        };
        let _ = ctx.db.record_upload(&row);
        return;
    }

    {
        let mut s = ctx.state.lock().unwrap();
        s.uploading += 1;
    }

    let mut attempt: usize = 0;
    let result = loop {
        let token = match (ctx.get_token)() {
            Some(t) => t,
            None => {
                let mut s = ctx.state.lock().unwrap();
                s.auth_invalid = true;
                break Err(UploadError::Unauthorized);
            }
        };
        match uploader::upload_file(&ctx.vault_url, &token, path) {
            Ok(()) => break Ok(()),
            Err(UploadError::Unauthorized) => {
                let mut s = ctx.state.lock().unwrap();
                s.auth_invalid = true;
                break Err(UploadError::Unauthorized);
            }
            Err(UploadError::TooLarge) => break Err(UploadError::TooLarge),
            Err(e) => {
                if attempt >= BACKOFF.len() {
                    log::warn!(
                        "giving up on {} after {} attempts: {}",
                        path.display(),
                        attempt + 1,
                        e
                    );
                    break Err(e);
                }
                let delay = BACKOFF[attempt];
                log::info!(
                    "upload failed (attempt {}): {} — retrying in {:?}",
                    attempt + 1,
                    e,
                    delay
                );
                std::thread::sleep(delay);
                attempt += 1;
            }
        }
    };

    {
        let mut s = ctx.state.lock().unwrap();
        s.uploading = s.uploading.saturating_sub(1);
        if result.is_err() {
            s.failed_paths.push(path_str.clone());
        }
    }

    if result.is_ok() {
        let row = UploadedRow {
            path: path_str,
            size,
            mtime_unix,
            sha256,
            uploaded_at: now_unix(),
        };
        let _ = ctx.db.record_upload(&row);
        on_success(ctx, filename);
    } else {
        on_failure(ctx, filename);
    }
}

/// Bumps the success counter; if the queue has fully drained, fires the
/// appropriate Single or Batch toast and resets the counter. The
/// `notify_lock` mutex serializes the check-and-fire so two workers that
/// finish at the same time can't both fire.
fn on_success(ctx: &WorkerCtx, filename: &str) {
    ctx.successes_this_drain.fetch_add(1, Ordering::Relaxed);

    let _g = ctx.notify_lock.lock().unwrap();

    let in_flight = ctx.state.lock().unwrap().uploading as u32;
    let queued = ctx.work_rx.len() as u32;
    let cfg = crate::config::load().unwrap_or_default();

    let action = decide_notification(
        in_flight,
        queued,
        ctx.successes_this_drain.load(Ordering::Relaxed),
        cfg.notifications_enabled,
    );

    match action {
        NotificationAction::None => {}
        NotificationAction::Single => {
            crate::notifier::notify(
                &ctx.app,
                &cfg,
                crate::notifier::NotifyEvent::Single {
                    filename: filename.to_string(),
                },
            );
            ctx.successes_this_drain.store(0, Ordering::Relaxed);
        }
        NotificationAction::Batch(n) => {
            crate::notifier::notify(
                &ctx.app,
                &cfg,
                crate::notifier::NotifyEvent::Batch { count: n },
            );
            ctx.successes_this_drain.store(0, Ordering::Relaxed);
        }
    }
}

/// Permanent failure — fire the failure toast immediately. Doesn't touch
/// the success batch counter.
fn on_failure(ctx: &WorkerCtx, filename: &str) {
    let cfg = crate::config::load().unwrap_or_default();
    crate::notifier::notify(
        &ctx.app,
        &cfg,
        crate::notifier::NotifyEvent::Failure {
            filename: filename.to_string(),
            watch_folder: ctx.watch_folder.clone(),
        },
    );
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_skips_dotfiles() {
        assert!(filter_by_name(".DS_Store"));
        assert!(filter_by_name(".hidden"));
    }

    #[test]
    fn filter_skips_temp_suffixes() {
        assert!(filter_by_name("download.crdownload"));
        assert!(filter_by_name("upload.part"));
        assert!(filter_by_name("session.tmp"));
        assert!(filter_by_name("file.partial"));
    }

    #[test]
    fn filter_passes_normal_filenames() {
        assert!(!filter_by_name("clip.mp4"));
        assert!(!filter_by_name("screenshot.png"));
        assert!(!filter_by_name("foo bar.zip"));
    }

    #[test]
    fn decide_filter_for_dotfile() {
        let d = decide(".DS_Store", true, false, 100, false, None);
        assert_eq!(d, UploadDecision::Filter);
    }

    #[test]
    fn decide_filter_for_zero_byte() {
        let d = decide("clip.mp4", true, false, 0, false, None);
        assert_eq!(d, UploadDecision::Filter);
    }

    #[test]
    fn decide_filter_for_symlink() {
        let d = decide("clip.mp4", true, true, 100, false, None);
        assert_eq!(d, UploadDecision::Filter);
    }

    #[test]
    fn decide_filter_for_directory() {
        let d = decide("foo", false, false, 100, false, None);
        assert_eq!(d, UploadDecision::Filter);
    }

    #[test]
    fn decide_already_uploaded_same_path() {
        let d = decide("clip.mp4", true, false, 100, true, None);
        assert_eq!(d, UploadDecision::AlreadyUploadedSamePath);
    }

    #[test]
    fn decide_already_uploaded_different_path_via_sha() {
        let d = decide("clip.mp4", true, false, 100, false, Some(true));
        assert_eq!(d, UploadDecision::AlreadyUploadedDifferentPath);
    }

    #[test]
    fn decide_proceed_when_new() {
        let d = decide("clip.mp4", true, false, 100, false, Some(false));
        assert_eq!(d, UploadDecision::Proceed);
        let d = decide("clip.mp4", true, false, 100, false, None);
        assert_eq!(d, UploadDecision::Proceed);
    }

    #[test]
    fn sha256_file_matches_known_value() {
        use std::io::Write;
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("foo.txt");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"hello world").unwrap();
        let h = sha256_file(&p).unwrap();
        assert_eq!(
            h,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn decide_returns_none_when_disabled() {
        assert_eq!(
            decide_notification(0, 0, 5, false),
            NotificationAction::None
        );
    }

    #[test]
    fn decide_returns_none_with_in_flight() {
        assert_eq!(decide_notification(1, 0, 5, true), NotificationAction::None);
    }

    #[test]
    fn decide_returns_none_with_queued() {
        assert_eq!(decide_notification(0, 1, 5, true), NotificationAction::None);
    }

    #[test]
    fn decide_returns_none_with_zero_successes() {
        assert_eq!(decide_notification(0, 0, 0, true), NotificationAction::None);
    }

    #[test]
    fn decide_returns_single_for_one_success() {
        assert_eq!(
            decide_notification(0, 0, 1, true),
            NotificationAction::Single
        );
    }

    #[test]
    fn decide_returns_batch_for_two_or_more() {
        assert_eq!(
            decide_notification(0, 0, 2, true),
            NotificationAction::Batch(2)
        );
        assert_eq!(
            decide_notification(0, 0, 17, true),
            NotificationAction::Batch(17)
        );
    }
}
