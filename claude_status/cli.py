"""Entry point: parse stdin from Claude Code, record, project, render."""

from __future__ import annotations

import json
import os
import random
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from .config import MIN_SAMPLES_FOR_PROJECTION, MIN_TIMESPAN_FOR_PROJECTION, log
from .projection import (
    compute_confidence,
    compute_trend,
    current_session_rate,
    historical_median_rate,
    overall_rate,
    project_end_of_window,
    project_linear,
    rate_per_day,
    rate_per_hour,
    smooth_projection,
    time_to_threshold,
)
from .render import render_status_line
from .storage import (
    get_historical_rates,
    get_hourly_activity_profile,
    get_window_samples,
    is_peak_hour,
    open_db,
    prune_old,
    record_sample,
)


def _parse_stdin() -> dict[str, Any]:
    """Read JSON from Claude Code stdin."""
    try:
        if sys.stdin.isatty():
            return {}
        raw = sys.stdin.read()
        if not raw:
            return {}
        return json.loads(raw)
    except (json.JSONDecodeError, TypeError):
        return {}


def _format_cooldown_5h(resets_at: Optional[float]) -> str:
    if resets_at is None:
        return "--"
    now = datetime.now(timezone.utc)
    diff = datetime.fromtimestamp(resets_at, tz=timezone.utc) - now
    total_min = max(0, int(diff.total_seconds() / 60))
    h, m = total_min // 60, total_min % 60
    return f"{h}h{m:02d}m" if h > 0 else f"{m}m"


def _format_cooldown_7d(resets_at: Optional[float]) -> str:
    if resets_at is None:
        return "--"
    now = datetime.now(timezone.utc)
    diff = datetime.fromtimestamp(resets_at, tz=timezone.utc) - now
    total_sec = max(0, int(diff.total_seconds()))
    d = total_sec // 86400
    h = (total_sec % 86400) // 3600
    m = (total_sec % 3600) // 60
    if d > 0:
        return f"{d}d{h:02d}h"
    if h > 0:
        return f"{h}h{m:02d}m"
    return f"{m}m"


def _is_bypass() -> bool:
    env = os.environ.get("CLAUDE_SKIP_PERMISSIONS", "").lower()
    if env in ("1", "true", "yes"):
        return True
    try:
        settings = json.loads(
            (Path.home() / ".claude" / "settings.json").read_text()
        )
        return settings.get("defaultMode") == "bypassPermissions"
    except Exception:
        return False


def _print_summary() -> None:
    """Print usage statistics from the database."""
    db = open_db()

    # Total samples
    total = db.execute("SELECT COUNT(*) FROM usage_samples").fetchone()[0]
    oldest = db.execute("SELECT MIN(timestamp) FROM usage_samples").fetchone()[0]
    newest = db.execute("SELECT MAX(timestamp) FROM usage_samples").fetchone()[0]

    print(f"=== Claude Status Summary ===")
    print(f"Total samples: {total}")
    if oldest and newest:
        from_dt = datetime.fromtimestamp(oldest, tz=timezone.utc).strftime("%Y-%m-%d %H:%M")
        to_dt = datetime.fromtimestamp(newest, tz=timezone.utc).strftime("%Y-%m-%d %H:%M")
        print(f"Date range: {from_dt} — {to_dt}")

    # Distinct windows
    windows = db.execute(
        "SELECT window_type, COUNT(DISTINCT resets_at) FROM usage_samples GROUP BY window_type"
    ).fetchall()
    for wt, cnt in windows:
        print(f"  {wt} windows tracked: {cnt}")

    # Hourly activity profile
    weekday = datetime.now(timezone.utc).weekday()
    profile = get_hourly_activity_profile(db, current_weekday=weekday)
    if profile:
        print(f"\nHourly activity (P(active)):")
        for h in range(24):
            p = profile.get(h, 0.0)
            bar = "#" * int(p * 20)
            print(f"  {h:02d}:00  {bar:<20s} {p:.0%}")

    # Historical rates
    rates = get_historical_rates(db)
    med = historical_median_rate(rates)
    if med is not None:
        print(f"\nHistorical median rate: {med:.3f}%/min")
        print(f"  ({len(rates)} window(s) analyzed)")

    # Peak usage windows
    peaks = db.execute(
        "SELECT window_type, resets_at, MAX(used_pct) FROM usage_samples "
        "GROUP BY window_type, resets_at ORDER BY MAX(used_pct) DESC LIMIT 5"
    ).fetchall()
    if peaks:
        print(f"\nTop usage windows:")
        for wt, ra, peak in peaks:
            reset_dt = datetime.fromtimestamp(ra, tz=timezone.utc).strftime("%Y-%m-%d %H:%M")
            print(f"  {wt} reset {reset_dt}: peak {peak:.0f}%")

    db.close()


