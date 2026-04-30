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
import re
import time
from pathlib import Path
from typing import Optional

PROJECTS_ROOT = Path.home() / ".claude" / "projects"

# Trailing non-yield lines newer than this are assumed to be genuinely in-flight
# (model thinking, tool running). Older ones are treated as stalled (cancelled,
# permission prompt, AskUserQuestion, long-running tool) so idle anchors on
# the actual cache decay start.
STALE_TRAILING_SEC = 30.0

# Render order for known families; unknown families append after, sorted.
FAMILY_ORDER = ("o", "s", "h")
_FAMILY_PATTERNS = (("opus", "o"), ("sonnet", "s"), ("haiku", "h"))

_TOKEN_KEYS = (
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
)


_NON_ALNUM = re.compile(r"[^A-Za-z0-9]")


def _encode_cwd(cwd: str) -> str:
    """Mirror Claude Code's project-dir encoding.

    Every non-alphanumeric character is replaced with ``-``. So ``/``,
    ``_`` and ``.`` all become ``-`` (e.g. ``~/.claude`` becomes
    ``-home-user--claude``; ``foo_bar`` becomes ``foo-bar``).
    """
    return _NON_ALNUM.sub("-", cwd)


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


def _parse_iso_ts(ts: object) -> Optional[float]:
    """Parse the transcript's ISO-8601 ``...Z`` timestamps to unix seconds."""
    if not isinstance(ts, str):
        return None
    try:
        from datetime import datetime
        return datetime.fromisoformat(ts.replace("Z", "+00:00")).timestamp()
    except (TypeError, ValueError):
        return None


def last_main_assistant_ts(
    session_id: str,
    cwd: str,
    projects_root: Path = PROJECTS_ROOT,
) -> Optional[float]:
    """Unix timestamp of when the conversation last yielded back to user input.

    Normal case: the latest main-conversation line (non-sidechain ``user`` or
    ``assistant``) is an assistant turn that finished without a pending tool
    call (``message.stop_reason`` is not ``"tool_use"``); return its
    timestamp. The model is done responding and the prompt cache is decaying
    toward TTL.

    Trailing user message: a real prompt or tool result is sitting at the
    tail. If it's recent (< ``STALE_TRAILING_SEC``), assume Claude is
    actively processing and return ``None``. If it's older, the turn was
    cancelled/killed before the assistant could write back; fall back to the
    most recent prior assistant yield.

    Trailing tool_use assistant: the assistant emitted a tool call but no
    result has been written back. Same staleness rule: <30s means the tool
    is running or about to be approved (cache stays warm). Older means
    stalled on a permission prompt, AskUserQuestion, or long-running tool;
    cache is decaying either way, so anchor idle on the tool_use timestamp.

    Returns ``None`` for transcripts with nothing to anchor on.
    """
    pdir = session_dir(cwd, projects_root)
    if pdir is None or not session_id:
        return None
    main = pdir / f"{session_id}.jsonl"
    if not main.is_file():
        return None

    last_main: Optional[dict] = None
    last_yield_ts: Optional[float] = None
    try:
        fh = main.open("r", encoding="utf-8")
    except OSError:
        return None
    with fh:
        for line in fh:
            if '"type":"assistant"' not in line and '"type":"user"' not in line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            if obj.get("type") not in ("assistant", "user") or obj.get("isSidechain"):
                continue
            last_main = obj
            if obj.get("type") == "assistant":
                msg = obj.get("message")
                if isinstance(msg, dict) and msg.get("stop_reason") != "tool_use":
                    parsed = _parse_iso_ts(obj.get("timestamp"))
                    if parsed is not None:
                        last_yield_ts = parsed

    if last_main is None:
        return None

    if last_main.get("type") == "assistant":
        msg = last_main.get("message")
        if isinstance(msg, dict) and msg.get("stop_reason") == "tool_use":
            tool_ts = _parse_iso_ts(last_main.get("timestamp"))
            if tool_ts is None:
                return None
            if time.time() - tool_ts < STALE_TRAILING_SEC:
                return None
            return tool_ts
        return _parse_iso_ts(last_main.get("timestamp"))

    # Trailing user message: in-flight if recent, else fall back to prior yield.
    user_ts = _parse_iso_ts(last_main.get("timestamp"))
    if user_ts is None:
        return None
    if time.time() - user_ts < STALE_TRAILING_SEC:
        return None
    return last_yield_ts


def detected_cache_ttl(
    session_id: str,
    cwd: str,
    projects_root: Path = PROJECTS_ROOT,
) -> Optional[int]:
    """Return the prompt-cache TTL (seconds) the session is currently using.

    Walks the main JSONL and looks at ``message.usage.cache_creation`` on
    assistant turns. Anthropic reports cache writes split between
    ``ephemeral_5m_input_tokens`` and ``ephemeral_1h_input_tokens``; whichever
    bucket received the most recent write tells us the TTL the client picked.
    Returns 3600 for 1h, 300 for 5m, or ``None`` if no assistant turn so far
    committed any tokens to cache (new session, or all calls were pure reads).
    """
    pdir = session_dir(cwd, projects_root)
    if pdir is None or not session_id:
        return None
    main = pdir / f"{session_id}.jsonl"
    if not main.is_file():
        return None

    last_ttl: Optional[int] = None
    try:
        fh = main.open("r", encoding="utf-8")
    except OSError:
        return None
    with fh:
        for line in fh:
            if '"cache_creation"' not in line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            if obj.get("type") != "assistant" or obj.get("isSidechain"):
                continue
            cc = (obj.get("message") or {}).get("usage", {}).get("cache_creation")
            if not isinstance(cc, dict):
                continue
            t1h = int(cc.get("ephemeral_1h_input_tokens") or 0)
            t5m = int(cc.get("ephemeral_5m_input_tokens") or 0)
            if t1h > 0 and t1h >= t5m:
                last_ttl = 3600
            elif t5m > 0:
                last_ttl = 300

    return last_ttl


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
