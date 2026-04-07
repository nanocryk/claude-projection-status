"""Tests for the projection algorithm."""

import time
import unittest

from claude_status.projection import (
    compute_confidence,
    compute_trend,
    current_session_rate,
    historical_median_rate,
    project_end_of_window,
    rate_per_day,
    rate_per_hour,
    smooth_projection,
    time_to_threshold,
)


class TestCurrentSessionRate(unittest.TestCase):
    def test_empty(self):
        self.assertIsNone(current_session_rate([]))

    def test_single_sample(self):
        self.assertIsNone(current_session_rate([(100.0, 10.0)]))

    def test_steady_increase(self):
        # 1%/min over 5 samples, 1 min apart
        now = time.time()
        samples = [(now + i * 60, 10.0 + i) for i in range(6)]
        rate = current_session_rate(samples)
        self.assertIsNotNone(rate)
        self.assertAlmostEqual(rate, 1.0, delta=0.1)

    def test_skips_idle_gaps(self):
        now = time.time()
        # Active for 3 min, then 2h gap, then active again
        samples = [
            (now, 10.0),
            (now + 60, 11.0),
            (now + 120, 12.0),
            (now + 7320, 12.0),  # 2h later, same pct (idle)
            (now + 7380, 13.0),
            (now + 7440, 14.0),
        ]
        rate = current_session_rate(samples)
        self.assertIsNotNone(rate)
        # Should be ~1%/min, not diluted by the 2h gap
        self.assertAlmostEqual(rate, 1.0, delta=0.3)

    def test_no_increase(self):
        now = time.time()
        samples = [(now + i * 60, 50.0) for i in range(5)]
        self.assertIsNone(current_session_rate(samples))


class TestProjectEndOfWindow(unittest.TestCase):
    def test_no_rates(self):
        self.assertIsNone(project_end_of_window(50.0, time.time() + 3600, None, {}, None))

    def test_basic_projection(self):
        # 50% now, 1h remaining, rate=0.1%/min, all hours 100% active
        resets = time.time() + 3600
        profile = {h: 1.0 for h in range(24)}
        proj = project_end_of_window(50.0, resets, 0.1, profile, None)
        # Expected: 50 + 0.1 * 60 * 1.0 = 56%
        self.assertIsNotNone(proj)
        self.assertAlmostEqual(proj, 56.0, delta=1.0)

    def test_inactive_hours_reduce_projection(self):
        # Same setup but 0% active probability
        resets = time.time() + 3600
        profile = {h: 0.0 for h in range(24)}
        proj = project_end_of_window(50.0, resets, 0.5, profile, None)
        # No activity = no increase
        self.assertIsNotNone(proj)
        self.assertAlmostEqual(proj, 50.0, delta=0.1)

    def test_caps_at_110(self):
        resets = time.time() + 7200
        profile = {h: 1.0 for h in range(24)}
        proj = project_end_of_window(90.0, resets, 1.0, profile, None)
        self.assertLessEqual(proj, 110.0)

    def test_expired_window(self):
        proj = project_end_of_window(80.0, time.time() - 10, 0.5, {}, None)
        self.assertEqual(proj, 80.0)


class TestTimeToThreshold(unittest.TestCase):
    def test_wont_reach(self):
        resets = time.time() + 3600
        profile = {h: 1.0 for h in range(24)}
        # 10% now, rate=0.01%/min, won't reach 100 in 1h
        result = time_to_threshold(10.0, resets, 0.01, profile, None)
        self.assertIsNone(result)

    def test_will_reach(self):
        resets = time.time() + 7200
        profile = {h: 1.0 for h in range(24)}
        # 80% now, rate=0.5%/min, will reach 100 in ~40min
        result = time_to_threshold(80.0, resets, 0.5, profile, None)
        self.assertIsNotNone(result)
        self.assertIn("m", result)


