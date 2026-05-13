"""ANSI-colored status line rendering."""

from __future__ import annotations

import re
from datetime import datetime
from typing import Optional

from .config import WARNING_PCT, CRITICAL_PCT, GREEN, YELLOW, RED, COLD_BLUE, BOLD, DIM, RESET
from .glyphs import (
    FILL, PROJ, EMPTY, SPARK_LEVELS, FG_WHITE,
    CALENDAR, COLD, IDLE, BEE, WAVE, clock_glyph,
)
from .transcript import FAMILY_ORDER

# Family colors: green = cheap (good to see), yellow = mid, dim = baseline.
_FAMILY_COLORS = {"o": DIM, "s": YELLOW, "h": GREEN}

_ANSI_RE = re.compile(r"\033\[[0-9;]*m")


def _color_for_pct(pct: float) -> str:
    if pct >= CRITICAL_PCT:
        return RED
    if pct >= WARNING_PCT:
        return YELLOW
    return GREEN


def _fg_for_proj(pct: float, warn: float = 75, crit: float = 90) -> str:
    """Projection foreground color with per-window thresholds."""
    if pct >= crit:
        return RED
    if pct >= warn:
        return YELLOW
    return GREEN


def _colored_pct(pct: float) -> str:
    return f"{_color_for_pct(pct)}{f'{pct:.0f}%':>4}{RESET}"



def _build_two_tone_bar(
    pct: float,
    projected: Optional[float],
    width: int = 10,
    proj_warn: float = 75,
    proj_crit: float = 90,
) -> str:
    """Two-tone bar: solid=current, dim=projected extra, empty=remaining.

    [████▒▒░░░░] where █=current, ▒=projected-current, ░=free
    """
    clamped = max(0.0, min(pct, 100.0))
    filled = int(clamped / 100 * width + 0.5)
    if pct > 0 and filled == 0:
        filled = 1

    proj_filled = 0
    if projected is not None:
        proj_clamped = max(0.0, min(projected, 100.0))
        proj_total = int(proj_clamped / 100 * width + 0.5)
        proj_filled = max(0, proj_total - filled)

    empty = width - filled - proj_filled

    fg_proj = _fg_for_proj(projected or pct, proj_warn, proj_crit)

    bar = ""
    bar += f"{FG_WHITE}" + FILL * filled + RESET if filled else ""
    bar += f"{fg_proj}" + PROJ * proj_filled + RESET if proj_filled else ""
    bar += f"{DIM}" + EMPTY * empty + RESET if empty else ""

    return bar


def _build_sparkline(
    samples: list[tuple[float, float]],
    lookback_sec: float,
    buckets: int = 8,
    peak_floor: float = 0.5,
) -> str:
    """Sparkline of usage rate (Δpct per bucket) over the lookback window.

    No-data buckets render as a dim ▁ baseline.
    """
    if not samples or len(samples) < 2:
        return ""
    now = samples[-1][0]
    start = now - lookback_sec
    bucket_dur = lookback_sec / buckets
    earliest_t, earliest_p = samples[0]

    deltas: list[Optional[float]] = []
    for i in range(buckets):
        b_start = start + i * bucket_dur
        b_end = b_start + bucket_dur
        if b_end < earliest_t:
            deltas.append(None)
            continue
        before = next((p for t, p in reversed(samples) if t <= b_start), earliest_p)
        after = next((p for t, p in reversed(samples) if t <= b_end), earliest_p)
        deltas.append(max(0.0, after - before))

    valid = [d for d in deltas if d is not None]
    if not valid:
        return ""
    peak = max(max(valid), peak_floor)

    out = ""
    for d in deltas:
        if d is None:
            out += f"\033[38;5;238m▁{RESET}"
        else:
            lv = min(6, int(d / peak * 6 + 0.5))
            if lv >= 5:
                color = RED
            elif lv >= 3:
                color = YELLOW
            else:
                color = "\033[38;5;252m"
            out += f"{color}{SPARK_LEVELS[lv]}{RESET}"
    return out


def _visible_len(s: str) -> int:
    """Length of string after stripping ANSI escape codes and zero-width VS16."""
    return len(_ANSI_RE.sub("", s).replace("️", ""))


