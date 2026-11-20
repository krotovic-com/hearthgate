//! Desired-state storage for Hearthgate.
//!
//! The desired state lives in a `SQLite` database (`state.db` in the data
//! directory). Every change is stored as a numbered, append-only
//! **revision**. The newest revision is the current state. Rolling back
//! creates a new revision with the content of an older one, so history is
//! never rewritten and every change can be audited.

use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use hearthgate_model::{DesiredState, ValidationErrors};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

mod accounts;

pub use accounts::{ApiToken, Session, User};

/// File name of the state database inside the data directory.
pub const DATABASE_FILE: &str = "state.db";

/// Errors from the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("cannot prepare the data directory: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Invalid(#[from] ValidationErrors),
    #[error("revision {0} does not exist")]
    NoSuchRevision(i64),
    #[error("revision {id} is corrupt: {reason}")]
    Corrupt { id: i64, reason: String },
    #[error(
        "the database was written by a newer Hearthgate (schema {found}, this build supports {supported})"
    )]
    TooNew { found: i64, supported: i64 },
}

/// Summary of a revision, without its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionMeta {
    /// Revision number, starting at 1.
    pub id: i64,
    /// When it was created, RFC 3339 in UTC.
    pub created_at: String,
    /// Who made the change, for example `cli:alice`.
    pub author: String,
    /// What the change was for.
    pub message: String,
    /// SHA-256 of the canonical JSON state.
    pub hash: String,
    /// Set when this revision restored an older one.
    pub rollback_of: Option<i64>,
}

/// A revision with its full desired state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub meta: RevisionMeta,
    pub state: DesiredState,
}

/// Result of storing a new state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// A new revision was created.
    Created(RevisionMeta),
    /// The state equals the current revision, so nothing was stored.
    Unchanged(RevisionMeta),
}

impl Applied {
    /// The revision that is now current.
    #[must_use]
    pub fn meta(&self) -> &RevisionMeta {
        match self {
            Self::Created(m) | Self::Unchanged(m) => m,
        }
    }
}

/// Ordered schema migrations. Each runs once, in a transaction, and bumps
/// `PRAGMA user_version`. Never edit a released migration; append a new one.
const MIGRATIONS: &[&str] = &[
    // 1: revisions and metadata.
    "CREATE TABLE revisions (
        id INTEGER PRIMARY KEY,
        created_at TEXT NOT NULL,
        author TEXT NOT NULL,
        message TEXT NOT NULL,
        hash TEXT NOT NULL,
        state TEXT NOT NULL,
        rollback_of INTEGER REFERENCES revisions(id)
    );
    CREATE TABLE meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );",
    // 2: dashboard accounts, sessions and API tokens.
    "CREATE TABLE users (
        id INTEGER PRIMARY KEY,
        username TEXT NOT NULL UNIQUE,
        password_hash TEXT NOT NULL,
        totp_secret TEXT,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE sessions (
        token_hash TEXT PRIMARY KEY,
        user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        csrf TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL
    );
    CREATE INDEX sessions_user ON sessions(user_id);
    CREATE TABLE api_tokens (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        token_hash TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        last_used_at INTEGER
    );",
];

