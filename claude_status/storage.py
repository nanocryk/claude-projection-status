"""SQLite-based usage history storage."""

from __future__ import annotations

import sqlite3
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

from .config import DB_PATH, RETENTION_DAYS

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

CREATE TABLE IF NOT EXISTS active_hours (
    date TEXT NOT NULL,
    hour INTEGER NOT NULL,
    sample_count INTEGER DEFAULT 0,
    total_delta_pct REAL DEFAULT 0.0,
    PRIMARY KEY (date, hour)
);
"""


def open_db() -> sqlite3.Connection:
    DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(DB_PATH), timeout=2)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.executescript(_SCHEMA)
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

    # Dedup: skip if pct and resets_at match last entry for this window
    row = conn.execute(
        "SELECT used_pct, resets_at FROM usage_samples "
        "WHERE window_type = ? ORDER BY timestamp DESC LIMIT 1",
        (window_type,),
    ).fetchone()
    if row and row[0] == used_pct and row[1] == resets_at:
        return

    conn.execute(
        "INSERT INTO usage_samples (timestamp, window_type, used_pct, resets_at, session_id) "
        "VALUES (?, ?, ?, ?, ?)",
        (now, window_type, used_pct, resets_at, session_id),
    )

    # Update active_hours: compute delta from previous sample in same window
    dt = datetime.now(timezone.utc)
    date_str = dt.strftime("%Y-%m-%d")
    hour = dt.hour

    delta = 0.0
    if row is not None:
        prev_pct = row[0]
        if used_pct > prev_pct and resets_at == row[1]:
            delta = used_pct - prev_pct

    conn.execute(
        "INSERT INTO active_hours (date, hour, sample_count, total_delta_pct) "
        "VALUES (?, ?, 1, ?) "
        "ON CONFLICT(date, hour) DO UPDATE SET "
        "sample_count = sample_count + 1, "
        "total_delta_pct = total_delta_pct + ?",
        (date_str, hour, delta, delta),
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
    """Return [(timestamp, used_pct)] for current window."""
    rows = conn.execute(
        "SELECT timestamp, used_pct FROM usage_samples "
        "WHERE window_type = ? AND resets_at = ? ORDER BY timestamp",
        (window_type, resets_at),
    ).fetchall()
    return rows


def get_hourly_activity_profile(
    conn: sqlite3.Connection,
) -> dict[int, float]:
    """Return {hour: probability_of_being_active} from recent history.

    Activity is defined as: at least one sample recorded in that hour
    with nonzero delta. Probability = active_days / total_days.
    """
    rows = conn.execute(
        "SELECT hour, COUNT(*) as days, SUM(CASE WHEN total_delta_pct > 0 THEN 1 ELSE 0 END) as active_days "
        "FROM active_hours GROUP BY hour",
    ).fetchall()

    profile: dict[int, float] = {}
    for hour, days, active_days in rows:
        profile[hour] = active_days / days if days > 0 else 0.0
    return profile


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