class TestComputeTrend(unittest.TestCase):
    def test_not_enough_data(self):
        self.assertIsNone(compute_trend([(1.0, 1.0), (2.0, 2.0)]))

    def test_stable(self):
        now = time.time()
        # Steady 1%/min over 30min
        samples = [(now - 1800 + i * 60, 10.0 + i * 0.5) for i in range(31)]
        trend = compute_trend(samples)
        self.assertEqual(trend, "stable")

    def test_accelerating(self):
        now = time.time()
        # Slow for first 20min, then fast for last 10min
        samples = []
        for i in range(20):
            samples.append((now - 1800 + i * 60, 10.0 + i * 0.1))
        for i in range(10):
            samples.append((now - 600 + i * 60, 12.0 + i * 2.0))
        trend = compute_trend(samples)
        self.assertEqual(trend, "up")


class TestComputeConfidence(unittest.TestCase):
    def test_low(self):
        self.assertEqual(compute_confidence(3, 300, False, 2), "low")

    def test_medium(self):
        self.assertEqual(compute_confidence(12, 2000, True, 8), "medium")

    def test_high(self):
        self.assertEqual(compute_confidence(25, 7200, True, 15), "high")


class TestHistoricalMedianRate(unittest.TestCase):
    def test_empty(self):
        self.assertIsNone(historical_median_rate([]))

    def test_median(self):
        self.assertEqual(historical_median_rate([1.0, 3.0, 5.0]), 3.0)


class TestRatePerHour(unittest.TestCase):
    def test_empty(self):
        self.assertIsNone(rate_per_hour([]))

    def test_single_sample(self):
        self.assertIsNone(rate_per_hour([(100.0, 10.0)]))

    def test_steady_increase(self):
        now = time.time()
        # 10% increase over 30 min = 20%/h
        samples = [(now - 1800 + i * 60, 10.0 + i * (10.0 / 30)) for i in range(31)]
        rate = rate_per_hour(samples)
        self.assertIsNotNone(rate)
        self.assertAlmostEqual(rate, 20.0, delta=1.0)

    def test_negative_clamped_to_zero(self):
        now = time.time()
        # Decreasing usage (window reset)
        samples = [(now - 600 + i * 60, 50.0 - i * 2) for i in range(11)]
        rate = rate_per_hour(samples)
        self.assertEqual(rate, 0.0)

    def test_too_short_span(self):
        now = time.time()
        samples = [(now - 30, 10.0), (now, 11.0)]  # 30s, below 1min threshold
        self.assertIsNone(rate_per_hour(samples))


class TestRatePerDay(unittest.TestCase):
    def test_empty(self):
        self.assertIsNone(rate_per_day([]))

    def test_basic_rate(self):
        now = time.time()
        # 5% increase over 2 hours = 60%/day
        samples = [(now - 7200 + i * 120, 30.0 + i * (5.0 / 60)) for i in range(61)]
        rate = rate_per_day(samples)
        self.assertIsNotNone(rate)
        self.assertAlmostEqual(rate, 60.0, delta=2.0)

    def test_negative_clamped_to_zero(self):
        now = time.time()
        samples = [(now - 3600 + i * 60, 80.0 - i) for i in range(61)]
        rate = rate_per_day(samples)
        self.assertEqual(rate, 0.0)


class TestSmoothProjection(unittest.TestCase):
    def test_first_call_returns_raw(self):
        # Clear any existing state
        from claude_status.projection import _EMA_FILE
        if _EMA_FILE.exists():
            _EMA_FILE.unlink()
        result = smooth_projection("test_window", 50.0)
        self.assertEqual(result, 50.0)

    def test_subsequent_call_smooths(self):
        from claude_status.projection import _EMA_FILE
        if _EMA_FILE.exists():
            _EMA_FILE.unlink()
        smooth_projection("test_smooth", 50.0)
        result = smooth_projection("test_smooth", 70.0)
        # EMA: 0.3 * 70 + 0.7 * 50 = 56
        self.assertAlmostEqual(result, 56.0, delta=0.1)


if __name__ == "__main__":
    unittest.main()