def _format_window(
    label: str,
    pct: Optional[float],
    projected: Optional[float],
    cooldown: str,
    time_to_100: Optional[str],
    samples: Optional[list[tuple[float, float]]] = None,
    lookback_sec: float = 5 * 3600,
    confidence: Optional[str] = None,
    rate_str: str = "",
    prefix_width: int = 0,
    proj_eta: Optional[str] = None,
    proj_warn: float = 75,
    proj_crit: float = 90,
) -> str:
    if pct is None:
        return f"{DIM}[--] {label}: --%{RESET}"

    # glyph cooldown/label bar pct% ⇒proj sparkline rate ⏰time
    if label == "7d":
        glyph = CALENDAR
    else:
        glyph = clock_glyph(datetime.now().hour)
    prefix = f"{DIM}{glyph} {cooldown}/{label}{RESET}"
    if prefix_width > 0:
        pad = prefix_width - _visible_len(prefix)
        if pad > 0:
            prefix += " " * pad

    bar = _build_two_tone_bar(pct, projected, proj_warn=proj_warn, proj_crit=proj_crit)
    parts = [f"{prefix} {bar}{_colored_pct(pct)}"]

    if projected is not None:
        proj_color = _fg_for_proj(projected, proj_warn, proj_crit)
        # Confidence saturates the projection: high=bold, medium=normal, low=dim
        conf_attr = BOLD if confidence == "high" else ("\033[2m" if confidence == "low" else "")
        parts.append(f"{DIM}⇒{RESET} {conf_attr}{proj_color}{f'{projected:.0f}%':>4}{RESET}")
    elif proj_eta:
        parts.append(f"{DIM}⇒ {proj_eta:>5}{RESET}")

    if samples:
        spark = _build_sparkline(samples, lookback_sec)
        if spark:
            parts.append(spark)

    if rate_str:
        parts.append(rate_str)

    if time_to_100:
        parts.append(f"{BOLD}{RED}⏰ {time_to_100}{RESET}")

    return " ".join(parts)


def _format_rate_h(rate: Optional[float]) -> str:
    """Format %/h rate."""
    if rate is None:
        return ""
    if rate < 0.5:
        return f"{DIM}{rate:.1f}%/h{RESET}"
    color = GREEN if rate < 15 else (YELLOW if rate < 30 else RED)
    return f"{color}{rate:.0f}%/h{RESET}"


def _format_rate_d(rate: Optional[float]) -> str:
    """Format %/d rate."""
    if rate is None:
        return ""
    if rate < 1:
        return f"{DIM}{rate:.1f}%/d{RESET}"
    color = GREEN if rate < 10 else (YELLOW if rate < 20 else RED)
    return f"{color}{rate:.0f}%/d{RESET}"


def _format_idle(idle_sec: Optional[float], cache_ttl: Optional[int]) -> str:
    """Idle indicator showing time since the conversation yielded to the user.

    Thresholds scale with the detected prompt-cache TTL: dim under 60% of
    TTL, yellow at 60-80%, red at 80-100%, bold red + ``cold`` past it.
    A ``[1h TTL]`` / ``[5m TTL]`` tag tells the user which TTL the client
    is using (silently auto-detected from recent assistant ``cache_creation``
    splits). When TTL can't be detected, fall back to 5m thresholds and
    omit the tag.
    """
    if idle_sec is None:
        return ""
    total = max(0, int(idle_sec))
    if total >= 3600:
        h, rem = divmod(total, 3600)
        m, _ = divmod(rem, 60)
        time_str = f"{h}h{m:02d}m"
    elif total >= 60:
        m, _ = divmod(total, 60)
        time_str = f"{m}m"
    else:
        time_str = f"{total:02d}s"

    ttl = cache_ttl if cache_ttl in (300, 3600) else 300
    ttl_tag = ""
    if cache_ttl == 3600:
        ttl_tag = "/1h"
    elif cache_ttl == 300:
        ttl_tag = "/5m"

    cold_thr = ttl
    red_thr = ttl * 0.8
    yellow_thr = ttl * 0.6

    if idle_sec >= cold_thr:
        glyph, color, bold = COLD, COLD_BLUE, BOLD
    elif idle_sec >= red_thr:
        glyph, color, bold = IDLE, RED, ""
    elif idle_sec >= yellow_thr:
        glyph, color, bold = IDLE, YELLOW, ""
    else:
        glyph, color, bold = IDLE, DIM, ""

    # Inverted bar: cells = time remaining before cold. Drains as idle grows.
    width = 5
    remaining = max(0.0, 1.0 - idle_sec / ttl)
    filled = max(0, int(remaining * width + 0.5)) if idle_sec < ttl else 0
    empty = width - filled
    bar = (f"{color}{FILL * filled}{RESET}" if filled else "") + (f"{DIM}{EMPTY * empty}{RESET}" if empty else "")
    nudge = f" {WAVE}" if idle_sec >= 1800 else ""
    return f"{glyph} {bar} {bold}{color}{time_str}{ttl_tag}{RESET}{nudge}"


def _build_ctx_bar(ctx_pct: float, width: int = 10) -> str:
    """Single-tone bar for context usage, threshold-coloured."""
    clamped = max(0.0, min(ctx_pct, 100.0))
    filled = int(clamped / 100 * width + 0.5)
    if ctx_pct > 0 and filled == 0:
        filled = 1
    empty = width - filled
    color = GREEN if ctx_pct < 50 else (YELLOW if ctx_pct < 80 else RED)
    out = ""
    if filled:
        out += f"{color}{FILL * filled}{RESET}"
    if empty:
        out += f"{DIM}{EMPTY * empty}{RESET}"
    return out


