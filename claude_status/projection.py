"""Usage projection algorithm.

Predicts end-of-window usage % by combining:
1. Current session rate (%/min during active use)
2. Hourly activity profile (P(active) per hour from history)
3. Historical median rate (fallback baseline)
"""

from __future__ import annotations

import json
import statistics
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

from .config import CACHE_DIR


def current_session_rate(
    samples: list[tuple[float, float]],
) -> Optional[float]:
    """Compute %/minute from recent samples where usage increased.

    samples: [(timestamp, used_pct)] sorted by timestamp.
    Returns None if insufficient data.
    """
    if len(samples) < 2:
        return None

    weighted_pct = 0.0
    weighted_min = 0.0

    for i in range(1, len(samples)):
        delta_pct = samples[i][1] - samples[i - 1][1]
        delta_sec = samples[i][0] - samples[i - 1][0]
        delta_min = delta_sec / 60

        # Skip idle gaps (>30min between samples) and non-increasing
        if delta_pct <= 0 or delta_min <= 0 or delta_min > 30:
            continue

        # Exponential recency weight: more recent pairs weighted higher
        # Weight = 2^(position/total) so last pair has ~2x weight of first
        weight = 2 ** (i / len(samples))
        weighted_pct += delta_pct * weight
        weighted_min += delta_min * weight

    if weighted_min <= 0:
        return None
    return weighted_pct / weighted_min


def historical_median_rate(rates: list[float]) -> Optional[float]:
    """Median %/minute from past windows."""
    if not rates:
        return None
    return statistics.median(rates)


def project_end_of_window(
    current_pct: float,
    resets_at: float,
    session_rate: Optional[float],
    hourly_profile: dict[int, float],
    hist_rate: Optional[float],
) -> Optional[float]:
    """Project usage % at end of window.

    Walks hour-by-hour, multiplying effective rate by P(active) per hour.
    Returns None if no rate data available.
    """
    now = time.time()
    remaining = resets_at - now
    if remaining <= 0:
        return current_pct

    # Determine effective rate with weights
    rates_with_weights: list[tuple[float, float]] = []
    if session_rate is not None:
        # More weight if we have decent session data
        rates_with_weights.append((session_rate, 0.6))
    if hist_rate is not None:
        rates_with_weights.append((hist_rate, 0.4 if session_rate is not None else 1.0))

    if not rates_with_weights:
        return None

    total_weight = sum(w for _, w in rates_with_weights)
    effective_rate = sum(r * w for r, w in rates_with_weights) / total_weight

    # Walk hour by hour from now to resets_at
    projected = current_pct
    cursor = now

    while cursor < resets_at:
        dt = datetime.fromtimestamp(cursor, tz=timezone.utc)
        hour = dt.hour

        # Next hour boundary
        next_hour = cursor + (3600 - dt.minute * 60 - dt.second)
        chunk_end = min(next_hour, resets_at)
        minutes_in_chunk = (chunk_end - cursor) / 60

        activity_prob = hourly_profile.get(hour, 0.3)  # default 30% if unknown

        projected += effective_rate * minutes_in_chunk * activity_prob
        cursor = chunk_end

    return min(projected, 110.0)  # cap slightly above 100 for display


