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
from typing import Iterator, Optional

from .config import CACHE_DIR

# --- Constants ---

SESSION_RATE_WEIGHT = 0.6
HIST_RATE_WEIGHT = 0.4
DEFAULT_ACTIVITY_PROB = 0.3
MAX_IDLE_GAP_MIN = 30
TREND_SHORT_SEC = 600   # 10 min
TREND_LONG_SEC = 1800   # 30 min
TREND_UP_RATIO = 1.3
TREND_DOWN_RATIO = 0.7


# --- Rate computation ---

def _compute_rate(
    samples: list[tuple[float, float]],
    window_sec: float,
    min_delta: float,
    unit_sec: float,
) -> Optional[float]:
    """Compute usage rate over a time window.

    Args:
        window_sec: How far back to look from the last sample.
        min_delta: Minimum time delta (in unit_sec units) to avoid noise.
        unit_sec: Divisor for time (3600=hours, 86400=days).
    """
    if len(samples) < 2:
        return None

    now = samples[-1][0]
    cutoff = now - window_sec
    recent = [(t, p) for t, p in samples if t >= cutoff]
    if len(recent) < 2:
        return None

    delta_pct = recent[-1][1] - recent[0][1]
    delta_units = (recent[-1][0] - recent[0][0]) / unit_sec
    if delta_units < min_delta:
        return None
    return max(0.0, delta_pct / delta_units)


def rate_per_hour(samples: list[tuple[float, float]]) -> Optional[float]:
    """Current usage rate as %/hour (last 30 min window)."""
    return _compute_rate(samples, window_sec=1800, min_delta=0.02, unit_sec=3600)


def rate_per_day(samples: list[tuple[float, float]]) -> Optional[float]:
    """Current usage rate as %/day (last 24h window)."""
    return _compute_rate(samples, window_sec=86400, min_delta=0.01, unit_sec=86400)


def overall_rate(
    samples: list[tuple[float, float]],
) -> Optional[float]:
    """Simple overall %/minute including idle time.

    Better for long windows (7d) where idle time is normal and should
    not be factored out.
    """
    if len(samples) < 2:
        return None
    delta_pct = samples[-1][1] - samples[0][1]
    delta_min = (samples[-1][0] - samples[0][0]) / 60
    if delta_min < 1:
        return None
    return max(0.0, delta_pct / delta_min)


def current_session_rate(
    samples: list[tuple[float, float]],
) -> Optional[float]:
    """Compute %/minute from recent samples where usage increased.

    Skips idle gaps (>30min). Weights recent pairs higher via exponential decay.
    """
    if len(samples) < 2:
        return None

    weighted_pct = 0.0
    weighted_min = 0.0

    for i in range(1, len(samples)):
        delta_pct = samples[i][1] - samples[i - 1][1]
        delta_sec = samples[i][0] - samples[i - 1][0]
        delta_min = delta_sec / 60

        if delta_pct <= 0 or delta_min <= 0 or delta_min > MAX_IDLE_GAP_MIN:
            continue

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


# --- Shared projection helpers ---

def _blend_rate(
    session_rate: Optional[float],
    hist_rate: Optional[float],
) -> Optional[float]:
    """Blend session rate (60%) with historical median (40%).

    Falls back to whichever is available. Returns None if neither.
    """
    rates: list[tuple[float, float]] = []
    if session_rate is not None:
        rates.append((session_rate, SESSION_RATE_WEIGHT))
    if hist_rate is not None:
        rates.append((hist_rate, HIST_RATE_WEIGHT if session_rate is not None else 1.0))

    if not rates:
        return None

    total_w = sum(w for _, w in rates)
    return sum(r * w for r, w in rates) / total_w


def _walk_hours(
    start: float,
    end: float,
    hourly_profile: dict[int, float],
) -> Iterator[tuple[float, float]]:
    """Yield (minutes_in_chunk, activity_prob) for each hour-slot between start and end."""
    cursor = start
    while cursor < end:
        dt = datetime.fromtimestamp(cursor, tz=timezone.utc)
        next_hour = cursor + (3600 - dt.minute * 60 - dt.second)
        chunk_end = min(next_hour, end)
        minutes = (chunk_end - cursor) / 60
        prob = hourly_profile.get(dt.hour, DEFAULT_ACTIVITY_PROB)
        yield minutes, prob
        cursor = chunk_end


# --- Projection functions ---

def project_end_of_window(
    current_pct: float,
    resets_at: float,
    session_rate: Optional[float],
    hourly_profile: dict[int, float],
    hist_rate: Optional[float],
) -> Optional[float]:
    """Project usage % at end of window.

    Walks hour-by-hour, multiplying effective rate by P(active) per hour.
    """
    now = time.time()
    if resets_at - now <= 0:
        return current_pct

    effective_rate = _blend_rate(session_rate, hist_rate)
    if effective_rate is None:
        return None

    projected = current_pct
    for minutes, prob in _walk_hours(now, resets_at, hourly_profile):
        projected += effective_rate * minutes * prob

    return projected


def project_linear(
    current_pct: float,
    resets_at: float,
    rate: Optional[float],
) -> Optional[float]:
    """Simple linear projection: current + rate * remaining_minutes.

    No hourly profile — rate already includes idle patterns. Better for 7d.
    """
    if rate is None or rate <= 0:
        return current_pct
    now = time.time()
    remaining_min = max(0, (resets_at - now) / 60)
    return current_pct + rate * remaining_min


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
    if resets_at - now <= 0:
        return None

    effective_rate = _blend_rate(session_rate, hist_rate)
    if effective_rate is None:
        return None

    projected = current_pct
    elapsed_sec = 0.0

    for minutes, prob in _walk_hours(now, resets_at, hourly_profile):
        chunk_increase = effective_rate * minutes * prob

        if projected + chunk_increase >= threshold:
            needed = threshold - projected
            if effective_rate * prob > 0:
                mins_needed = needed / (effective_rate * prob)
                secs_from_now = elapsed_sec + mins_needed * 60
                if secs_from_now < 60:
                    return "<1m"
                total_min = int(secs_from_now / 60)
                if total_min >= 1440:
                    d = total_min // 1440
                    h = (total_min % 1440) // 60
                    return f"{d}d{h:02d}h"
                if total_min >= 60:
                    return f"{total_min // 60}h{total_min % 60:02d}m"
                return f"{total_min}m"

        projected += chunk_increase
        elapsed_sec += minutes * 60

    return None  # won't hit threshold before window resets


# --- Trend & confidence ---

def compute_trend(
    samples: list[tuple[float, float]],
    short_window: float = TREND_SHORT_SEC,
    long_window: float = TREND_LONG_SEC,
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
    if ratio > TREND_UP_RATIO:
        return "up"
    if ratio < TREND_DOWN_RATIO:
        return "down"
    return "stable"


def compute_confidence(
    n_samples: int,
    timespan_sec: float,
    has_hist_rate: bool,
    profile_hours_known: int,
) -> str:
    """Return confidence level: "low", "medium", or "high"."""
    score = 0
    if n_samples >= 20:
        score += 2
    elif n_samples >= 10:
        score += 1
    if timespan_sec >= 3600:
        score += 2
    elif timespan_sec >= 1800:
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


# --- EMA smoothing ---

_EMA_FILE = CACHE_DIR / "ema_state.json"
_EMA_ALPHA = 0.3


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
