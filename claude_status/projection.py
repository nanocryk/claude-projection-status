"""Usage projection algorithm.

Predicts end-of-window usage % by combining:
1. Current session rate (%/min during active use)
2. Hourly activity profile (P(active) per hour from history)
3. Historical median rate (fallback baseline)
"""

from __future__ import annotations

import statistics
import time
from datetime import datetime, timezone
from typing import Optional


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
