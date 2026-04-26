"""Threshold checks against the usage_samples table.

Lives next to storage.py so the SQL stays adjacent to the schema. Callers
(e.g. Stop hooks that decide when to force a handoff) should go through
this module instead of querying the DB directly.
"""

from __future__ import annotations

import sqlite3
from typing import Optional


def latest_used_pct(
    conn: sqlite3.Connection,
    window_type: str,
    session_id: str,
) -> Optional[float]:
    """Return the most recent used_pct for (window_type, session_id), or None."""
    row = conn.execute(
        "SELECT used_pct FROM usage_samples "
        "WHERE window_type = ? AND session_id = ? "
        "ORDER BY timestamp DESC LIMIT 1",
        (window_type, session_id),
    ).fetchone()
    return float(row[0]) if row else None
