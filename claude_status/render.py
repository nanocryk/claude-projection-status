"""ANSI-colored status line rendering."""

from __future__ import annotations

import re
from typing import Optional

from .config import WARNING_PCT, CRITICAL_PCT, GREEN, YELLOW, RED, BOLD, DIM, RESET


def _color_for_pct(pct: float) -> str:
    if pct >= CRITICAL_PCT:
        return RED
    if pct >= WARNING_PCT:
        return YELLOW
    return GREEN


def _colored_pct(pct: float) -> str:
    return f"{_color_for_pct(pct)}{pct:.0f}%{RESET}"


def _format_number(n: int) -> str:
    if n >= 1_000_000:
        return f"{n / 1_000_000:.0f}M"
    if n >= 1_000:
        return f"{n / 1_000:.0f}k"
    return str(n)


def _format_window(
    label: str,
    pct: Optional[float],
    projected: Optional[float],
    cooldown: str,
    time_to_100: Optional[str],
) -> str:
    if pct is None:
        return f"{DIM}{label}: --% [--]{RESET}"

    parts = [f"{DIM}{label}:{RESET} {_colored_pct(pct)}"]

    if projected is not None:
        proj_color = _color_for_pct(projected)
        if projected > 80:
            proj_color = BOLD + RED
        proj_str = "100%+" if projected > 100 else f"{projected:.0f}%"
        parts.append(f"{DIM}~>{RESET}{proj_color}{proj_str}{RESET}")

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
) -> str:
    seg_5h = _format_window("5h", pct_5h, proj_5h, cooldown_5h, time_to_100_5h)
    seg_7d = _format_window("7d", pct_7d, proj_7d, cooldown_7d, time_to_100_7d)

    # Model segment with context
    model_clean = re.sub(r"\s*\([^)]*context[^)]*\)", "", model)
    if ctx_pct is not None and ctx_size > 0:
        model_seg = f"{model_clean} ({ctx_pct:.0f}%ctx)"
    else:
        model_seg = model_clean

    parts = [seg_5h, seg_7d, f"{DIM}{model_seg}{RESET}"]

    if bypass:
        parts.append(f"{BOLD}{RED}[BYPASS]{RESET}")

    return f" {DIM}|{RESET} ".join(parts)