def _format_model_stats(
    shares: Optional[dict[str, float]],
    sub_count: int,
    sub_share: float = 0.0,
) -> list[str]:
    """Build colored per-family share strings + subagent count and token share.

    Hides shares when only one family is present (no signal). Hides count
    when zero. Within the family-share group, fixed order: opus, sonnet,
    haiku, then unknowns in alphabetic order.
    """
    parts: list[str] = []

    if shares and len(shares) >= 2:
        rounded = {fam: int(round(s * 100)) for fam, s in shares.items()}
        visible = {fam: pct for fam, pct in rounded.items() if pct > 0}
        if len(visible) >= 2:
            for fam in FAMILY_ORDER:
                if fam in visible:
                    color = _FAMILY_COLORS.get(fam, DIM)
                    parts.append(f"{color}{visible[fam]}%{fam}{RESET}")
            for fam in sorted(visible):
                if fam not in FAMILY_ORDER:
                    parts.append(f"{DIM}{visible[fam]}%{fam}{RESET}")

    if sub_count > 0:
        share_pct = int(round(sub_share * 100))
        suffix = f" {share_pct}%" if share_pct > 0 else ""
        parts.append(f"{DIM}{BEE}{sub_count}{suffix}{RESET}")

    return parts


def render_status_line(
    pct_5h: Optional[float],
    pct_7d: Optional[float],
    cooldown_5h: str,
    cooldown_7d: str,
    proj_5h: Optional[float],
    proj_7d: Optional[float],
    time_to_100_5h: Optional[str],
    time_to_100_7d: Optional[str],
    model: str,
    ctx_pct: Optional[float],
    ctx_size: int,
    bypass: bool = False,
    samples_5h: Optional[list[tuple[float, float]]] = None,
    samples_7d: Optional[list[tuple[float, float]]] = None,
    conf_5h: Optional[str] = None,
    conf_7d: Optional[str] = None,
    rate_per_h: Optional[float] = None,
    rate_per_d: Optional[float] = None,
    proj_eta: Optional[str] = None,
    model_shares: Optional[dict[str, float]] = None,
    subagent_count: int = 0,
    subagent_share: float = 0.0,
    idle_sec: Optional[float] = None,
    cache_ttl: Optional[int] = None,
) -> str:
    model_clean = re.sub(r"\s*\([^)]*context[^)]*\)", "", model)

    # Aligned prefix width so the bars on lines 1-2 start at the same column.
    pfx_5h = f"🕒 {cooldown_5h}/5h"
    pfx_7d = f"{CALENDAR} {cooldown_7d}/7d"
    prefix_width = max(_visible_len(pfx_5h), _visible_len(pfx_7d))

    seg_5h = _format_window("5h", pct_5h, proj_5h, cooldown_5h, time_to_100_5h,
                             samples_5h, 5 * 3600,
                             conf_5h, _format_rate_h(rate_per_h),
                             prefix_width=prefix_width,
                             proj_eta=proj_eta if proj_5h is None else None,
                             proj_warn=75, proj_crit=90)
    seg_7d = _format_window("7d", pct_7d, proj_7d, cooldown_7d, time_to_100_7d,
                             samples_7d, 24 * 3600,
                             conf_7d, _format_rate_d(rate_per_d),
                             prefix_width=prefix_width,
                             proj_warn=85, proj_crit=95)

    idle_str = _format_idle(idle_sec, cache_ttl)

    # Line 1: 5h + extras
    extras_1: list[str] = []
    if idle_str:
        extras_1.append(idle_str)
    if bypass:
        extras_1.append(f"{BOLD}{RED}[BYPASS]{RESET}")
    line1 = seg_5h + (" " + " ".join(extras_1) if extras_1 else "")

    # Line 2: 7d only
    line2 = seg_7d

    # Line 3: model name first (left-flush to survive leading-strip),
    # then pad to align ctx bar with the time bars on lines 1-2. Long
    # model names push the bar right rather than breaking alignment.
    head = f"{DIM}{model_clean}{RESET}"
    target_col = prefix_width + 2  # bar start column (see line 1/2 layout)
    pad = max(1, target_col - _visible_len(head))
    l3_parts: list[str] = []
    if ctx_pct is not None and ctx_size > 0:
        cc = GREEN if ctx_pct < 50 else (YELLOW if ctx_pct < 80 else RED)
        l3_parts.append(f"{_build_ctx_bar(ctx_pct)} {cc}{ctx_pct:.0f}%ctx{RESET}")
    model_text = _format_model_stats(model_shares, subagent_count, subagent_share)
    if model_text:
        l3_parts.append(" ".join(model_text))
    line3 = head + (" " * pad) + "  ".join(l3_parts)

    return line1 + "\n" + line2 + "\n" + line3