def main() -> None:
    if "--summary" in sys.argv:
        _print_summary()
        return

    data = _parse_stdin()

    rl = data.get("rate_limits", {})
    fh = rl.get("five_hour", {})
    sd = rl.get("seven_day", {})

    pct_5h: Optional[float] = fh.get("used_percentage") if fh else None
    pct_7d: Optional[float] = sd.get("used_percentage") if sd else None
    resets_5h: Optional[float] = fh.get("resets_at") if fh else None
    resets_7d: Optional[float] = sd.get("resets_at") if sd else None

    session_id = data.get("session_id", "")

    # Model info
    model_obj = data.get("model", {})
    model_name = (
        model_obj.get("display_name", "")
        if isinstance(model_obj, dict)
        else str(model_obj)
    ) or "Unknown"

    # Context window
    cw = data.get("context_window", {})
    ctx_pct = cw.get("used_percentage") if cw else None
    ctx_size = cw.get("context_window_size", 0) if cw else 0

    # Cache efficiency
    cache_pct: Optional[float] = None
    cu = cw.get("current_usage", {}) if cw else {}
    if cu:
        cache_read = cu.get("cache_read_input_tokens", 0)
        cache_create = cu.get("cache_creation_input_tokens", 0)
        input_tok = cu.get("input_tokens", 0)
        total = cache_read + cache_create + input_tok
        if total > 0:
            cache_pct = cache_read / total * 100

    # Record samples & compute projections
    proj_5h: Optional[float] = None
    proj_7d: Optional[float] = None
    t100_5h: Optional[str] = None
    t100_7d: Optional[str] = None
    trend_5h: Optional[str] = None
    trend_7d: Optional[str] = None
    conf_5h: Optional[str] = None
    conf_7d: Optional[str] = None
    rate_5h_ph: Optional[float] = None
    rate_7d_pd: Optional[float] = None
    proj_eta: Optional[str] = None
    peak_hour: bool = False

    try:
        db = open_db()

        if pct_5h is not None and resets_5h is not None:
            record_sample(db, "5h", pct_5h, resets_5h, session_id)
        if pct_7d is not None and resets_7d is not None:
            record_sample(db, "7d", pct_7d, resets_7d, session_id)

        # Prune occasionally (roughly every 100th call)
        if random.randint(0, 99) == 0:
            prune_old(db)

        weekday = datetime.now(timezone.utc).weekday()
        hourly_profile = get_hourly_activity_profile(db, current_weekday=weekday)

        def _has_enough_data(samples: list) -> bool:
            return (
                len(samples) >= MIN_SAMPLES_FOR_PROJECTION
                and samples[-1][0] - samples[0][0] >= MIN_TIMESPAN_FOR_PROJECTION
            )

        def _smooth_and_conf(wtype, samples):
            """Shared: smooth projection + compute confidence."""
            timespan = samples[-1][0] - samples[0][0] if len(samples) >= 2 else 0
            return compute_confidence(
                len(samples), timespan,
                bool(get_historical_rates(db, wtype)), len(hourly_profile),
            )

        def _time_to_100_linear(pct, resets, rate_per_min):
            """Estimate time to 100% using linear rate."""
            if rate_per_min is None or rate_per_min <= 0:
                return None
            remaining_pct = 100 - pct
            mins = remaining_pct / rate_per_min
            if mins * 60 > (resets - time.time()):
                return None  # won't hit before reset
            total_min = int(mins)
            if total_min < 1:
                return "<1m"
            if total_min >= 1440:  # 24h+
                d = total_min // 1440
                h = (total_min % 1440) // 60
                return f"{d}d{h:02d}h"
            if total_min >= 60:
                return f"{total_min // 60}h{total_min % 60:02d}m"
            return f"{total_min}m"

        # 5h: active-rate projection with hourly activity profile
        if pct_5h is not None and resets_5h is not None:
            samples_5h = get_window_samples(db, "5h", resets_5h)
            trend_5h = compute_trend(samples_5h)
            rate_5h_ph = rate_per_hour(samples_5h)

            if _has_enough_data(samples_5h):
                hist_rate = historical_median_rate(get_historical_rates(db, "5h"))
                rate = current_session_rate(samples_5h)
                raw = project_end_of_window(pct_5h, resets_5h, rate, hourly_profile, hist_rate)
                if raw is not None:
                    proj_5h = smooth_projection("5h", raw)
                    conf_5h = _smooth_and_conf("5h", samples_5h)
                    if proj_5h > 80:
                        t100_5h = time_to_threshold(pct_5h, resets_5h, rate, hourly_profile, hist_rate)
                    log.debug("5h: rate=%.4f%%/min proj=%.1f%% conf=%s samples=%d",
                              rate or 0, proj_5h, conf_5h, len(samples_5h))

            # Projection ETA
            if proj_5h is None and len(samples_5h) >= 2:
                span = samples_5h[-1][0] - samples_5h[0][0]
                remaining_sec = max(0, MIN_TIMESPAN_FOR_PROJECTION - span)
                if remaining_sec > 0:
                    proj_eta = f"{int(remaining_sec / 60) + 1}m"

        # 7d: simple linear projection (overall rate includes idle time)
        if pct_7d is not None and resets_7d is not None:
            samples_7d = get_window_samples(db, "7d", resets_7d)
            trend_7d = compute_trend(samples_7d)
            rate_7d_pd = rate_per_day(samples_7d)

            if _has_enough_data(samples_7d):
                rate_7d_min = overall_rate(samples_7d)
                raw = project_linear(pct_7d, resets_7d, rate_7d_min)
                if raw is not None:
                    proj_7d = smooth_projection("7d", raw)
                    conf_7d = _smooth_and_conf("7d", samples_7d)
                    if proj_7d > 80:
                        t100_7d = _time_to_100_linear(pct_7d, resets_7d, rate_7d_min)
                    log.debug("7d: rate=%.4f%%/min proj=%.1f%% conf=%s samples=%d",
                              rate_7d_min or 0, proj_7d, conf_7d, len(samples_7d))

        # Peak hour detection
        current_hour = datetime.now(timezone.utc).hour
        peak_hour = is_peak_hour(db, current_hour, weekday)

        db.close()
    except Exception:
        log.exception("storage/projection error")

    print(render_status_line(
        pct_5h=pct_5h,
        pct_7d=pct_7d,
        cooldown_5h=_format_cooldown_5h(resets_5h),
        cooldown_7d=_format_cooldown_7d(resets_7d),
        proj_5h=proj_5h,
        proj_7d=proj_7d,
        time_to_100_5h=t100_5h,
        time_to_100_7d=t100_7d,
        model=model_name,
        ctx_pct=ctx_pct,
        ctx_size=ctx_size,
        cache_pct=cache_pct,
        bypass=_is_bypass(),
        trend_5h=trend_5h,
        trend_7d=trend_7d,
        conf_5h=conf_5h,
        conf_7d=conf_7d,
        rate_per_h=rate_5h_ph,
        rate_per_d=rate_7d_pd,
        proj_eta=proj_eta,
        peak_hour=peak_hour,
    ))