/// Handle to the state database. Cheap to share between threads.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Opens (or creates) the store in `data_dir`, creating the directory
    /// with owner-only permissions and running pending migrations.
    pub fn open_dir(data_dir: &Path) -> Result<Self, StoreError> {
        if !data_dir.exists() {
            fs::create_dir_all(data_dir)?;
            restrict(data_dir, 0o700)?;
        }
        Self::open(&data_dir.join(DATABASE_FILE))
    }

    /// Opens (or creates) the database file at `path`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let created = !path.exists();
        let conn = Connection::open(path)?;
        if created {
            restrict(path, 0o600)?;
        }
        conn.busy_timeout(Duration::from_secs(5))?;
        set_wal(&conn)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::with_connection(conn)
    }

    /// Opens a throwaway in-memory store, for tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::with_connection(Connection::open_in_memory()?)
    }

    fn with_connection(mut conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Validates `state` and stores it as a new revision, unless it equals
    /// the current one.
    pub fn apply(
        &self,
        state: &DesiredState,
        author: &str,
        message: &str,
    ) -> Result<Applied, StoreError> {
        self.insert(state, author, message, None)
    }

    /// Makes the content of revision `id` current again by storing it as a
    /// new revision.
    pub fn rollback(&self, id: i64, author: &str) -> Result<Applied, StoreError> {
        let target = self.revision(id)?.ok_or(StoreError::NoSuchRevision(id))?;
        self.insert(
            &target.state,
            author,
            &format!("Roll back to revision {id}"),
            Some(id),
        )
    }

    fn insert(
        &self,
        state: &DesiredState,
        author: &str,
        message: &str,
        rollback_of: Option<i64>,
    ) -> Result<Applied, StoreError> {
        let state = state.clone().normalized();
        state.validate()?;
        let json = state.to_canonical_json();
        let hash = hex_sha256(&json);

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(current) = latest_meta(&tx)? {
            if current.hash == hash {
                return Ok(Applied::Unchanged(current));
            }
        }
        let created_at = humantime::format_rfc3339_seconds(SystemTime::now()).to_string();
        tx.execute(
            "INSERT INTO revisions (created_at, author, message, hash, state, rollback_of)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![created_at, author, message, hash, json, rollback_of],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Applied::Created(RevisionMeta {
            id,
            created_at,
            author: author.to_owned(),
            message: message.to_owned(),
            hash,
            rollback_of,
        }))
    }

    /// The current revision, or `None` before the first apply.
    pub fn current(&self) -> Result<Option<Revision>, StoreError> {
        let id = self.current_id()?;
        match id {
            Some(id) => self.revision(id),
            None => Ok(None),
        }
    }

    /// Number of the current revision. Cheap: the daemon polls it to notice
    /// changes made by the CLI.
    pub fn current_id(&self) -> Result<Option<i64>, StoreError> {
        Ok(self
            .conn()
            .query_row("SELECT max(id) FROM revisions", [], |row| row.get(0))?)
    }

    /// The current state, or an empty state before the first apply.
    pub fn current_state(&self) -> Result<DesiredState, StoreError> {
        Ok(self.current()?.map(|r| r.state).unwrap_or_default())
    }

    /// A revision with its content.
    pub fn revision(&self, id: i64) -> Result<Option<Revision>, StoreError> {
        let row = self
            .conn()
            .query_row(
                "SELECT id, created_at, author, message, hash, rollback_of, state
                 FROM revisions WHERE id = ?1",
                [id],
                |row| Ok((meta_from_row(row)?, row.get::<_, String>(6)?)),
            )
            .optional()?;
        let Some((meta, json)) = row else {
            return Ok(None);
        };
        if hex_sha256(&json) != meta.hash {
            return Err(StoreError::Corrupt {
                id,
                reason: "content does not match its hash".to_owned(),
            });
        }
        let state = DesiredState::from_json(&json).map_err(|e| StoreError::Corrupt {
            id,
            reason: e.to_string(),
        })?;
        Ok(Some(Revision { meta, state }))
    }

    /// The newest revisions first, at most `limit`.
    pub fn list(&self, limit: usize) -> Result<Vec<RevisionMeta>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, created_at, author, message, hash, rollback_of
             FROM revisions ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], meta_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Reads a metadata value.
    pub fn meta_get(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    /// Removes a metadata value.
    pub fn meta_delete(&self, key: &str) -> Result<(), StoreError> {
        self.conn()
            .execute("DELETE FROM meta WHERE key = ?1", [key])?;
        Ok(())
    }

    /// Sets a metadata value.
    pub fn meta_set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

fn latest_meta(conn: &Connection) -> rusqlite::Result<Option<RevisionMeta>> {
    conn.query_row(
        "SELECT id, created_at, author, message, hash, rollback_of
         FROM revisions ORDER BY id DESC LIMIT 1",
        [],
        meta_from_row,
    )
    .optional()
}

fn meta_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RevisionMeta> {
    Ok(RevisionMeta {
        id: row.get(0)?,
        created_at: row.get(1)?,
        author: row.get(2)?,
        message: row.get(3)?,
        hash: row.get(4)?,
        rollback_of: row.get(5)?,
    })
}

/// Switches to WAL. The switch needs a moment of exclusive access, and
/// SQLite reports "database is locked" without calling the busy handler
/// when another connection opens the same new file at that instant, so
/// retry for a while.
fn set_wal(conn: &Connection) -> Result<(), StoreError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::DatabaseBusy
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            other => return other.map_err(StoreError::from),
        }
    }
}

fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    let supported = i64::try_from(MIGRATIONS.len()).expect("few migrations");
    // One immediate transaction for the whole upgrade: when the daemon and
    // the CLI open a new database at the same moment, the second waits and
    // then sees the schema the first created.
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > supported {
        return Err(StoreError::TooNew {
            found: version,
            supported,
        });
    }
    for (n, sql) in (1..)
        .zip(MIGRATIONS)
        .skip(usize::try_from(version).unwrap_or(0))
    {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", n)?;
    }
    tx.commit()?;
    Ok(())
}

