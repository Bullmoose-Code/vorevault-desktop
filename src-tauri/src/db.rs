//! SQLite wrapper for the persistent uploaded-files index.

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

#[derive(Debug)]
pub enum DbError {
    Sqlite(rusqlite::Error),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Sqlite(e) => write!(f, "sqlite: {}", e),
        }
    }
}

impl std::error::Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Sqlite(e)
    }
}

pub struct Db {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UploadedRow {
    pub path: String,
    pub size: u64,
    pub mtime_unix: i64,
    pub sha256: String,
    pub uploaded_at: i64,
}

impl Db {
    /// Open the database at `<dir>/uploads.db`. Creates the file + applies
    /// schema (idempotent) on first open.
    pub fn open(dir: &Path) -> Result<Self, DbError> {
        let path = dir.join("uploads.db");
        let conn = Connection::open(&path)?;
        let db = Db { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Open an in-memory database for tests.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        let db = Db { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<(), DbError> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS uploaded_files (
                path TEXT PRIMARY KEY,
                size INTEGER NOT NULL,
                mtime_unix INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                uploaded_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS uploaded_files_sha256_idx
                ON uploaded_files (sha256);
            CREATE INDEX IF NOT EXISTS uploaded_files_uploaded_at_idx
                ON uploaded_files (uploaded_at DESC);
            "#,
        )?;
        Ok(())
    }

    /// Cheap dedupe check: has this exact `(path, size, mtime)` been uploaded?
    pub fn has_path_size_mtime(
        &self,
        path: &str,
        size: u64,
        mtime_unix: i64,
    ) -> Result<bool, DbError> {
        let row: Option<i64> = self.conn.query_row(
            "SELECT 1 FROM uploaded_files WHERE path = ?1 AND size = ?2 AND mtime_unix = ?3",
            params![path, size as i64, mtime_unix],
            |r| r.get(0),
        ).optional()?;
        Ok(row.is_some())
    }

    /// Has any path with this sha256 been uploaded? (Catches renames + duplicate copies.)
    pub fn has_sha256(&self, sha256: &str) -> Result<bool, DbError> {
        let row: Option<i64> = self.conn.query_row(
            "SELECT 1 FROM uploaded_files WHERE sha256 = ?1",
            params![sha256],
            |r| r.get(0),
        ).optional()?;
        Ok(row.is_some())
    }

    /// Insert (or upsert) a successful upload row.
    pub fn record_upload(&self, row: &UploadedRow) -> Result<(), DbError> {
        self.conn.execute(
            r#"
            INSERT INTO uploaded_files (path, size, mtime_unix, sha256, uploaded_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(path) DO UPDATE SET
              size = excluded.size,
              mtime_unix = excluded.mtime_unix,
              sha256 = excluded.sha256,
              uploaded_at = excluded.uploaded_at
            "#,
            params![
                row.path,
                row.size as i64,
                row.mtime_unix,
                row.sha256,
                row.uploaded_at,
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row() -> UploadedRow {
        UploadedRow {
            path: "/tmp/foo.mp4".to_string(),
            size: 1024,
            mtime_unix: 1_700_000_000,
            sha256: "deadbeef".to_string(),
            uploaded_at: 1_700_000_100,
        }
    }

    #[test]
    fn fresh_db_has_no_rows() {
        let db = Db::open_in_memory().unwrap();
        assert!(!db.has_path_size_mtime("/tmp/foo.mp4", 1024, 1_700_000_000).unwrap());
        assert!(!db.has_sha256("deadbeef").unwrap());
    }

    #[test]
    fn record_then_has_path_size_mtime_returns_true() {
        let db = Db::open_in_memory().unwrap();
        let row = sample_row();
        db.record_upload(&row).unwrap();
        assert!(db.has_path_size_mtime(&row.path, row.size, row.mtime_unix).unwrap());
    }

    #[test]
    fn has_path_size_mtime_is_strict_about_all_three_fields() {
        let db = Db::open_in_memory().unwrap();
        db.record_upload(&sample_row()).unwrap();
        assert!(!db.has_path_size_mtime("/tmp/foo.mp4", 999, 1_700_000_000).unwrap());
        assert!(!db.has_path_size_mtime("/tmp/foo.mp4", 1024, 9_999_999).unwrap());
        assert!(!db.has_path_size_mtime("/tmp/bar.mp4", 1024, 1_700_000_000).unwrap());
    }

    #[test]
    fn record_then_has_sha256_returns_true() {
        let db = Db::open_in_memory().unwrap();
        db.record_upload(&sample_row()).unwrap();
        assert!(db.has_sha256("deadbeef").unwrap());
        assert!(!db.has_sha256("cafef00d").unwrap());
    }

    #[test]
    fn record_upsert_overwrites_existing_path() {
        let db = Db::open_in_memory().unwrap();
        let mut row = sample_row();
        db.record_upload(&row).unwrap();
        row.size = 2048;
        row.mtime_unix = 1_700_001_000;
        row.sha256 = "newhash".to_string();
        row.uploaded_at = 1_700_001_100;
        db.record_upload(&row).unwrap();
        assert!(db.has_path_size_mtime(&row.path, 2048, 1_700_001_000).unwrap());
        assert!(db.has_sha256("newhash").unwrap());
        assert!(!db.has_sha256("deadbeef").unwrap());
    }

    #[test]
    fn schema_init_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        db.init_schema().unwrap();
        db.init_schema().unwrap();
    }
}