def time_to_threshold(
    current_pct: float,
    resets_at: float,
    session_rate: Optional[float],
    hourly_profile: dict[int, float],
    hist_rate: Optional[float],
    threshold: float = 100.0,
) -> Optional[str]:
    """Estimate time until usage hits threshold. Returns formatted string or None."""
    now = time.time()
    remaining = resets_at - now
    if remaining <= 0:
        return None

    rates_with_weights: list[tuple[float, float]] = []
    if session_rate is not None:
        rates_with_weights.append((session_rate, 0.6))
    if hist_rate is not None:
        rates_with_weights.append((hist_rate, 0.4 if session_rate is not None else 1.0))

    if not rates_with_weights:
        return None

    total_weight = sum(w for _, w in rates_with_weights)
    effective_rate = sum(r * w for r, w in rates_with_weights) / total_weight

    # Walk hour by hour, track when we cross threshold
    projected = current_pct
    cursor = now

    while cursor < resets_at:
        dt = datetime.fromtimestamp(cursor, tz=timezone.utc)
        hour = dt.hour
        next_hour = cursor + (3600 - dt.minute * 60 - dt.second)
        chunk_end = min(next_hour, resets_at)
        minutes_in_chunk = (chunk_end - cursor) / 60

        activity_prob = hourly_profile.get(hour, 0.3)
        chunk_increase = effective_rate * minutes_in_chunk * activity_prob

        if projected + chunk_increase >= threshold:
            # Interpolate within this chunk
            needed = threshold - projected
            if effective_rate * activity_prob > 0:
                mins_needed = needed / (effective_rate * activity_prob)
                hit_time = cursor + mins_needed * 60
                secs_from_now = hit_time - now
                if secs_from_now < 60:
                    return "<1m"
                mins = int(secs_from_now / 60)
                if mins >= 60:
                    return f"{mins // 60}h{mins % 60:02d}m"
                return f"{mins}m"

        projected += chunk_increase
        cursor = chunk_end

    return None  # won't hit threshold before window resets


def compute_trend(
    samples: list[tuple[float, float]],
    short_window: float = 600,   # 10 min
    long_window: float = 1800,   # 30 min
) -> Optional[str]:
    """Compare recent rate vs longer-term rate to detect acceleration.

    Returns "up", "down", "stable", or None if not enough data.
    """
    if len(samples) < 3:
        return None

    now = samples[-1][0]

    def _rate_in_window(window_sec: float) -> Optional[float]:
        cutoff = now - window_sec
        window = [(t, p) for t, p in samples if t >= cutoff]
        if len(window) < 2:
            return None
        delta_pct = window[-1][1] - window[0][1]
        delta_min = (window[-1][0] - window[0][0]) / 60
        if delta_min < 1:
            return None
        return delta_pct / delta_min

    short_rate = _rate_in_window(short_window)
    long_rate = _rate_in_window(long_window)

    if short_rate is None or long_rate is None:
        return None
    if long_rate <= 0:
        return "stable" if short_rate <= 0 else "up"

    ratio = short_rate / long_rate
    if ratio > 1.3:
        return "up"
    if ratio < 0.7:
        return "down"
    return "stable"


def compute_confidence(
    n_samples: int,
    timespan_sec: float,
    has_hist_rate: bool,
    profile_hours_known: int,
) -> str:
    """Return confidence level: "low", "medium", or "high".

    Based on how much data the projection has to work with.
    """
    score = 0
    if n_samples >= 20:
        score += 2
    elif n_samples >= 10:
        score += 1
    if timespan_sec >= 3600:  # 1h+
        score += 2
    elif timespan_sec >= 1800:  # 30min+
        score += 1
    if has_hist_rate:
        score += 1
    if profile_hours_known >= 12:
        score += 1

    if score >= 5:
        return "high"
    if score >= 3:
        return "medium"
    return "low"


_EMA_FILE = CACHE_DIR / "ema_state.json"
_EMA_ALPHA = 0.3  # smoothing factor: 0=fully smooth, 1=no smoothing


def smooth_projection(
    window_type: str,
    raw_value: float,
) -> float:
    """Apply EMA smoothing to reduce projection jitter across refreshes."""
    state: dict[str, float] = {}
    try:
        if _EMA_FILE.exists():
            state = json.loads(_EMA_FILE.read_text())
    except (json.JSONDecodeError, OSError):
        pass

    key = f"proj_{window_type}"
    prev = state.get(key)

    if prev is None:
        smoothed = raw_value
    else:
        smoothed = _EMA_ALPHA * raw_value + (1 - _EMA_ALPHA) * prev

    state[key] = smoothed
    try:
        _EMA_FILE.parent.mkdir(parents=True, exist_ok=True)
        _EMA_FILE.write_text(json.dumps(state))
    except OSError:
        pass

    return smoothed
