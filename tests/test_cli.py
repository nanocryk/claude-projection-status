"""Integration tests for cli._project_7d covering the with/without-history fix."""

import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from claude_status import cli, projection, storage


class TestProject7d(unittest.TestCase):
    """Verify that fresh 7d windows lean on rolling history when available."""

    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.db_path = Path(self.tmpdir) / "test.db"
        self.ema_path = Path(self.tmpdir) / "ema.json"
        self.patchers = [
            mock.patch.object(storage, "DB_PATH", self.db_path),
            mock.patch.object(projection, "_EMA_FILE", self.ema_path),
        ]
        for p in self.patchers:
            p.start()
        self.conn = storage.open_db()

    def tearDown(self):
        self.conn.close()
        for p in self.patchers:
            p.stop()

    def _insert(self, ts, pct, resets, session="s1", wtype="7d"):
        self.conn.execute(
            "INSERT INTO usage_samples (timestamp, window_type, used_pct, resets_at, session_id) "
            "VALUES (?, ?, ?, ?, ?)",
            (ts, wtype, pct, resets, session),
        )

    def test_fresh_window_no_history_falls_back_to_window_rate(self):
        """Without prior windows, behavior matches the pre-fix path."""
        now = time.time()
        resets = now + 7 * 86400 - 1800  # ~7d remaining
        # 6 samples over 30 min, 1% -> 2% (= 1/30 %/min)
        for i in range(6):
            self._insert(now - 1800 + i * 360, 1.0 + i * 0.2, resets)
        self.conn.commit()

        r = cli._project_7d(self.conn, 2.0, resets, hourly_profile={})

        self.assertIsNotNone(r["proj"])
        # No rolling baseline available -> blended rate == window rate.
        # Window rate ~0.033%/min * (7d-30min in min) ~= 332% added.
        # So projection should be in the hundreds — the bug scenario, but
        # mathematically unavoidable with zero history.
        self.assertGreater(r["proj"], 200.0)

    def test_fresh_window_with_history_uses_rolling_baseline(self):
        """With prior 7d windows, the rolling rate dominates a fresh window."""
        now = time.time()
        current_resets = now + 7 * 86400 - 1800
        prior_resets = now - 2 * 86400  # finished 2 days ago

        # Prior 7d window: 5 days of observation, ended at ~20%.
        # Rate = 20% / (5*1440 min) = ~0.00278 %/min
        prior_start = prior_resets - 5 * 86400
        for i in range(11):
            ts = prior_start + i * (5 * 86400 / 10)
            self._insert(ts, i * 2.0, prior_resets)  # 0%, 2%, 4%, ..., 20%

        # Current window: same burst as the no-history test (0.033%/min)
        for i in range(6):
            self._insert(now - 1800 + i * 360, 1.0 + i * 0.2, current_resets)
        self.conn.commit()

        r = cli._project_7d(self.conn, 2.0, current_resets, hourly_profile={})

        self.assertIsNotNone(r["proj"])
        # Rolling rate ~0.00278 %/min * ~7d remaining = ~28% added; with
        # tiny current-window weight (~30min/24h = ~2%), blend stays close
        # to rolling. Projection should be far below the no-history blow-up.
        self.assertLess(r["proj"], 50.0)
        # And clearly above the starting pct (rate is positive).
        self.assertGreater(r["proj"], 2.0)

    def test_fresh_window_with_history_derates_confidence(self):
        """A rolling-dominant blend reports lower confidence than the raw score."""
        now = time.time()
        current_resets = now + 7 * 86400 - 1800
        prior_resets = now - 2 * 86400

        # Enough prior data for a confident raw score: 11 samples over 5 days.
        prior_start = prior_resets - 5 * 86400
        for i in range(11):
            self._insert(prior_start + i * (5 * 86400 / 10), i * 2.0, prior_resets)

        # Current window: 30 min of data (rolling-dominant blend, w_cur ~0.02).
        for i in range(6):
            self._insert(now - 1800 + i * 360, 1.0 + i * 0.2, current_resets)
        self.conn.commit()

        r = cli._project_7d(self.conn, 2.0, current_resets, hourly_profile={})
        # Without deration the raw score may land medium/high; with rolling
        # weight near zero we expect the floor.
        self.assertEqual(r["conf"], "low")

    def test_mature_window_unchanged_by_blend(self):
        """After 24h of in-window data, the rolling rate has no influence."""
        now = time.time()
        current_resets = now + 6 * 86400  # 1 day into a 7d window
        prior_resets = now - 2 * 86400

        # Prior window with a very different rate to make any blending visible.
        prior_start = prior_resets - 5 * 86400
        for i in range(11):
            self._insert(prior_start + i * (5 * 86400 / 10), i * 5.0, prior_resets)

        # Current window: 24h of observation, 0% -> 5% (steady ~0.0035%/min).
        for i in range(25):
            self._insert(now - 86400 + i * 3600, i * (5.0 / 24), current_resets)
        self.conn.commit()

        r_with_prior = cli._project_7d(self.conn, 5.0, current_resets, hourly_profile={})

        # Drop the prior window and re-project to compare.
        self.conn.execute("DELETE FROM usage_samples WHERE resets_at = ?", (prior_resets,))
        self.conn.commit()
        # Reset EMA so smoothing doesn't carry the prior projection over.
        self.ema_path.unlink(missing_ok=True)
        r_no_prior = cli._project_7d(self.conn, 5.0, current_resets, hourly_profile={})

        self.assertIsNotNone(r_with_prior["proj"])
        self.assertIsNotNone(r_no_prior["proj"])
        self.assertAlmostEqual(r_with_prior["proj"], r_no_prior["proj"], delta=0.5)


if __name__ == "__main__":
    unittest.main()
