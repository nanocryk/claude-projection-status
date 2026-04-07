"""SQLite-based usage history storage."""

from __future__ import annotations

import sqlite3
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

from .config import DB_PATH, RETENTION_DAYS, log

_SCHEMA = """
CREATE TABLE IF NOT EXISTS usage_samples (
    id INTEGER PRIMARY KEY,
    timestamp REAL NOT NULL,
    window_type TEXT NOT NULL,
    used_pct REAL NOT NULL,
    resets_at REAL NOT NULL,
    session_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_samples_time ON usage_samples(timestamp);
CREATE INDEX IF NOT EXISTS idx_samples_window ON usage_samples(window_type, resets_at);
CREATE INDEX IF NOT EXISTS idx_samples_session ON usage_samples(window_type, session_id);

CREATE TABLE IF NOT EXISTS active_hours (
    date TEXT NOT NULL,
    hour INTEGER NOT NULL,
    weekday INTEGER NOT NULL DEFAULT 0,
    sample_count INTEGER DEFAULT 0,
    total_delta_pct REAL DEFAULT 0.0,
    PRIMARY KEY (date, hour)
);
"""


def _migrate(conn: sqlite3.Connection) -> None:
    """Add columns missing from older schema versions."""
    cols = {row[1] for row in conn.execute("PRAGMA table_info(active_hours)").fetchall()}
    if "weekday" not in cols:
        conn.execute("ALTER TABLE active_hours ADD COLUMN weekday INTEGER NOT NULL DEFAULT 0")
        conn.commit()
        log.debug("migrated: added weekday column to active_hours")


def open_db() -> sqlite3.Connection:
    DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(DB_PATH), timeout=2)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.executescript(_SCHEMA)
    _migrate(conn)
    return conn


def record_sample(
    conn: sqlite3.Connection,
    window_type: str,
    used_pct: float,
    resets_at: float,
    session_id: str = "",
) -> None:
    """Insert a usage sample, skip if unchanged from last."""
    now = time.time()

    # Dedup: skip if pct and resets_at match last entry for this window+session
    row = conn.execute(
        "SELECT used_pct, resets_at FROM usage_samples "
        "WHERE window_type = ? AND session_id = ? ORDER BY timestamp DESC LIMIT 1",
        (window_type, session_id),
    ).fetchone()
    if row and row[0] == used_pct and row[1] == resets_at:
        return

    conn.execute(
        "INSERT INTO usage_samples (timestamp, window_type, used_pct, resets_at, session_id) "
        "VALUES (?, ?, ?, ?, ?)",
        (now, window_type, used_pct, resets_at, session_id),
    )
    log.debug("recorded %s: %.1f%% session=%s", window_type, used_pct, session_id[:8])

    # Update active_hours: compute delta from previous sample in same window
    dt = datetime.now(timezone.utc)
    date_str = dt.strftime("%Y-%m-%d")
    hour = dt.hour

    delta = 0.0
    if row is not None:
        prev_pct = row[0]
        if used_pct > prev_pct and resets_at == row[1]:
            delta = used_pct - prev_pct

    weekday = dt.weekday()  # 0=Monday, 6=Sunday

    conn.execute(
        "INSERT INTO active_hours (date, hour, weekday, sample_count, total_delta_pct) "
        "VALUES (?, ?, ?, 1, ?) "
        "ON CONFLICT(date, hour) DO UPDATE SET "
        "sample_count = sample_count + 1, "
        "total_delta_pct = total_delta_pct + ?",
        (date_str, hour, weekday, delta, delta),
    )

    conn.commit()


def prune_old(conn: sqlite3.Connection) -> None:
    cutoff = time.time() - RETENTION_DAYS * 86400
    conn.execute("DELETE FROM usage_samples WHERE timestamp < ?", (cutoff,))
    cutoff_date = datetime.fromtimestamp(cutoff, tz=timezone.utc).strftime("%Y-%m-%d")
    conn.execute("DELETE FROM active_hours WHERE date < ?", (cutoff_date,))
    conn.commit()


