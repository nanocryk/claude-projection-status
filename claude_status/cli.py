"""Entry point: parse stdin from Claude Code, record, project, render."""

from __future__ import annotations

import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from .config import MIN_SAMPLES_FOR_PROJECTION, MIN_TIMESPAN_FOR_PROJECTION
from .projection import (
    current_session_rate,
    historical_median_rate,
    project_end_of_window,
    time_to_threshold,
)
from .render import render_status_line
from .storage import (
    get_historical_rates,
    get_hourly_activity_profile,
    get_window_samples,
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


def main() -> None:
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

    # Record samples & compute projections
    proj_5h: Optional[float] = None
    proj_7d: Optional[float] = None
    t100_5h: Optional[str] = None
    t100_7d: Optional[str] = None

    try:
        db = open_db()

        if pct_5h is not None and resets_5h is not None:
            record_sample(db, "5h", pct_5h, resets_5h, session_id)
        if pct_7d is not None and resets_7d is not None:
            record_sample(db, "7d", pct_7d, resets_7d, session_id)

        # Prune occasionally (roughly every 100th call)
        import random
        if random.randint(0, 99) == 0:
            prune_old(db)

        hourly_profile = get_hourly_activity_profile(db)
        hist_rates = get_historical_rates(db)
        hist_rate = historical_median_rate(hist_rates)

        def _has_enough_data(samples: list) -> bool:
            return (
                len(samples) >= MIN_SAMPLES_FOR_PROJECTION
                and samples[-1][0] - samples[0][0] >= MIN_TIMESPAN_FOR_PROJECTION
            )

        # 5h projection
        if pct_5h is not None and resets_5h is not None:
            samples_5h = get_window_samples(db, "5h", resets_5h)
            if _has_enough_data(samples_5h):
                rate_5h = current_session_rate(samples_5h)
                proj_5h = project_end_of_window(
                    pct_5h, resets_5h, rate_5h, hourly_profile, hist_rate,
                )
                if proj_5h is not None and proj_5h > 80:
                    t100_5h = time_to_threshold(
                        pct_5h, resets_5h, rate_5h, hourly_profile, hist_rate,
                    )

        # 7d projection
        if pct_7d is not None and resets_7d is not None:
            samples_7d = get_window_samples(db, "7d", resets_7d)
            if _has_enough_data(samples_7d):
                rate_7d = current_session_rate(samples_7d)
                proj_7d = project_end_of_window(
                    pct_7d, resets_7d, rate_7d, hourly_profile, hist_rate,
                )
                if proj_7d is not None and proj_7d > 80:
                    t100_7d = time_to_threshold(
                        pct_7d, resets_7d, rate_7d, hourly_profile, hist_rate,
                    )

        db.close()
    except Exception:
        pass  # Storage/projection failure should never break the status line

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
        bypass=_is_bypass(),
    ))
