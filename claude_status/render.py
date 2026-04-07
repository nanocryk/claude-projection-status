"""ANSI-colored status line rendering."""

from __future__ import annotations

import os
import re
from typing import Optional

from .config import WARNING_PCT, CRITICAL_PCT, GREEN, YELLOW, RED, BOLD, DIM, RESET

COMPACT = os.environ.get("CLAUDE_STATUS_COMPACT", "").lower() in ("1", "true", "yes")

# Bar characters
FILL = "\u2588"      # █ solid — current usage
PROJ = "\u2592"      # ▒ medium shade — projected additional
EMPTY = "\u2591"     # ░ light shade — remaining

# Background colors for bar segments
BG_GREEN = "\033[42m"
BG_YELLOW = "\033[43m"
BG_RED = "\033[41m"
BG_DARK = "\033[100m"
FG_WHITE = "\033[97m"
FG_BLACK = "\033[30m"


def _color_for_pct(pct: float) -> str:
    if pct >= CRITICAL_PCT:
        return RED
    if pct >= WARNING_PCT:
        return YELLOW
    return GREEN


def _bg_for_pct(pct: float) -> str:
    if pct >= CRITICAL_PCT:
        return BG_RED
    if pct >= WARNING_PCT:
        return BG_YELLOW
    return BG_GREEN


def _colored_pct(pct: float) -> str:
    return f"{_color_for_pct(pct)}{pct:.0f}%{RESET}"


def _format_number(n: int) -> str:
    if n >= 1_000_000:
        return f"{n / 1_000_000:.0f}M"
    if n >= 1_000:
        return f"{n / 1_000:.0f}k"
    return str(n)


def _build_two_tone_bar(
    pct: float,
    projected: Optional[float],
    width: int = 10,
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

    bg_current = _bg_for_pct(pct)
    bg_proj = _bg_for_pct(projected or pct)

    bar = ""
    bar += f"{bg_current}{FG_WHITE}" + FILL * filled + RESET if filled else ""
    bar += f"{DIM}{bg_proj}{FG_WHITE}" + PROJ * proj_filled + RESET if proj_filled else ""
    bar += f"{BG_DARK}{FG_WHITE}" + EMPTY * empty + RESET if empty else ""

    return f"[{bar}]"


def _trend_arrow(trend: Optional[str]) -> str:
    """Format trend indicator."""
    if trend == "up":
        return f"{RED}\u2191{RESET}"       # ↑
    if trend == "down":
        return f"{GREEN}\u2193{RESET}"     # ↓
    if trend == "stable":
        return f"{DIM}\u2192{RESET}"       # →
    return ""


def _confidence_prefix(conf: Optional[str]) -> str:
    """Prefix for projected value based on confidence."""
    if conf == "low":
        return "~"
    if conf == "medium":
        return "\u2248"  # ≈
    return ""


def _format_window(
    label: str,
    pct: Optional[float],
    projected: Optional[float],
    cooldown: str,
    time_to_100: Optional[str],
    trend: Optional[str] = None,
    confidence: Optional[str] = None,
) -> str:
    if pct is None:
        if COMPACT:
            return f"{DIM}{label}:--{RESET}"
        return f"{DIM}{label}: --% [--]{RESET}"

    if COMPACT:
        parts = [f"{DIM}{label}:{RESET}{_colored_pct(pct)}"]
        if projected is not None:
            proj_color = _color_for_pct(projected)
            if projected > 80:
                proj_color = BOLD + RED
            proj_str = "100+" if projected > 100 else f"{projected:.0f}"
            parts.append(f"{DIM}\u2192{RESET}{proj_color}{proj_str}{RESET}")
        return "".join(parts)

    # Full mode
    bar = _build_two_tone_bar(pct, projected)
    parts = [f"{DIM}{label}:{RESET}{bar}{_colored_pct(pct)}"]

    if projected is not None:
        proj_color = _color_for_pct(projected)
        if projected > 80:
            proj_color = BOLD + RED
        cpfx = _confidence_prefix(confidence)
        proj_str = "100%+" if projected > 100 else f"{projected:.0f}%"
        parts.append(f"{DIM}~>{RESET}{proj_color}{cpfx}{proj_str}{RESET}")

    arrow = _trend_arrow(trend)
    if arrow:
        parts.append(arrow)

    parts.append(f"{DIM}[{cooldown}]{RESET}")

    if time_to_100:
        parts.append(f"{BOLD}{RED}!100%~{time_to_100}{RESET}")

    return " ".join(parts)


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
    bypass: bool,
    trend_5h: Optional[str] = None,
    trend_7d: Optional[str] = None,
    conf_5h: Optional[str] = None,
    conf_7d: Optional[str] = None,
) -> str:
    seg_5h = _format_window("5h", pct_5h, proj_5h, cooldown_5h, time_to_100_5h, trend_5h, conf_5h)
    seg_7d = _format_window("7d", pct_7d, proj_7d, cooldown_7d, time_to_100_7d, trend_7d, conf_7d)

    # Model segment with context
    model_clean = re.sub(r"\s*\([^)]*context[^)]*\)", "", model)
    if ctx_pct is not None and ctx_size > 0:
        model_seg = f"{model_clean} ({ctx_pct:.0f}%ctx)"
    else:
        model_seg = model_clean

    if COMPACT:
        return f"{seg_5h} {seg_7d} {DIM}{model_clean}{RESET}"

    parts = [seg_5h, seg_7d, f"{DIM}{model_seg}{RESET}"]

    if bypass:
        parts.append(f"{BOLD}{RED}[BYPASS]{RESET}")

    return f" {DIM}|{RESET} ".join(parts)