def get_window_samples(
    conn: sqlite3.Connection,
    window_type: str,
    resets_at: float,
) -> list[tuple[float, float]]:
    """Return [(timestamp, used_pct)] for current window.

    When multiple sessions report for the same window, we take the MAX pct
    per timestamp bucket (30-second buckets) to merge concurrent session reports.
    """
    rows = conn.execute(
        "SELECT CAST(timestamp/30 AS INTEGER)*30 as ts_bucket, MAX(used_pct) "
        "FROM usage_samples "
        "WHERE window_type = ? AND resets_at = ? "
        "GROUP BY ts_bucket ORDER BY ts_bucket",
        (window_type, resets_at),
    ).fetchall()
    return rows


def get_hourly_activity_profile(
    conn: sqlite3.Connection,
    current_weekday: Optional[int] = None,
) -> dict[int, float]:
    """Return {hour: probability_of_being_active} from recent history.

    When current_weekday is provided, same-type days (weekday vs weekend)
    get 2x weight. Activity = at least one sample with nonzero delta.
    """
    rows = conn.execute(
        "SELECT hour, weekday, "
        "SUM(CASE WHEN total_delta_pct > 0 THEN 1 ELSE 0 END) as active, "
        "COUNT(*) as total "
        "FROM active_hours GROUP BY hour, weekday",
    ).fetchall()

    # Accumulate weighted active/total per hour
    hour_active: dict[int, float] = {}
    hour_total: dict[int, float] = {}

    is_weekend_now = current_weekday is not None and current_weekday >= 5

    for hour, weekday, active, total in rows:
        is_weekend_row = weekday >= 5
        # Same day-type gets 2x weight
        weight = 2.0 if (current_weekday is not None and is_weekend_row == is_weekend_now) else 1.0
        hour_active[hour] = hour_active.get(hour, 0.0) + active * weight
        hour_total[hour] = hour_total.get(hour, 0.0) + total * weight

    profile: dict[int, float] = {}
    for hour in hour_total:
        t = hour_total[hour]
        profile[hour] = hour_active.get(hour, 0.0) / t if t > 0 else 0.0
    return profile


def get_daily_7d_deltas(
    conn: sqlite3.Connection,
) -> list[float]:
    """Return list of daily 7d-window % increases from history.

    Used for daily budget pacing: what's the typical daily consumption
    of the 7d window?
    """
    # For each day, find the max-min of 7d pct within that day
    rows = conn.execute(
        "SELECT date(timestamp, 'unixepoch') as day, MAX(used_pct) - MIN(used_pct) as delta "
        "FROM usage_samples "
        "WHERE window_type = '7d' "
        "GROUP BY day "
        "HAVING delta > 0",
    ).fetchall()
    return [r[1] for r in rows]


def is_peak_hour(
    conn: sqlite3.Connection,
    hour: int,
    current_weekday: Optional[int] = None,
) -> bool:
    """Return True if this hour is historically a high-activity hour.

    Peak = activity probability > 0.7 AND above-average delta.
    """
    profile = get_hourly_activity_profile(conn, current_weekday)
    prob = profile.get(hour, 0.0)
    if prob < 0.7:
        return False

    # Check if this hour's average delta is above the overall average
    rows = conn.execute(
        "SELECT hour, AVG(total_delta_pct) as avg_delta "
        "FROM active_hours WHERE total_delta_pct > 0 GROUP BY hour",
    ).fetchall()
    if not rows:
        return False

    hour_deltas = {r[0]: r[1] for r in rows}
    overall_avg = sum(hour_deltas.values()) / len(hour_deltas)
    return hour_deltas.get(hour, 0.0) > overall_avg * 1.2


def get_historical_rates(
    conn: sqlite3.Connection,
) -> list[float]:
    """Return list of %/minute rates from past windows (one per window)."""
    # Group samples by (window_type, resets_at) to find distinct windows
    windows = conn.execute(
        "SELECT window_type, resets_at, MIN(timestamp), MAX(timestamp), "
        "MIN(used_pct), MAX(used_pct) "
        "FROM usage_samples "
        "GROUP BY window_type, resets_at "
        "HAVING MAX(used_pct) > MIN(used_pct) AND MAX(timestamp) > MIN(timestamp)",
    ).fetchall()

    rates = []
    for _, _, t_min, t_max, pct_min, pct_max in windows:
        duration_min = (t_max - t_min) / 60
        if duration_min > 1:
            rates.append((pct_max - pct_min) / duration_min)
    return rates
