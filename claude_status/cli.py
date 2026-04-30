"""Entry point: parse stdin from Claude Code, record, project, render."""

from __future__ import annotations

import json
import os
import random
import sys
import time
from datetime import datetime, timezone
from typing import Any, Optional

from .config import MIN_SAMPLES_FOR_PROJECTION, MIN_TIMESPAN_FOR_PROJECTION, SHOW_MODEL_MIX, log
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
from .threshold import latest_used_pct
from .transcript import (
    detected_cache_ttl,
    last_main_assistant_ts,
    model_token_shares,
    subagent_count,
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


def _format_cooldown(resets_at: Optional[float], use_days: bool = False) -> str:
    """Format time remaining until window reset."""
    if resets_at is None:
        return "   --"
    now = datetime.now(timezone.utc)
    diff = datetime.fromtimestamp(resets_at, tz=timezone.utc) - now
    total_sec = max(0, int(diff.total_seconds()))
    d = total_sec // 86400
    h = (total_sec % 86400) // 3600
    m = (total_sec % 3600) // 60
    if use_days and d > 0:
        return f"{d}d{h:02d}h".rjust(5)
    if h > 0:
        return f"{h}h{m:02d}m".rjust(5)
    return f"{m}m".rjust(5)


def _is_bypass() -> bool:
    env = os.environ.get("CLAUDE_SKIP_PERMISSIONS", "").lower()
    if env in ("1", "true", "yes"):
        return True
    try:
        from pathlib import Path
        settings = json.loads(
            (Path.home() / ".claude" / "settings.json").read_text()
        )
        return settings.get("defaultMode") == "bypassPermissions"
    except Exception:
        return False


def _has_enough_data(samples: list[tuple[float, float]]) -> bool:
    return (
        len(samples) >= MIN_SAMPLES_FOR_PROJECTION
        and samples[-1][0] - samples[0][0] >= MIN_TIMESPAN_FOR_PROJECTION
    )


def _compute_confidence(db, wtype: str, samples: list[tuple[float, float]],
                         hourly_profile: dict[int, float]) -> str:
    timespan = samples[-1][0] - samples[0][0] if len(samples) >= 2 else 0
    return compute_confidence(
        len(samples), timespan,
        bool(get_historical_rates(db, wtype)), len(hourly_profile),
    )


def _time_to_100_linear(pct: float, resets: float, rate_per_min: Optional[float]) -> Optional[str]:
    """Time to 100% using simple linear rate. Used for 7d (rate includes idle)."""
    if rate_per_min is None or rate_per_min <= 0:
        return None
    remaining_pct = 100 - pct
    mins = remaining_pct / rate_per_min
    if mins * 60 > (resets - time.time()):
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


def _project_5h(db, pct: float, resets: float,
                hourly_profile: dict[int, float]) -> dict[str, Any]:
    """Compute 5h projection using active-rate + hourly activity profile."""
    result: dict[str, Any] = {"proj": None, "conf": None, "trend": None,
                               "rate": None, "t100": None, "eta": None}

    samples = get_window_samples(db, "5h", resets)
    result["trend"] = compute_trend(samples)
    result["rate"] = rate_per_hour(samples)

    if _has_enough_data(samples):
        hist_rate = historical_median_rate(get_historical_rates(db, "5h"))
        rate = current_session_rate(samples)
        raw = project_end_of_window(pct, resets, rate, hourly_profile, hist_rate)
        if raw is not None:
            result["proj"] = smooth_projection("5h", raw)
            result["conf"] = _compute_confidence(db, "5h", samples, hourly_profile)
            if result["proj"] > 80:
                result["t100"] = time_to_threshold(pct, resets, rate, hourly_profile, hist_rate)
            log.debug("5h: rate=%.4f%%/min proj=%.1f%% conf=%s samples=%d",
                      rate or 0, result["proj"], result["conf"], len(samples))

    # Projection ETA countdown
    if result["proj"] is None and len(samples) >= 2:
        span = samples[-1][0] - samples[0][0]
        remaining_sec = max(0, MIN_TIMESPAN_FOR_PROJECTION - span)
        if remaining_sec > 0:
            result["eta"] = f"{int(remaining_sec / 60) + 1}m"

    return result


def _project_7d(db, pct: float, resets: float,
                hourly_profile: dict[int, float]) -> dict[str, Any]:
    """Compute 7d projection using overall rate (includes idle time)."""
    result: dict[str, Any] = {"proj": None, "conf": None, "trend": None,
                               "rate": None, "t100": None}

    samples = get_window_samples(db, "7d", resets)
    result["trend"] = compute_trend(samples)
    result["rate"] = rate_per_day(samples)

    if _has_enough_data(samples):
        rate_min = overall_rate(samples)
        raw = project_linear(pct, resets, rate_min)
        if raw is not None:
            result["proj"] = smooth_projection("7d", raw)
            result["conf"] = _compute_confidence(db, "7d", samples, hourly_profile)
            if result["proj"] > 80:
                result["t100"] = _time_to_100_linear(pct, resets, rate_min)
            log.debug("7d: rate=%.4f%%/min proj=%.1f%% conf=%s samples=%d",
                      rate_min or 0, result["proj"], result["conf"], len(samples))

    return result


def _print_summary() -> None:
    """Print usage statistics from the database."""
    db = open_db()

    total = db.execute("SELECT COUNT(*) FROM usage_samples").fetchone()[0]
    oldest = db.execute("SELECT MIN(timestamp) FROM usage_samples").fetchone()[0]
    newest = db.execute("SELECT MAX(timestamp) FROM usage_samples").fetchone()[0]

    print("=== Claude Status Summary ===")
    print(f"Total samples: {total}")
    if oldest and newest:
        from_dt = datetime.fromtimestamp(oldest, tz=timezone.utc).strftime("%Y-%m-%d %H:%M")
        to_dt = datetime.fromtimestamp(newest, tz=timezone.utc).strftime("%Y-%m-%d %H:%M")
        print(f"Date range: {from_dt} — {to_dt}")

    windows = db.execute(
        "SELECT window_type, COUNT(DISTINCT resets_at) FROM usage_samples GROUP BY window_type"
    ).fetchall()
    for wt, cnt in windows:
        print(f"  {wt} windows tracked: {cnt}")

    weekday = datetime.now(timezone.utc).weekday()
    profile = get_hourly_activity_profile(db, current_weekday=weekday)
    if profile:
        print("\nHourly activity (P(active)):")
        for h in range(24):
            p = profile.get(h, 0.0)
            bar = "#" * int(p * 20)
            print(f"  {h:02d}:00  {bar:<20s} {p:.0%}")

    rates = get_historical_rates(db)
    med = historical_median_rate(rates)
    if med is not None:
        print(f"\nHistorical median rate: {med:.3f}%/min")
        print(f"  ({len(rates)} window(s) analyzed)")

    peaks = db.execute(
        "SELECT window_type, resets_at, MAX(used_pct) FROM usage_samples "
        "GROUP BY window_type, resets_at ORDER BY MAX(used_pct) DESC LIMIT 5"
    ).fetchall()
    if peaks:
        print("\nTop usage windows:")
        for wt, ra, peak in peaks:
            reset_dt = datetime.fromtimestamp(ra, tz=timezone.utc).strftime("%Y-%m-%d %H:%M")
            print(f"  {wt} reset {reset_dt}: peak {peak:.0f}%")

    db.close()


def _check_threshold(argv: list[str]) -> int:
    """Print latest used_pct if it crosses --threshold, else nothing.

    Used by Stop hooks to decide whether to force a handoff. Always exits 0
    on success (crossed or not); non-zero only on real errors.
    """
    import argparse
    p = argparse.ArgumentParser(prog="claude-status check-threshold")
    p.add_argument("--session-id", required=True)
    p.add_argument("--window", required=True, help="window_type, e.g. '5h' or '7d'")
    p.add_argument("--threshold", required=True, type=float, help="percentage, 0-100")
    args = p.parse_args(argv)

    db = open_db()
    try:
        pct = latest_used_pct(db, args.window, args.session_id)
    finally:
        db.close()

    if pct is not None and pct >= args.threshold:
        print(f"{pct:.1f}")
    return 0


def main() -> None:
    if len(sys.argv) >= 2 and sys.argv[1] == "check-threshold":
        sys.exit(_check_threshold(sys.argv[2:]))

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

    cwd = ""
    workspace = data.get("workspace") or {}
    if isinstance(workspace, dict):
        cwd = workspace.get("current_dir") or workspace.get("project_dir") or ""
    if not cwd:
        cwd = data.get("cwd") or os.getcwd()

    model_shares: dict[str, float] = {}
    sub_count = 0
    idle_sec: Optional[float] = None
    cache_ttl: Optional[int] = None
    if SHOW_MODEL_MIX and session_id:
        try:
            model_shares = model_token_shares(session_id, cwd)
            sub_count = subagent_count(session_id, cwd)
            last_ts = last_main_assistant_ts(session_id, cwd)
            if last_ts is not None:
                idle_sec = max(0.0, time.time() - last_ts)
            cache_ttl = detected_cache_ttl(session_id, cwd)
        except Exception:
            log.exception("model mix computation failed")

    model_obj = data.get("model", {})
    model_name = (
        model_obj.get("display_name", "")
        if isinstance(model_obj, dict)
        else str(model_obj)
    ) or "Unknown"

    cw = data.get("context_window", {})
    ctx_pct = cw.get("used_percentage") if cw else None
    ctx_size = cw.get("context_window_size", 0) if cw else 0

    # Record & project
    r5h: dict[str, Any] = {}
    r7d: dict[str, Any] = {}
    peak_hour = False

    try:
        db = open_db()

        if pct_5h is not None and resets_5h is not None:
            record_sample(db, "5h", pct_5h, resets_5h, session_id)
        if pct_7d is not None and resets_7d is not None:
            record_sample(db, "7d", pct_7d, resets_7d, session_id)

        # Prune ~1% of calls
        if random.randint(0, 99) == 0:
            prune_old(db)

        weekday = datetime.now(timezone.utc).weekday()
        hourly_profile = get_hourly_activity_profile(db, current_weekday=weekday)

        if pct_5h is not None and resets_5h is not None:
            r5h = _project_5h(db, pct_5h, resets_5h, hourly_profile)

        if pct_7d is not None and resets_7d is not None:
            r7d = _project_7d(db, pct_7d, resets_7d, hourly_profile)

        current_hour = datetime.now(timezone.utc).hour
        peak_hour = is_peak_hour(db, current_hour, weekday)

        db.close()
    except Exception:
        log.exception("storage/projection error")

    print(render_status_line(
        pct_5h=pct_5h,
        pct_7d=pct_7d,
        cooldown_5h=_format_cooldown(resets_5h),
        cooldown_7d=_format_cooldown(resets_7d, use_days=True),
        proj_5h=r5h.get("proj"),
        proj_7d=r7d.get("proj"),
        time_to_100_5h=r5h.get("t100"),
        time_to_100_7d=r7d.get("t100"),
        model=model_name,
        ctx_pct=ctx_pct,
        ctx_size=ctx_size,
        bypass=_is_bypass(),
        trend_5h=r5h.get("trend"),
        trend_7d=r7d.get("trend"),
        conf_5h=r5h.get("conf"),
        conf_7d=r7d.get("conf"),
        rate_per_h=r5h.get("rate"),
        rate_per_d=r7d.get("rate"),
        proj_eta=r5h.get("eta"),
        peak_hour=peak_hour,
        model_shares=model_shares,
        subagent_count=sub_count,
        idle_sec=idle_sec,
        cache_ttl=cache_ttl,
    ))
