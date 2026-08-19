//! Transactional provisioning records.
//!
//! The trait is the application boundary: today it is a local SQLite file, while an online
//! implementation can preserve the allocation semantics without leaking SQL into the worker.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservation {
    pub serial: u32,
    pub uid: String,
    pub status: String,
    pub resumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepositoryError {
    #[error("provisioning database unavailable: {0}")]
    Unavailable(String),
    #[error("serial {serial} is already associated with MCU {existing_uid}")]
    Conflict { serial: u32, existing_uid: String },
    #[error("serial must be a positive decimal u32")]
    InvalidSerial,
}

impl From<rusqlite::Error> for RepositoryError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Unavailable(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub id: i64,
    pub at_ms: i64,
    pub serial: Option<u32>,
    pub uid: Option<String>,
    pub action: String,
    pub outcome: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEvidence {
    pub uid: String,
    pub idcode: String,
    pub dev_id: String,
    pub flash_kb: u16,
    pub option_bytes: String,
    pub probe_name: String,
    pub probe_serial: String,
    pub probe_firmware: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptEvidence {
    pub serial: u32,
    pub uid: String,
    pub firmware_version: String,
    pub bootloader_sha256: String,
    pub application_sha256: String,
    pub bundle_sha256: String,
    pub provenance: String,
    pub outcome: String,
    pub detail: String,
}

pub trait ProvisioningRepository: Send {
    fn health(&self) -> Result<(), RepositoryError>;
    fn next_serial(&self) -> Result<u32, RepositoryError>;
    fn set_next_at_least(&mut self, serial: u32) -> Result<u32, RepositoryError>;
    fn reserve(
        &mut self,
        uid: &str,
        requested: Option<u32>,
        allow_reassignment: bool,
    ) -> Result<Reservation, RepositoryError>;
    fn mark_provisioned(
        &mut self,
        serial: u32,
        uid: &str,
        detail: &str,
    ) -> Result<(), RepositoryError>;
    fn mark_failed(&mut self, serial: u32, uid: &str, detail: &str) -> Result<(), RepositoryError>;
    fn record_action(
        &mut self,
        serial: Option<u32>,
        uid: Option<&str>,
        action: &str,
        outcome: &str,
        detail: &str,
    ) -> Result<(), RepositoryError>;
    fn record_device(&mut self, evidence: &DeviceEvidence) -> Result<(), RepositoryError>;
    fn record_attempt(&mut self, evidence: &AttemptEvidence) -> Result<(), RepositoryError>;
    fn history(
        &self,
        serial: Option<u32>,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Action>, RepositoryError>;
}

pub struct SqliteRepository {
    connection: Connection,
    path: PathBuf,
}

impl core::fmt::Debug for SqliteRepository {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SqliteRepository")
            .field("path", &self.path)
            .finish()
    }
}

impl SqliteRepository {
    pub fn open(path: &Path) -> Result<Self, RepositoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| RepositoryError::Unavailable(error.to_string()))?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS meta(
                 key TEXT PRIMARY KEY,
                 value INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS associations(
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 serial INTEGER NOT NULL,
                 uid TEXT NOT NULL,
                 status TEXT NOT NULL CHECK(status IN ('reserved','provisioned','failed','superseded','conflicted')),
                 created_ms INTEGER NOT NULL,
                 updated_ms INTEGER NOT NULL
             );
             CREATE UNIQUE INDEX IF NOT EXISTS one_active_serial
               ON associations(serial) WHERE status IN ('reserved','provisioned','failed');
             CREATE UNIQUE INDEX IF NOT EXISTS one_active_uid
               ON associations(uid) WHERE status IN ('reserved','provisioned','failed');
             CREATE TABLE IF NOT EXISTS actions(
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 at_ms INTEGER NOT NULL,
                 serial INTEGER,
                 uid TEXT,
                 action TEXT NOT NULL,
                 outcome TEXT NOT NULL,
                 detail TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS action_serial_time ON actions(serial, at_ms);
             CREATE TABLE IF NOT EXISTS devices(
                 uid TEXT PRIMARY KEY,
                 idcode TEXT NOT NULL, dev_id TEXT NOT NULL, flash_kb INTEGER NOT NULL,
                 option_bytes TEXT NOT NULL, probe_name TEXT NOT NULL, probe_serial TEXT NOT NULL,
                 probe_firmware TEXT NOT NULL, first_seen_ms INTEGER NOT NULL, last_seen_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS attempts(
                 id INTEGER PRIMARY KEY AUTOINCREMENT, at_ms INTEGER NOT NULL,
                 serial INTEGER NOT NULL, uid TEXT NOT NULL, firmware_version TEXT NOT NULL,
                 bootloader_sha256 TEXT NOT NULL, application_sha256 TEXT NOT NULL,
                 bundle_sha256 TEXT NOT NULL, provenance TEXT NOT NULL,
                 outcome TEXT NOT NULL, detail TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS attempt_serial_time ON attempts(serial,at_ms);
             INSERT OR IGNORE INTO meta(key,value) VALUES('next_serial',1);"
        )?;
        let mut repository = Self {
            connection,
            path: path.to_path_buf(),
        };
        repository.recover_counter()?;
        Ok(repository)
    }

    pub fn in_memory() -> Result<Self, RepositoryError> {
        let connection = Connection::open_in_memory()?;
        let mut result = Self {
            connection,
            path: PathBuf::from(":memory:"),
        };
        result.connection.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE meta(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
             INSERT INTO meta VALUES('next_serial',1);
             CREATE TABLE associations(id INTEGER PRIMARY KEY AUTOINCREMENT,serial INTEGER NOT NULL,uid TEXT NOT NULL,status TEXT NOT NULL,created_ms INTEGER NOT NULL,updated_ms INTEGER NOT NULL);
             CREATE UNIQUE INDEX one_active_serial ON associations(serial) WHERE status IN ('reserved','provisioned','failed');
             CREATE UNIQUE INDEX one_active_uid ON associations(uid) WHERE status IN ('reserved','provisioned','failed');
             CREATE TABLE actions(id INTEGER PRIMARY KEY AUTOINCREMENT,at_ms INTEGER NOT NULL,serial INTEGER,uid TEXT,action TEXT NOT NULL,outcome TEXT NOT NULL,detail TEXT NOT NULL);
             CREATE INDEX action_serial_time ON actions(serial,at_ms);
             CREATE TABLE devices(uid TEXT PRIMARY KEY,idcode TEXT NOT NULL,dev_id TEXT NOT NULL,flash_kb INTEGER NOT NULL,option_bytes TEXT NOT NULL,probe_name TEXT NOT NULL,probe_serial TEXT NOT NULL,probe_firmware TEXT NOT NULL,first_seen_ms INTEGER NOT NULL,last_seen_ms INTEGER NOT NULL);
             CREATE TABLE attempts(id INTEGER PRIMARY KEY AUTOINCREMENT,at_ms INTEGER NOT NULL,serial INTEGER NOT NULL,uid TEXT NOT NULL,firmware_version TEXT NOT NULL,bootloader_sha256 TEXT NOT NULL,application_sha256 TEXT NOT NULL,bundle_sha256 TEXT NOT NULL,provenance TEXT NOT NULL,outcome TEXT NOT NULL,detail TEXT NOT NULL);"
        )?;
        result.recover_counter()?;
        Ok(result)
    }

    fn recover_counter(&mut self) -> Result<(), RepositoryError> {
        let max: u32 = self.connection.query_row(
            "SELECT COALESCE(MAX(serial),0) FROM associations",
            [],
            |row| row.get(0),
        )?;
        self.connection.execute(
            "UPDATE meta SET value=MAX(value,?1) WHERE key='next_serial'",
            params![i64::from(max.saturating_add(1))],
        )?;
        Ok(())
    }
}

impl ProvisioningRepository for SqliteRepository {
    fn health(&self) -> Result<(), RepositoryError> {
        self.connection.query_row("PRAGMA quick_check", [], |row| {
            let result: String = row.get(0)?;
            if result == "ok" {
                Ok(())
            } else {
                Err(rusqlite::Error::InvalidQuery)
            }
        })?;
        Ok(())
    }

    fn next_serial(&self) -> Result<u32, RepositoryError> {
        Ok(self.connection.query_row(
            "SELECT value FROM meta WHERE key='next_serial'",
            [],
            |row| row.get(0),
        )?)
    }

    fn set_next_at_least(&mut self, serial: u32) -> Result<u32, RepositoryError> {
        if serial == 0 || serial == u32::MAX {
            return Err(RepositoryError::InvalidSerial);
        }
        self.connection.execute(
            "UPDATE meta SET value=MAX(value,?1) WHERE key='next_serial'",
            params![serial],
        )?;
        self.next_serial()
    }

    fn reserve(
        &mut self,
        uid: &str,
        requested: Option<u32>,
        allow_reassignment: bool,
    ) -> Result<Reservation, RepositoryError> {
        let now = now_ms();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((serial, status)) = transaction.query_row(
            "SELECT serial,status FROM associations WHERE uid=?1 AND status IN ('reserved','provisioned','failed') ORDER BY id DESC LIMIT 1",
            params![uid], |row| Ok((row.get::<_,u32>(0)?, row.get::<_,String>(1)?))
        ).optional()? {
            if requested.is_none_or(|requested| requested == serial) {
                transaction.execute(
                    "INSERT INTO actions(at_ms,serial,uid,action,outcome,detail) VALUES(?1,?2,?3,'reservation-resumed','ok',?4)",
                    params![now, serial, uid, status],
                )?;
                transaction.commit()?;
                return Ok(Reservation { serial, uid: uid.to_owned(), status, resumed: true });
            }
            if !allow_reassignment {
                return Err(RepositoryError::Conflict {
                    serial: requested.unwrap(),
                    existing_uid: format!("{uid} (currently serial {serial})"),
                });
            }
            transaction.execute(
                "UPDATE associations SET status='superseded',updated_ms=?1 WHERE uid=?2 AND status IN ('reserved','provisioned','failed')",
                params![now, uid],
            )?;
            transaction.execute(
                "INSERT INTO actions(at_ms,serial,uid,action,outcome,detail) VALUES(?1,?2,?3,'uid-serial-overridden','override',?4)",
                params![now, requested.unwrap(), uid, format!("former serial {serial}")],
            )?;
        }

        let serial = requested.unwrap_or(transaction.query_row(
            "SELECT value FROM meta WHERE key='next_serial'",
            [],
            |row| row.get(0),
        )?);
        if serial == 0 || serial == u32::MAX {
            return Err(RepositoryError::InvalidSerial);
        }
        if let Some(existing_uid) = transaction.query_row(
            "SELECT uid FROM associations WHERE serial=?1 AND status IN ('reserved','provisioned','failed') ORDER BY id DESC LIMIT 1",
            params![serial], |row| row.get::<_,String>(0)
        ).optional()? {
            if existing_uid != uid && !allow_reassignment {
                return Err(RepositoryError::Conflict { serial, existing_uid });
            }
            if existing_uid != uid {
                transaction.execute(
                    "UPDATE associations SET status='superseded',updated_ms=?1 WHERE serial=?2 AND status IN ('reserved','provisioned','failed')",
                    params![now, serial],
                )?;
                transaction.execute(
                    "INSERT INTO actions(at_ms,serial,uid,action,outcome,detail) VALUES(?1,?2,?3,'serial-reassigned','override',?4)",
                    params![now, serial, uid, existing_uid],
                )?;
            }
        }
        transaction.execute(
            "INSERT INTO associations(serial,uid,status,created_ms,updated_ms) VALUES(?1,?2,'reserved',?3,?3)",
            params![serial, uid, now],
        )?;
        transaction.execute(
            "UPDATE meta SET value=MAX(value,?1) WHERE key='next_serial'",
            params![i64::from(serial) + 1],
        )?;
        transaction.execute(
            "INSERT INTO actions(at_ms,serial,uid,action,outcome,detail) VALUES(?1,?2,?3,'serial-reserved','ok','')",
            params![now, serial, uid],
        )?;
        transaction.commit()?;
        Ok(Reservation {
            serial,
            uid: uid.to_owned(),
            status: "reserved".into(),
            resumed: false,
        })
    }

    fn mark_provisioned(
        &mut self,
        serial: u32,
        uid: &str,
        detail: &str,
    ) -> Result<(), RepositoryError> {
        self.connection.execute("UPDATE associations SET status='provisioned',updated_ms=?1 WHERE serial=?2 AND uid=?3 AND status IN ('reserved','failed')", params![now_ms(),serial,uid])?;
        self.record_action(Some(serial), Some(uid), "provision-complete", "ok", detail)
    }

    fn mark_failed(&mut self, serial: u32, uid: &str, detail: &str) -> Result<(), RepositoryError> {
        self.connection.execute("UPDATE associations SET status='failed',updated_ms=?1 WHERE serial=?2 AND uid=?3 AND status='reserved'", params![now_ms(),serial,uid])?;
        self.record_action(
            Some(serial),
            Some(uid),
            "provision-failed",
            "failed",
            detail,
        )
    }

    fn record_action(
        &mut self,
        serial: Option<u32>,
        uid: Option<&str>,
        action: &str,
        outcome: &str,
        detail: &str,
    ) -> Result<(), RepositoryError> {
        self.connection.execute(
            "INSERT INTO actions(at_ms,serial,uid,action,outcome,detail) VALUES(?1,?2,?3,?4,?5,?6)",
            params![now_ms(), serial, uid, action, outcome, detail],
        )?;
        Ok(())
    }

    fn record_device(&mut self, evidence: &DeviceEvidence) -> Result<(), RepositoryError> {
        let now = now_ms();
        self.connection.execute(
            "INSERT INTO devices(uid,idcode,dev_id,flash_kb,option_bytes,probe_name,probe_serial,probe_firmware,first_seen_ms,last_seen_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)
             ON CONFLICT(uid) DO UPDATE SET idcode=excluded.idcode,dev_id=excluded.dev_id,
             flash_kb=excluded.flash_kb,option_bytes=excluded.option_bytes,probe_name=excluded.probe_name,
             probe_serial=excluded.probe_serial,probe_firmware=excluded.probe_firmware,last_seen_ms=excluded.last_seen_ms",
            params![evidence.uid,evidence.idcode,evidence.dev_id,evidence.flash_kb,evidence.option_bytes,
                evidence.probe_name,evidence.probe_serial,evidence.probe_firmware,now],
        )?;
        Ok(())
    }

    fn record_attempt(&mut self, evidence: &AttemptEvidence) -> Result<(), RepositoryError> {
        self.connection.execute(
            "INSERT INTO attempts(at_ms,serial,uid,firmware_version,bootloader_sha256,application_sha256,bundle_sha256,provenance,outcome,detail)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![now_ms(),evidence.serial,evidence.uid,evidence.firmware_version,evidence.bootloader_sha256,
                evidence.application_sha256,evidence.bundle_sha256,evidence.provenance,evidence.outcome,evidence.detail],
        )?;
        Ok(())
    }

    fn history(
        &self,
        serial: Option<u32>,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Action>, RepositoryError> {
        let needle = query.unwrap_or("");
        let mut statement = self.connection.prepare(
            "SELECT id,at_ms,serial,uid,action,outcome,detail FROM actions
             WHERE (?1 IS NULL OR serial=?1) AND (?2='' OR action LIKE '%'||?2||'%' OR detail LIKE '%'||?2||'%' OR uid LIKE '%'||?2||'%')
             ORDER BY id DESC LIMIT ?3"
        )?;
        let rows = statement.query_map(params![serial, needle, limit.min(5000) as i64], |row| {
            Ok(Action {
                id: row.get(0)?,
                at_ms: row.get(1)?,
                serial: row.get(2)?,
                uid: row.get(3)?,
                action: row.get(4)?,
                outcome: row.get(5)?,
                detail: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_is_transactional_gappy_and_resumes_by_uid() {
        let mut repo = SqliteRepository::in_memory().unwrap();
        let first = repo.reserve("UID-A", None, false).unwrap();
        assert_eq!(first.serial, 1);
        repo.mark_failed(1, "UID-A", "power lost").unwrap();
        assert_eq!(repo.reserve("UID-A", None, false).unwrap().serial, 1);
        assert_eq!(repo.reserve("UID-B", None, false).unwrap().serial, 2);
        assert_eq!(repo.next_serial().unwrap(), 3);
    }

    #[test]
    fn manual_numbers_only_advance_the_counter_and_conflicts_need_override() {
        let mut repo = SqliteRepository::in_memory().unwrap();
        repo.reserve("UID-A", Some(50), false).unwrap();
        assert_eq!(repo.next_serial().unwrap(), 51);
        assert!(matches!(
            repo.reserve("UID-B", Some(50), false),
            Err(RepositoryError::Conflict { .. })
        ));
        let moved = repo.reserve("UID-B", Some(50), true).unwrap();
        assert_eq!(moved.serial, 50);
        repo.reserve("UID-C", Some(7), false).unwrap();
        assert_eq!(repo.next_serial().unwrap(), 51);

        let same_uid = repo.reserve("UID-A", Some(51), true).unwrap();
        assert_eq!(
            same_uid.serial, 51,
            "an explicit PCB override moves the same MCU"
        );
    }

    #[test]
    fn restart_recovers_counter_above_every_association() {
        let path = std::env::temp_dir().join(format!(
            "ptb-provisioning-{}-{}.sqlite3",
            std::process::id(),
            now_ms()
        ));
        {
            let mut repo = SqliteRepository::open(&path).unwrap();
            repo.reserve("UID-A", Some(12), false).unwrap();
            repo.connection
                .execute("UPDATE meta SET value=1 WHERE key='next_serial'", [])
                .unwrap();
        }
        let repo = SqliteRepository::open(&path).unwrap();
        assert_eq!(repo.next_serial().unwrap(), 13);
        let _ = std::fs::remove_file(path);
    }
}
