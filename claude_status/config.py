import json
import logging
import os
from pathlib import Path
from typing import Any

# Config file: ~/.config/claude-status/config.json
# Env vars override config file values.
CONFIG_PATH = Path(os.environ.get(
    "CLAUDE_STATUS_CONFIG",
    Path.home() / ".config" / "claude-status" / "config.json",
))

def _load_config_file() -> dict[str, Any]:
    try:
        if CONFIG_PATH.exists():
            return json.loads(CONFIG_PATH.read_text())
    except (json.JSONDecodeError, OSError):
        pass
    return {}

_cfg = _load_config_file()

def _get(key: str, env_key: str, default: str) -> str:
    """Env var > config file > default."""
    return os.environ.get(env_key, str(_cfg.get(key, default)))


WARNING_PCT = float(_get("warning_pct", "CLAUDE_STATUS_WARNING", "40"))
CRITICAL_PCT = float(_get("critical_pct", "CLAUDE_STATUS_CRITICAL", "70"))

MULTILINE = _get("multiline", "CLAUDE_STATUS_MULTILINE", "false").lower() in ("1", "true", "yes")

CACHE_DIR = Path(_get("cache_dir", "CLAUDE_STATUS_CACHE",
                       str(Path.home() / ".cache" / "claude-status")))
DB_PATH = CACHE_DIR / "history.db"
RETENTION_DAYS = int(_get("retention_days", "CLAUDE_STATUS_RETENTION", "14"))

MIN_SAMPLES_FOR_PROJECTION = int(_get("min_samples", "CLAUDE_STATUS_MIN_SAMPLES", "5"))

# Minimum time span (seconds) between first and last sample before projecting
MIN_TIMESPAN_FOR_PROJECTION = int(_get("min_timespan", "CLAUDE_STATUS_MIN_TIMESPAN", "600"))

# ANSI codes
GREEN = "\033[32m"
YELLOW = "\033[33m"
RED = "\033[31m"
BOLD = "\033[1m"
DIM = "\033[2m"
RESET = "\033[0m"

# Debug logging
DEBUG = _get("debug", "CLAUDE_STATUS_DEBUG", "false").lower() in ("1", "true", "yes")
log = logging.getLogger("claude-status")

if DEBUG:
    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    _handler = logging.FileHandler(CACHE_DIR / "debug.log")
    _handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(message)s"))
    log.addHandler(_handler)
    log.setLevel(logging.DEBUG)
else:
    log.addHandler(logging.NullHandler())
    log.setLevel(logging.CRITICAL)
