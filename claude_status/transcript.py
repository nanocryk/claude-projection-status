"""Read Claude Code session transcripts to compute per-model token shares.

Claude Code writes session data under ``~/.claude/projects/<encoded-cwd>/``:

- ``<session_id>.jsonl`` is the main conversation. Each ``type: "assistant"``
  line carries ``message.model`` and ``message.usage``. ``isSidechain`` is
  always ``false`` here.
- ``<session_id>/subagents/agent-<hash>.jsonl`` is one file per spawned
  subagent. Same record shape; assistant turns have ``isSidechain: true`` and
  ``message.model`` reflects whatever model that subagent ran on.

The token unit is the sum of ``input_tokens``, ``output_tokens``,
``cache_read_input_tokens`` and ``cache_creation_input_tokens`` per turn,
matching what's actually billed.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Optional

PROJECTS_ROOT = Path.home() / ".claude" / "projects"

# Render order for known families; unknown families append after, sorted.
FAMILY_ORDER = ("o", "s", "h")
_FAMILY_PATTERNS = (("opus", "o"), ("sonnet", "s"), ("haiku", "h"))

_TOKEN_KEYS = (
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
)


def _encode_cwd(cwd: str) -> str:
    """Mirror Claude Code's project-dir encoding: replace '/' with '-'."""
    return cwd.replace("/", "-")


def session_dir(cwd: str, projects_root: Path = PROJECTS_ROOT) -> Optional[Path]:
    """Return the project dir for ``cwd``, or None if it doesn't exist."""
    if not cwd:
        return None
    p = projects_root / _encode_cwd(cwd)
    return p if p.is_dir() else None


def _family(model: str) -> Optional[str]:
    if not model or model.startswith("<"):
        return None
    for needle, code in _FAMILY_PATTERNS:
        if needle in model:
            return code
    return "?"


def _accumulate(path: Path, totals: dict[str, int]) -> None:
    """Stream-parse one JSONL file, add token totals by family in-place."""
    try:
        fh = path.open("r", encoding="utf-8")
    except OSError:
        return
    with fh:
        for line in fh:
            if '"type":"assistant"' not in line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            if obj.get("type") != "assistant":
                continue
            msg = obj.get("message") or {}
            fam = _family(msg.get("model", ""))
            if fam is None:
                continue
            usage = msg.get("usage") or {}
            tokens = sum(int(usage.get(k, 0) or 0) for k in _TOKEN_KEYS)
            if tokens <= 0:
                continue
            totals[fam] = totals.get(fam, 0) + tokens


def model_token_shares(
    session_id: str,
    cwd: str,
    projects_root: Path = PROJECTS_ROOT,
) -> dict[str, float]:
    """Return ``{family: share}`` summing to 1.0 across the session.

    Returns ``{}`` if the session can't be found or has no countable tokens.
    Walks the main session JSONL plus every ``agent-*.jsonl`` under
    ``<session_id>/subagents/``.
    """
    pdir = session_dir(cwd, projects_root)
    if pdir is None or not session_id:
        return {}

    totals: dict[str, int] = {}

    main = pdir / f"{session_id}.jsonl"
    if main.is_file():
        _accumulate(main, totals)

    sub_dir = pdir / session_id / "subagents"
    if sub_dir.is_dir():
        try:
            entries = sorted(sub_dir.iterdir())
        except OSError:
            entries = []
        for entry in entries:
            if entry.suffix == ".jsonl" and entry.name.startswith("agent-"):
                _accumulate(entry, totals)

    grand = sum(totals.values())
    if grand <= 0:
        return {}
    return {fam: cnt / grand for fam, cnt in totals.items()}


def format_mix(shares: dict[str, float]) -> str:
    """Format shares as ``60%o 30%s 10%h``.

    Returns ``""`` if there's only one family present (no signal to convey).
    Drops entries that round to 0%. Known families render in fixed order
    ``o s h``; unknowns append in alphabetic order so layout stays stable.
    """
    if len(shares) < 2:
        return ""

    rounded = {fam: int(round(share * 100)) for fam, share in shares.items()}
    visible = {fam: pct for fam, pct in rounded.items() if pct > 0}
    if len(visible) < 2:
        return ""

    ordered: list[str] = []
    for fam in FAMILY_ORDER:
        if fam in visible:
            ordered.append(f"{visible[fam]}%{fam}")
    for fam in sorted(visible):
        if fam not in FAMILY_ORDER:
            ordered.append(f"{visible[fam]}%{fam}")
    return " ".join(ordered)


def session_mix_string(
    session_id: str,
    cwd: str,
    projects_root: Path = PROJECTS_ROOT,
) -> str:
    """Convenience: shares + format in one call. Returns ``""`` if no signal."""
    return format_mix(model_token_shares(session_id, cwd, projects_root))


def subagent_count(
    session_id: str,
    cwd: str,
    projects_root: Path = PROJECTS_ROOT,
) -> int:
    """Number of subagents spawned in the session.

    One ``agent-<hash>.jsonl`` file per spawn under ``<session>/subagents/``.
    Returns 0 if the session or directory doesn't exist.
    """
    pdir = session_dir(cwd, projects_root)
    if pdir is None or not session_id:
        return 0
    sub_dir = pdir / session_id / "subagents"
    if not sub_dir.is_dir():
        return 0
    try:
        return sum(
            1 for entry in sub_dir.iterdir()
            if entry.suffix == ".jsonl" and entry.name.startswith("agent-")
        )
    except OSError:
        return 0
