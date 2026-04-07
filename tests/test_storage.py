"""Tests for storage layer."""

import sqlite3
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from claude_status import storage


class TestStorage(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.db_path = Path(self.tmpdir) / "test.db"
        self.patcher = mock.patch.object(storage, "DB_PATH", self.db_path)
        self.patcher.start()
        self.conn = storage.open_db()

    def tearDown(self):
        self.conn.close()
        self.patcher.stop()
        if self.db_path.exists():
            self.db_path.unlink()

    def test_record_and_dedup(self):
        storage.record_sample(self.conn, "5h", 10.0, 9999.0, "sess1")
        storage.record_sample(self.conn, "5h", 10.0, 9999.0, "sess1")  # dedup
        rows = self.conn.execute("SELECT COUNT(*) FROM usage_samples").fetchone()[0]
        self.assertEqual(rows, 1)

    def test_different_sessions_not_deduped(self):
        storage.record_sample(self.conn, "5h", 10.0, 9999.0, "sess1")
        storage.record_sample(self.conn, "5h", 10.0, 9999.0, "sess2")
        rows = self.conn.execute("SELECT COUNT(*) FROM usage_samples").fetchone()[0]
        self.assertEqual(rows, 2)

    def test_get_window_samples(self):
        storage.record_sample(self.conn, "5h", 10.0, 1000.0, "s1")
        storage.record_sample(self.conn, "5h", 15.0, 1000.0, "s1")
        storage.record_sample(self.conn, "5h", 20.0, 2000.0, "s1")  # different window
        samples = storage.get_window_samples(self.conn, "5h", 1000.0)
        self.assertEqual(len(samples), 2)

    def test_hourly_activity_profile(self):
        storage.record_sample(self.conn, "5h", 10.0, 9999.0, "s1")
        storage.record_sample(self.conn, "5h", 15.0, 9999.0, "s1")
        profile = storage.get_hourly_activity_profile(self.conn)
        self.assertGreater(len(profile), 0)

    def test_prune(self):
        # Insert old sample manually
        old_ts = time.time() - 20 * 86400
        self.conn.execute(
            "INSERT INTO usage_samples (timestamp, window_type, used_pct, resets_at, session_id) "
            "VALUES (?, ?, ?, ?, ?)",
            (old_ts, "5h", 50.0, old_ts + 18000, "old"),
        )
        self.conn.commit()
        storage.record_sample(self.conn, "5h", 10.0, 9999.0, "new")
        storage.prune_old(self.conn)
        rows = self.conn.execute("SELECT COUNT(*) FROM usage_samples").fetchone()[0]
        self.assertEqual(rows, 1)  # only the new one remains

    def test_migration_adds_weekday(self):
        # Verify weekday column exists after open_db
        cols = {row[1] for row in self.conn.execute("PRAGMA table_info(active_hours)").fetchall()}
        self.assertIn("weekday", cols)


if __name__ == "__main__":
    unittest.main()
