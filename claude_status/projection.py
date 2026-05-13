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


def derate_confidence(conf: str, window_blend_weight: float) -> str:
    """Drop confidence when the projection leans on cross-window baseline.

    The in-window sample count and timespan that drove the original score
    describe observation quality, not projection quality: when the blend
    is rolling-dominant, the projection reflects historical rate more
    than the user's current trajectory, so the headline number should
    visually de-emphasise.

    No change when ``window_blend_weight >= 0.75`` (blend is window-dominant).
    Drop one level at ``>= 0.25``, two levels below that.
    """
    levels = ["low", "medium", "high"]
    if conf not in levels:
        return conf
    if window_blend_weight >= 0.75:
        return conf
    drop = 1 if window_blend_weight >= 0.25 else 2
    return levels[max(0, levels.index(conf) - drop)]


def blended_rolling_rate(
    window_rate: Optional[float],
    rolling_rate: Optional[float],
    in_window_age_sec: float,
    full_weight_sec: float = 86400,
) -> Optional[float]:
    """Blend current-window rate with a cross-window rolling baseline.

    Weight on ``window_rate`` ramps linearly from 0 at observation start
    to 1 after ``full_weight_sec`` of in-window data. Stabilises the
    7d projection at the start of a fresh window: the few minutes of
    in-window data alone extrapolate to absurd end-of-window values,
    so we lean on the rolling baseline until the current window has
    accumulated enough data to stand on its own.

    Falls back to whichever input is non-None when one is missing.
    """
    if window_rate is None:
        return rolling_rate
    if rolling_rate is None:
        return window_rate
    w_cur = min(1.0, max(0.0, in_window_age_sec / full_weight_sec))
    return w_cur * window_rate + (1.0 - w_cur) * rolling_rate


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
    projected_pct: Optional[float],
    resets_at: float,
    threshold: float = 100.0,
) -> Optional[str]:
    """Format time-to-threshold derived from the projected end-of-window value.

    Both numbers come from the same projection line, so the returned deadline
    is consistent with the displayed ``proj``: ``None`` when ``proj`` won't
    cross ``threshold`` before reset, otherwise the in-window crossing time.
    """
    if projected_pct is None or projected_pct < threshold:
        return None
    if projected_pct <= current_pct:
        return None
    remaining_min = (resets_at - time.time()) / 60
    if remaining_min <= 0:
        return None
    mins = remaining_min * (threshold - current_pct) / (projected_pct - current_pct)
    if mins <= 0:
        return None
    total_min = int(mins)
    if total_min < 1:
        return "<1m"
    if total_min >= 1440:
        d = total_min // 1440
        h = (total_min % 1440) // 60
        return f"{d}d{h:02d}h"
    if total_min >= 60:
        return f"{total_min // 60}h{total_min % 60:02d}m"
    return f"{total_min}m"


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
