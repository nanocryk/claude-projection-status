//! Usage history and the activity profile's counters.
//!
//! Plumbing only: rows in, rows out. What the numbers mean lives in the
//! profile and estimator modules.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension as _, params};

use crate::slots::SlotId;
use crate::units::{Pct, Timestamp};
use crate::window::{Sample, WindowKind, WindowState};

/// Concurrent sessions share one database; a writer holds it briefly.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
/// Readings from different sessions inside one bucket describe the same state.
const MERGE_BUCKET_SEC: f64 = 30.0;

pub const META_HISTORY_STARTED: &str = "history_started_at";
pub const META_CLOSED_THROUGH: &str = "slots_closed_through";
pub const META_DECAYED_AT: &str = "slots_decayed_at";
pub const META_PRUNED_AT: &str = "pruned_at";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS samples (
    id INTEGER PRIMARY KEY,
    at REAL NOT NULL,
    window_kind TEXT NOT NULL,
    pct REAL NOT NULL,
    resets_at REAL NOT NULL,
    session TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_samples_window ON samples(window_kind, resets_at, at);
CREATE INDEX IF NOT EXISTS idx_samples_session ON samples(window_kind, session, at);
CREATE INDEX IF NOT EXISTS idx_samples_at ON samples(at);

CREATE TABLE IF NOT EXISTS slots (
    weekday INTEGER NOT NULL,
    hour INTEGER NOT NULL,
    occurrences REAL NOT NULL DEFAULT 0,
    active REAL NOT NULL DEFAULT 0,
    PRIMARY KEY (weekday, hour)
);

CREATE TABLE IF NOT EXISTS closed_hours (
    at REAL PRIMARY KEY,
    active REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS priors (
    window_kind TEXT PRIMARY KEY,
    lambda REAL NOT NULL,
    weight REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value REAL NOT NULL
);
";

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Db(rusqlite::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io(error) => write!(out, "history directory: {error}"),
            StoreError::Db(error) => write!(out, "history database: {error}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        StoreError::Io(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        StoreError::Db(error)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A stored reading, carrying the window instance it belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordedSample {
    pub at: Timestamp,
    pub pct: Pct,
    pub resets_at: Timestamp,
}

/// Decayed counts for one slot of the local week.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SlotCount {
    /// Times this slot has come round since history began.
    pub occurrences: f64,
    /// How many of those saw usage.
    pub active: f64,
}

/// Intensity carried over from completed windows, in percent per active hour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prior {
    pub lambda: f64,
    /// Active hours behind the estimate.
    pub weight: f64,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path, now: Timestamp) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::init(Connection::open(path)?, now)
    }

    pub fn in_memory(now: Timestamp) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, now)
    }

    fn init(conn: Connection, now: Timestamp) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // Write-ahead logging lets a reader run while another session writes.
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        conn.execute_batch(SCHEMA)?;
        let store = Store { conn };
        if store.meta(META_HISTORY_STARTED)?.is_none() {
            store.set_meta(META_HISTORY_STARTED, now.get())?;
        }
        Ok(store)
    }

    pub fn meta(&self, key: &str) -> Result<Option<f64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: f64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Store a reading, unless this session already reported the same one.
    ///
    /// Returns whether a row was written.
    pub fn record(
        &self,
        kind: WindowKind,
        state: WindowState,
        session: &str,
        now: Timestamp,
    ) -> Result<bool> {
        let last: Option<(f64, f64)> = self
            .conn
            .query_row(
                "SELECT pct, resets_at FROM samples
                 WHERE window_kind = ?1 AND session = ?2 ORDER BY at DESC LIMIT 1",
                params![kind.label(), session],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if last == Some((state.used.get(), state.resets_at.get())) {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO samples (at, window_kind, pct, resets_at, session)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                now.get(),
                kind.label(),
                state.used.get(),
                state.resets_at.get(),
                session
            ],
        )?;
        Ok(true)
    }

    /// Readings for one window instance, oldest first.
    pub fn window_samples(&self, kind: WindowKind, resets_at: Timestamp) -> Result<Vec<Sample>> {
        let mut statement = self.conn.prepare(
            "SELECT CAST(at / ?3 AS INTEGER) * ?3 AS bucket, MAX(pct) FROM samples
             WHERE window_kind = ?1 AND resets_at = ?2
             GROUP BY bucket ORDER BY bucket",
        )?;
        let rows = statement.query_map(
            params![kind.label(), resets_at.get(), MERGE_BUCKET_SEC],
            |row| {
                Ok(Sample {
                    at: Timestamp::new(row.get(0)?),
                    pct: Pct::new(row.get(1)?),
                })
            },
        )?;
        let mut samples = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        drop_stale_highs(&mut samples);
        Ok(samples)
    }

    /// Readings in a time range across window instances, oldest first.
    ///
    /// Each instance is cleaned on its own, since usage only has to rise
    /// within one window.
    pub fn samples_between(
        &self,
        kind: WindowKind,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Vec<RecordedSample>> {
        let mut statement = self.conn.prepare(
            "SELECT resets_at, CAST(at / ?4 AS INTEGER) * ?4 AS bucket, MAX(pct) FROM samples
             WHERE window_kind = ?1 AND at >= ?2 AND at < ?3
             GROUP BY resets_at, bucket ORDER BY bucket",
        )?;
        let rows = statement.query_map(
            params![kind.label(), from.get(), to.get(), MERGE_BUCKET_SEC],
            |row| {
                Ok(RecordedSample {
                    resets_at: Timestamp::new(row.get(0)?),
                    at: Timestamp::new(row.get(1)?),
                    pct: Pct::new(row.get(2)?),
                })
            },
        )?;
        let all = rows.collect::<rusqlite::Result<Vec<_>>>()?;

        let mut per_instance: BTreeMap<u64, Vec<RecordedSample>> = BTreeMap::new();
        for sample in all {
            per_instance
                .entry(sample.resets_at.get().to_bits())
                .or_default()
                .push(sample);
        }
        let mut out = Vec::new();
        for instance in per_instance.values_mut() {
            drop_stale_highs(instance);
            out.append(instance);
        }
        out.sort_by(|left, right| left.at.get().total_cmp(&right.at.get()));
        Ok(out)
    }

    /// Window instances whose reset has passed, with the usage they ended on.
    ///
    /// `after` excludes instances already accounted for.
    pub fn completed_instances(
        &self,
        kind: WindowKind,
        after: Timestamp,
        now: Timestamp,
    ) -> Result<Vec<(Timestamp, Pct)>> {
        let mut statement = self.conn.prepare(
            "SELECT resets_at, MAX(pct) FROM samples
             WHERE window_kind = ?1 AND resets_at > ?2 AND resets_at <= ?3
             GROUP BY resets_at ORDER BY resets_at",
        )?;
        let rows = statement.query_map(params![kind.label(), after.get(), now.get()], |row| {
            Ok((Timestamp::new(row.get(0)?), Pct::new(row.get(1)?)))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Most recent reading a session reported for a window.
    pub fn latest_pct(&self, kind: WindowKind, session: &str) -> Result<Option<Pct>> {
        Ok(self
            .conn
            .query_row(
                "SELECT pct FROM samples WHERE window_kind = ?1 AND session = ?2
                 ORDER BY at DESC LIMIT 1",
                params![kind.label(), session],
                |row| row.get::<_, f64>(0),
            )
            .optional()?
            .map(Pct::new))
    }

    pub fn prune(&self, before: Timestamp) -> Result<usize> {
        let removed = self
            .conn
            .execute("DELETE FROM samples WHERE at < ?1", params![before.get()])?;
        self.conn.execute(
            "DELETE FROM closed_hours WHERE at < ?1",
            params![before.get()],
        )?;
        Ok(removed)
    }

    /// Prune at most once a day, so a refresh every few seconds does not
    /// rewrite the table.
    pub fn prune_if_due(&self, now: Timestamp, retention_days: u32) -> Result<usize> {
        let last = self.meta(META_PRUNED_AT)?.unwrap_or(f64::NEG_INFINITY);
        if now.get() - last < 86400.0 {
            return Ok(0);
        }
        let removed = self.prune(Timestamp::new(
            now.get() - f64::from(retention_days) * 86400.0,
        ))?;
        self.set_meta(META_PRUNED_AT, now.get())?;
        Ok(removed)
    }

    pub fn slot_counts(&self) -> Result<BTreeMap<SlotId, SlotCount>> {
        let mut statement = self
            .conn
            .prepare("SELECT weekday, hour, occurrences, active FROM slots")?;
        let rows = statement.query_map([], |row| {
            Ok((
                SlotId::new(row.get::<_, i64>(0)? as u8, row.get::<_, i64>(1)? as u8),
                SlotCount {
                    occurrences: row.get(2)?,
                    active: row.get(3)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
    }

    /// Fold one occurrence of a slot into the profile, `active` of it spent
    /// working.
    pub fn add_slot_occurrence(&self, slot: SlotId, active: f64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO slots (weekday, hour, occurrences, active) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(weekday, hour) DO UPDATE SET
                occurrences = occurrences + 1,
                active = active + excluded.active",
            params![i64::from(slot.weekday), i64::from(slot.hour), active],
        )?;
        Ok(())
    }

    /// Remember how much of one elapsed hour was spent working.
    ///
    /// Judging an hour once, when it closes, keeps every later reader of that
    /// hour in agreement and saves rescanning the readings behind it.
    pub fn record_closed_hour(&self, at: Timestamp, active: f64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO closed_hours (at, active) VALUES (?1, ?2)
             ON CONFLICT(at) DO UPDATE SET active = excluded.active",
            params![at.get(), active],
        )?;
        Ok(())
    }

    /// Active hours across the closed hours in a range.
    pub fn active_hours_between(&self, from: Timestamp, to: Timestamp) -> Result<f64> {
        let total: Option<f64> = self.conn.query_row(
            "SELECT SUM(active) FROM closed_hours WHERE at >= ?1 AND at < ?2",
            params![from.get(), to.get()],
            |row| row.get(0),
        )?;
        Ok(total.unwrap_or(0.0))
    }

    /// Scale every counter, ageing the profile toward the present.
    pub fn decay_slots(&self, factor: f64) -> Result<()> {
        self.conn.execute(
            "UPDATE slots SET occurrences = occurrences * ?1, active = active * ?1",
            params![factor],
        )?;
        Ok(())
    }

    pub fn prior(&self, kind: WindowKind) -> Result<Option<Prior>> {
        Ok(self
            .conn
            .query_row(
                "SELECT lambda, weight FROM priors WHERE window_kind = ?1",
                params![kind.label()],
                |row| {
                    Ok(Prior {
                        lambda: row.get(0)?,
                        weight: row.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_prior(&self, kind: WindowKind, prior: Prior) -> Result<()> {
        self.conn.execute(
            "INSERT INTO priors (window_kind, lambda, weight) VALUES (?1, ?2, ?3)
             ON CONFLICT(window_kind) DO UPDATE SET
                lambda = excluded.lambda, weight = excluded.weight",
            params![kind.label(), prior.lambda, prior.weight],
        )?;
        Ok(())
    }

    pub fn summary(&self) -> Result<Summary> {
        let samples: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM samples", [], |row| row.get(0))?;
        let span: (Option<f64>, Option<f64>) =
            self.conn
                .query_row("SELECT MIN(at), MAX(at) FROM samples", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?;
        let mut statement = self.conn.prepare(
            "SELECT window_kind, COUNT(DISTINCT resets_at) FROM samples GROUP BY window_kind",
        )?;
        let windows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let priors = WindowKind::ALL
            .iter()
            .filter_map(|kind| self.prior(*kind).transpose().map(|prior| (*kind, prior)))
            .map(|(kind, prior)| prior.map(|prior| (kind, prior)))
            .collect::<Result<Vec<_>>>()?;
        Ok(Summary {
            samples: samples.max(0) as u64,
            oldest: span.0.map(Timestamp::new),
            newest: span.1.map(Timestamp::new),
            instances: windows
                .into_iter()
                .map(|(label, count)| (label, count.max(0) as u64))
                .collect(),
            slots: self.slot_counts()?,
            priors,
            history_started: self.meta(META_HISTORY_STARTED)?.map(Timestamp::new),
        })
    }
}

/// What `--summary` reports.
#[derive(Debug)]
pub struct Summary {
    pub samples: u64,
    pub oldest: Option<Timestamp>,
    pub newest: Option<Timestamp>,
    /// Window instances tracked per window kind.
    pub instances: Vec<(String, u64)>,
    pub slots: BTreeMap<SlotId, SlotCount>,
    pub priors: Vec<(WindowKind, Prior)>,
    pub history_started: Option<Timestamp>,
}

/// Usage cannot fall inside one window, so a reading above a later one is a
/// stale snapshot from before the window opened. Drop those.
fn drop_stale_highs<T: HasPct>(samples: &mut Vec<T>) {
    let mut ceiling = f64::INFINITY;
    let mut keep = vec![true; samples.len()];
    for (index, sample) in samples.iter().enumerate().rev() {
        if sample.pct_value() > ceiling {
            keep[index] = false;
        } else {
            ceiling = sample.pct_value();
        }
    }
    let mut cursor = 0;
    samples.retain(|_| {
        let kept = keep[cursor];
        cursor += 1;
        kept
    });
}

pub trait HasPct {
    fn pct_value(&self) -> f64;
}

impl HasPct for Sample {
    fn pct_value(&self) -> f64 {
        self.pct.get()
    }
}

impl HasPct for RecordedSample {
    fn pct_value(&self) -> f64 {
        self.pct.get()
    }
}

#[cfg(test)]
mod tests {
    use super::{META_PRUNED_AT, Prior, Store};
    use crate::slots::SlotId;
    use crate::units::{Pct, Timestamp};
    use crate::window::{WindowKind, WindowState};

    const NOW: f64 = 1_789_128_000.0;

    fn store() -> Store {
        Store::in_memory(Timestamp::new(NOW)).expect("in-memory store")
    }

    fn state(pct: f64, resets_at: f64) -> WindowState {
        WindowState {
            used: Pct::new(pct),
            resets_at: Timestamp::new(resets_at),
        }
    }

    #[test]
    fn a_reading_is_stored_once_per_change() {
        let store = store();
        let resets = NOW + 3600.0;
        assert!(
            store
                .record(
                    WindowKind::FiveHour,
                    state(5.0, resets),
                    "s1",
                    Timestamp::new(NOW)
                )
                .expect("record")
        );
        assert!(
            !store
                .record(
                    WindowKind::FiveHour,
                    state(5.0, resets),
                    "s1",
                    Timestamp::new(NOW + 10.0)
                )
                .expect("record")
        );
        assert!(
            store
                .record(
                    WindowKind::FiveHour,
                    state(6.0, resets),
                    "s1",
                    // Past the merge bucket, so this stays a second reading.
                    Timestamp::new(NOW + 60.0)
                )
                .expect("record")
        );
        let samples = store
            .window_samples(WindowKind::FiveHour, Timestamp::new(resets))
            .expect("samples");
        assert_eq!(samples.len(), 2);
    }

    #[test]
    fn two_sessions_reporting_together_merge_into_one_reading() {
        let store = store();
        let resets = NOW + 3600.0;
        store
            .record(
                WindowKind::FiveHour,
                state(5.0, resets),
                "s1",
                Timestamp::new(NOW),
            )
            .expect("record");
        store
            .record(
                WindowKind::FiveHour,
                state(7.0, resets),
                "s2",
                Timestamp::new(NOW + 5.0),
            )
            .expect("record");
        let samples = store
            .window_samples(WindowKind::FiveHour, Timestamp::new(resets))
            .expect("samples");
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].pct.get(), 7.0);
    }

    #[test]
    fn a_stale_snapshot_before_the_window_opened_is_dropped() {
        let store = store();
        let resets = NOW + 3600.0;
        // A cold-started session reports the previous window's 80% first.
        for (offset, pct) in [(0.0, 80.0), (60.0, 2.0), (120.0, 3.0), (180.0, 5.0)] {
            store
                .record(
                    WindowKind::FiveHour,
                    state(pct, resets),
                    "s1",
                    Timestamp::new(NOW + offset),
                )
                .expect("record");
        }
        let samples = store
            .window_samples(WindowKind::FiveHour, Timestamp::new(resets))
            .expect("samples");
        let percentages: Vec<f64> = samples.iter().map(|sample| sample.pct.get()).collect();
        assert_eq!(percentages, vec![2.0, 3.0, 5.0]);
    }

    #[test]
    fn window_instances_are_kept_apart() {
        let store = store();
        store
            .record(
                WindowKind::FiveHour,
                state(90.0, NOW + 60.0),
                "s1",
                Timestamp::new(NOW),
            )
            .expect("record");
        store
            .record(
                WindowKind::FiveHour,
                state(1.0, NOW + 18060.0),
                "s1",
                Timestamp::new(NOW + 120.0),
            )
            .expect("record");
        let fresh = store
            .window_samples(WindowKind::FiveHour, Timestamp::new(NOW + 18060.0))
            .expect("samples");
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].pct.get(), 1.0);
    }

    #[test]
    fn samples_between_spans_instances_in_time_order() {
        let store = store();
        store
            .record(
                WindowKind::FiveHour,
                state(90.0, NOW + 60.0),
                "s1",
                Timestamp::new(NOW),
            )
            .expect("record");
        store
            .record(
                WindowKind::FiveHour,
                state(1.0, NOW + 18060.0),
                "s1",
                Timestamp::new(NOW + 120.0),
            )
            .expect("record");
        let spanning = store
            .samples_between(
                WindowKind::FiveHour,
                Timestamp::new(NOW - 10.0),
                Timestamp::new(NOW + 600.0),
            )
            .expect("samples");
        assert_eq!(spanning.len(), 2);
        assert!(spanning[0].at.get() < spanning[1].at.get());
        assert_eq!(spanning[0].pct.get(), 90.0);
    }

    #[test]
    fn the_latest_reading_of_a_session_is_readable() {
        let store = store();
        let resets = NOW + 3600.0;
        store
            .record(
                WindowKind::FiveHour,
                state(5.0, resets),
                "s1",
                Timestamp::new(NOW),
            )
            .expect("record");
        store
            .record(
                WindowKind::FiveHour,
                state(9.0, resets),
                "s1",
                Timestamp::new(NOW + 60.0),
            )
            .expect("record");
        store
            .record(
                WindowKind::FiveHour,
                state(3.0, resets),
                "other",
                Timestamp::new(NOW + 70.0),
            )
            .expect("record");
        assert_eq!(
            store
                .latest_pct(WindowKind::FiveHour, "s1")
                .expect("latest")
                .map(Pct::get),
            Some(9.0)
        );
        assert_eq!(
            store
                .latest_pct(WindowKind::SevenDay, "s1")
                .expect("latest")
                .map(Pct::get),
            None
        );
    }

    #[test]
    fn pruning_drops_readings_past_retention_and_waits_a_day() {
        let store = store();
        let resets = NOW + 3600.0;
        store
            .record(
                WindowKind::FiveHour,
                state(5.0, resets),
                "s1",
                Timestamp::new(NOW - 20.0 * 86400.0),
            )
            .expect("record");
        store
            .record(
                WindowKind::FiveHour,
                state(6.0, resets),
                "s1",
                Timestamp::new(NOW),
            )
            .expect("record");

        let removed = store
            .prune_if_due(Timestamp::new(NOW), 14)
            .expect("first prune");
        assert_eq!(removed, 1);
        assert_eq!(
            store
                .prune_if_due(Timestamp::new(NOW + 60.0), 14)
                .expect("second prune"),
            0
        );
        assert!(store.meta(META_PRUNED_AT).expect("meta").is_some());
    }

    #[test]
    fn slot_counters_accumulate_and_decay() {
        let store = store();
        let slot = SlotId::new(2, 14);
        store.add_slot_occurrence(slot, 1.0).expect("bump");
        store.add_slot_occurrence(slot, 0.0).expect("bump");
        let counts = store.slot_counts().expect("counts");
        assert_eq!(counts[&slot].occurrences, 2.0);
        assert_eq!(counts[&slot].active, 1.0);

        store.decay_slots(0.5).expect("decay");
        let decayed = store.slot_counts().expect("counts");
        assert_eq!(decayed[&slot].occurrences, 1.0);
        assert_eq!(decayed[&slot].active, 0.5);
    }

    #[test]
    fn a_prior_round_trips_per_window() {
        let store = store();
        assert_eq!(store.prior(WindowKind::SevenDay).expect("prior"), None);
        store
            .set_prior(
                WindowKind::SevenDay,
                Prior {
                    lambda: 1.4,
                    weight: 22.0,
                },
            )
            .expect("set prior");
        store
            .set_prior(
                WindowKind::SevenDay,
                Prior {
                    lambda: 1.6,
                    weight: 30.0,
                },
            )
            .expect("set prior");
        assert_eq!(
            store.prior(WindowKind::SevenDay).expect("prior"),
            Some(Prior {
                lambda: 1.6,
                weight: 30.0
            })
        );
        assert_eq!(store.prior(WindowKind::FiveHour).expect("prior"), None);
    }

    #[test]
    fn summary_counts_what_is_stored() {
        let store = store();
        store
            .record(
                WindowKind::FiveHour,
                state(5.0, NOW + 60.0),
                "s1",
                Timestamp::new(NOW),
            )
            .expect("record");
        store
            .record(
                WindowKind::SevenDay,
                state(2.0, NOW + 86400.0),
                "s1",
                Timestamp::new(NOW),
            )
            .expect("record");
        let summary = store.summary().expect("summary");
        assert_eq!(summary.samples, 2);
        assert_eq!(summary.instances.len(), 2);
        assert_eq!(summary.oldest.map(Timestamp::get), Some(NOW));
        assert!(summary.history_started.is_some());
    }
}