fn hex_sha256(text: &str) -> String {
    use std::fmt::Write;
    Sha256::digest(text.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(port: u16) -> DesiredState {
        DesiredState::from_toml(&format!(
            r#"
[[interfaces]]
name = "ssh"
listeners = [{{ port = {port}, mode = "raw" }}]
[[services]]
name = "sshd"
kind = "external"
host = "127.0.0.1"
ports = {{ ssh = 22 }}
[[routes]]
name = "ssh"
interface = "ssh"
listener = {port}
targets = [{{ service = "sshd", port = "ssh" }}]
"#
        ))
        .unwrap()
    }

    #[test]
    fn empty_store_has_no_revision() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.current().unwrap().is_none());
        assert_eq!(store.current_state().unwrap(), DesiredState::default());
    }

    #[test]
    fn apply_creates_numbered_revisions_and_skips_no_ops() {
        let store = Store::open_in_memory().unwrap();
        let a = store.apply(&state(2222), "test", "first").unwrap();
        assert!(matches!(&a, Applied::Created(m) if m.id == 1));
        let same = store.apply(&state(2222), "test", "again").unwrap();
        assert!(matches!(&same, Applied::Unchanged(m) if m.id == 1));
        let b = store.apply(&state(2223), "test", "second").unwrap();
        assert!(matches!(&b, Applied::Created(m) if m.id == 2));
        assert_eq!(store.current().unwrap().unwrap().state, state(2223));
        let ids: Vec<i64> = store.list(10).unwrap().iter().map(|m| m.id).collect();
        assert_eq!(ids, [2, 1]);
    }

    #[test]
    fn invalid_states_are_refused() {
        let store = Store::open_in_memory().unwrap();
        let mut bad = state(2222);
        bad.routes[0].targets[0].service = "missing".into();
        assert!(matches!(
            store.apply(&bad, "test", "bad"),
            Err(StoreError::Invalid(_))
        ));
        assert!(store.current_id().unwrap().is_none());
    }

    #[test]
    fn rollback_restores_exactly() {
        let store = Store::open_in_memory().unwrap();
        store.apply(&state(2222), "test", "first").unwrap();
        store.apply(&state(2223), "test", "second").unwrap();
        let back = store.rollback(1, "test").unwrap();
        let Applied::Created(meta) = back else {
            panic!("rollback should create a revision")
        };
        assert_eq!(meta.id, 3);
        assert_eq!(meta.rollback_of, Some(1));
        let r1 = store.revision(1).unwrap().unwrap();
        let r3 = store.revision(3).unwrap().unwrap();
        assert_eq!(r1.state, r3.state);
        assert_eq!(r1.meta.hash, r3.meta.hash);
        assert!(matches!(
            store.rollback(99, "test"),
            Err(StoreError::NoSuchRevision(99))
        ));
    }

    #[test]
    fn persists_across_reopen_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        {
            let store = Store::open_dir(&data).unwrap();
            store.apply(&state(2222), "test", "first").unwrap();
            store.meta_set("k", "v").unwrap();
        }
        let store = Store::open_dir(&data).unwrap();
        assert_eq!(store.current_id().unwrap(), Some(1));
        assert_eq!(store.meta_get("k").unwrap().as_deref(), Some("v"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&data), 0o700);
            assert_eq!(mode(&data.join(DATABASE_FILE)), 0o600);
        }
    }

    #[test]
    fn concurrent_first_opens_agree_on_the_schema() {
        for _ in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(DATABASE_FILE);
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let path = path.clone();
                    std::thread::spawn(move || Store::open(&path).map(|_| ()))
                })
                .collect();
            for h in handles {
                h.join().unwrap().unwrap();
            }
        }
    }

    #[test]
    fn detects_tampering() {
        let store = Store::open_in_memory().unwrap();
        store.apply(&state(2222), "test", "first").unwrap();
        store
            .conn()
            .execute(
                "UPDATE revisions SET state = replace(state, '2222', '2224')",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.revision(1),
            Err(StoreError::Corrupt { id: 1, .. })
        ));
    }

    #[test]
    fn refuses_newer_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        assert!(matches!(
            migrate(&mut conn),
            Err(StoreError::TooNew { found: 999, .. })
        ));
    }
}
