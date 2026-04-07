import os
from pathlib import Path

WARNING_PCT = float(os.environ.get("CLAUDE_STATUS_WARNING", "40"))
CRITICAL_PCT = float(os.environ.get("CLAUDE_STATUS_CRITICAL", "70"))

DB_PATH = Path(os.environ.get(
    "CLAUDE_STATUS_DB",
    Path.home() / ".cache" / "claude-status" / "history.db",
))
RETENTION_DAYS = 14

# Minimum samples in current window before showing projection
MIN_SAMPLES_FOR_PROJECTION = 5

# ANSI codes
GREEN = "\033[32m"
YELLOW = "\033[33m"
RED = "\033[31m"
BOLD = "\033[1m"
DIM = "\033[2m"
RESET = "\033[0m"
