//! Dashboard accounts, login sessions and API tokens.
//!
//! The store keeps only what it is given: password hashes, and SHA-256
//! hashes of session and API tokens. Hashing passwords and generating
//! tokens is the caller's job, so no secret ever reaches the database in
//! a usable form.

use rusqlite::{OptionalExtension, params};

use crate::{Store, StoreError};

/// A dashboard account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub username: String,
    /// PHC string (argon2id).
    pub password_hash: String,
    /// Base32 TOTP secret, when two-factor login is on.
    pub totp_secret: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
}

/// A login session, looked up by the hash of its cookie value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub user_id: i64,
    pub csrf: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub expires_at: i64,
}

/// An API token for scripts, without the token itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiToken {
    pub id: i64,
    pub name: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds, if it was ever used.
    pub last_used_at: Option<i64>,
}

fn user_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: row.get(0)?,
        username: row.get(1)?,
        password_hash: row.get(2)?,
        totp_secret: row.get(3)?,
        created_at: row.get(4)?,
    })
}

const USER_COLUMNS: &str = "id, username, password_hash, totp_secret, created_at";

impl Store {
    /// Number of accounts.
    pub fn user_count(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn()
            .query_row("SELECT count(*) FROM users", [], |r| r.get(0))?)
    }

    /// Creates the first account, but only while there is none. Returns
    /// `None` when an account already exists, so two people racing through
    /// first-run setup cannot both become admin.
    pub fn create_first_user(
        &self,
        username: &str,
        password_hash: &str,
        now: i64,
    ) -> Result<Option<User>, StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let count: i64 = tx.query_row("SELECT count(*) FROM users", [], |r| r.get(0))?;
        if count > 0 {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO users (username, password_hash, created_at) VALUES (?1, ?2, ?3)",
            params![username, password_hash, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Some(User {
            id,
            username: username.to_owned(),
            password_hash: password_hash.to_owned(),
            totp_secret: None,
            created_at: now,
        }))
    }

    /// Looks up an account by name.
    pub fn user_by_name(&self, username: &str) -> Result<Option<User>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE username = ?1"),
                [username],
                user_from_row,
            )
            .optional()?)
    }

    /// Looks up an account by id.
    pub fn user(&self, id: i64) -> Result<Option<User>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
                [id],
                user_from_row,
            )
            .optional()?)
    }

    /// Replaces an account's password hash and ends its other sessions,
    /// keeping only `keep_session` (the one that made the change).
    pub fn set_password(
        &self,
        user_id: i64,
        password_hash: &str,
        keep_session: Option<&str>,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE users SET password_hash = ?2 WHERE id = ?1",
            params![user_id, password_hash],
        )?;
        tx.execute(
            "DELETE FROM sessions WHERE user_id = ?1 AND token_hash IS NOT ?2",
            params![user_id, keep_session],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Turns two-factor login on (with a secret) or off.
    pub fn set_totp(&self, user_id: i64, secret: Option<&str>) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE users SET totp_secret = ?2 WHERE id = ?1",
            params![user_id, secret],
        )?;
        Ok(())
    }

    /// Stores a new session.
    pub fn create_session(&self, token_hash: &str, session: &Session) -> Result<(), StoreError> {
        let conn = self.conn();
        // Opportunistic cleanup keeps the table small without a timer.
        conn.execute(
            "DELETE FROM sessions WHERE expires_at <= ?1",
            [session.created_at],
        )?;
        conn.execute(
            "INSERT INTO sessions (token_hash, user_id, csrf, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                token_hash,
                session.user_id,
                session.csrf,
                session.created_at,
                session.expires_at
            ],
        )?;
        Ok(())
    }

    /// The session for a token hash, if it exists and has not expired.
    pub fn session(&self, token_hash: &str, now: i64) -> Result<Option<Session>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT user_id, csrf, created_at, expires_at FROM sessions
                 WHERE token_hash = ?1 AND expires_at > ?2",
                params![token_hash, now],
                |r| {
                    Ok(Session {
                        user_id: r.get(0)?,
                        csrf: r.get(1)?,
                        created_at: r.get(2)?,
                        expires_at: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// Moves a session's expiry (sliding idle timeout).
    pub fn extend_session(&self, token_hash: &str, expires_at: i64) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE sessions SET expires_at = ?2 WHERE token_hash = ?1",
            params![token_hash, expires_at],
        )?;
        Ok(())
    }

    /// Ends a session.
    pub fn delete_session(&self, token_hash: &str) -> Result<(), StoreError> {
        self.conn()
            .execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])?;
        Ok(())
    }

    /// Stores a new API token.
    pub fn create_api_token(
        &self,
        name: &str,
        token_hash: &str,
        now: i64,
    ) -> Result<ApiToken, StoreError> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO api_tokens (name, token_hash, created_at) VALUES (?1, ?2, ?3)",
            params![name, token_hash, now],
        )?;
        Ok(ApiToken {
            id: conn.last_insert_rowid(),
            name: name.to_owned(),
            created_at: now,
            last_used_at: None,
        })
    }

    /// Every API token, newest first.
    pub fn api_tokens(&self) -> Result<Vec<ApiToken>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at, last_used_at FROM api_tokens ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ApiToken {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                last_used_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Finds the API token with this hash and records the use.
    pub fn use_api_token(&self, token_hash: &str, now: i64) -> Result<Option<i64>, StoreError> {
        let conn = self.conn();
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM api_tokens WHERE token_hash = ?1",
                [token_hash],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            conn.execute(
                "UPDATE api_tokens SET last_used_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
        }
        Ok(id)
    }

    /// Revokes an API token. Returns whether it existed.
    pub fn delete_api_token(&self, id: i64) -> Result<bool, StoreError> {
        Ok(self
            .conn()
            .execute("DELETE FROM api_tokens WHERE id = ?1", [id])?
            > 0)
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    use super::Session;

    #[test]
    fn only_one_first_user() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.user_count().unwrap(), 0);
        let user = store
            .create_first_user("admin", "hash", 1)
            .unwrap()
            .unwrap();
        assert_eq!(store.create_first_user("eve", "hash", 2).unwrap(), None);
        assert_eq!(store.user_count().unwrap(), 1);
        assert_eq!(store.user_by_name("admin").unwrap(), Some(user.clone()));
        assert_eq!(store.user(user.id).unwrap(), Some(user));
        assert_eq!(store.user_by_name("eve").unwrap(), None);
    }

    #[test]
    fn sessions_expire_and_password_change_ends_others() {
        let store = Store::open_in_memory().unwrap();
        let user = store.create_first_user("admin", "h1", 0).unwrap().unwrap();
        let session = |exp| Session {
            user_id: user.id,
            csrf: "c".into(),
            created_at: 100,
            expires_at: exp,
        };
        store.create_session("a", &session(200)).unwrap();
        store.create_session("b", &session(200)).unwrap();
        assert!(store.session("a", 150).unwrap().is_some());
        assert_eq!(store.session("a", 200).unwrap(), None);
        store.extend_session("a", 300).unwrap();
        assert!(store.session("a", 250).unwrap().is_some());

        store.set_password(user.id, "h2", Some("a")).unwrap();
        assert!(store.session("a", 150).unwrap().is_some());
        assert_eq!(store.session("b", 150).unwrap(), None);
        assert_eq!(store.user(user.id).unwrap().unwrap().password_hash, "h2");

        store.delete_session("a").unwrap();
        assert_eq!(store.session("a", 150).unwrap(), None);
    }

    #[test]
    fn api_tokens() {
        let store = Store::open_in_memory().unwrap();
        let t = store.create_api_token("backup", "hash", 5).unwrap();
        assert_eq!(store.use_api_token("hash", 9).unwrap(), Some(t.id));
        assert_eq!(store.use_api_token("other", 9).unwrap(), None);
        let listed = store.api_tokens().unwrap();
        assert_eq!(listed[0].last_used_at, Some(9));
        assert!(store.delete_api_token(t.id).unwrap());
        assert!(!store.delete_api_token(t.id).unwrap());
        assert_eq!(store.use_api_token("hash", 10).unwrap(), None);
    }
}
