# claude-status

A custom status bar for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) that replaces the default rate-limit display with rich usage visualization, projections, and historical tracking.

## Motivation

Inspired by [leeguooooo/claude-code-usage-bar](https://github.com/leeguooooo/claude-code-usage-bar), which provides a real-time statusline with token usage and burn rate. This project rebuilds the concept from scratch with a focus on predictive projections and historical learning. It adds:

- **Visual usage bars** with two-tone display (current + projected)
- **End-of-window projections** using historical activity patterns
- **Time-to-100% estimates** factoring in hourly activity probability
- **Trend indicators** showing acceleration/deceleration
- **Cache hit ratio** and context window consumption
- **Peak hour detection** from historical usage patterns
- **Multi-line layout** with aligned bars for 5h and 7d windows

## Screenshot

```
[4h32] 5h:[██████▒▒──]15% ~>≈23% ↑ 8%/h   !2h30m
[6d02] 7d:[██▒───────] 2% ~>≈ 8% → 4%/d   opus-4 (42%ctx 85%hit)
```

## How It Works

Claude Code pipes rate-limit JSON to stdin on each status refresh. This tool:

1. **Records** each usage sample in a local SQLite database
2. **Builds** an hourly activity profile from historical data (P(active) per hour)
3. **Projects** end-of-window usage by combining current session rate (60%) with historical median (40%), modulated by activity probability per hour
4. **Renders** an ANSI-colored status line to stdout

### Projection Algorithm

- **5h window**: Active-rate projection with hourly activity profile. Walks hour-by-hour, multiplying effective rate by P(active) for each hour.
- **7d window**: Linear projection using overall rate (includes idle time), since idle patterns are already baked into the rate.

### Data Storage

SQLite database at `~/.cache/claude-status/history.db` with two tables:
- `usage_samples` — timestamped usage snapshots per window type
- `active_hours` — hourly activity profile (sample count + usage delta per hour/weekday)

Data is automatically pruned after 14 days (configurable).

## Installation

Requires Python >= 3.10. No external dependencies.

```bash
# Clone
git clone https://github.com/user/custom-claude-statusbar.git
cd custom-claude-statusbar

# Install (editable mode recommended for easy updates)
pip install -e .
```

### Register with Claude Code

Add to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "/path/to/custom-claude-statusbar/claude-status",
    "padding": 0
  }
}
```

Or if installed via pip:

```json
{
  "statusLine": {
    "type": "command",
    "command": "claude-status",
    "padding": 0
  }
}
```

## Configuration

Settings are read from `~/.config/claude-status/config.json`, overridable by environment variables. Env vars take precedence over the config file.

```json
{
  "multiline": true,
  "warning_pct": 40,
  "critical_pct": 70,
  "retention_days": 14,
  "min_samples": 5,
  "min_timespan": 600
}
```

| Setting | Env Var | Default | Description |
|---------|---------|---------|-------------|
| `multiline` | `CLAUDE_STATUS_MULTILINE` | `false` | Two-line layout (5h + 7d stacked) |
| `warning_pct` | `CLAUDE_STATUS_WARNING` | `40` | Yellow threshold (%) |
| `critical_pct` | `CLAUDE_STATUS_CRITICAL` | `70` | Red threshold (%) |
| `cache_dir` | `CLAUDE_STATUS_CACHE` | `~/.cache/claude-status` | Database and cache location |
| `retention_days` | `CLAUDE_STATUS_RETENTION` | `14` | Days of history to keep |
| `min_samples` | `CLAUDE_STATUS_MIN_SAMPLES` | `5` | Minimum samples before projecting |
| `min_timespan` | `CLAUDE_STATUS_MIN_TIMESPAN` | `600` | Seconds of data needed before projecting |
| `debug` | `CLAUDE_STATUS_DEBUG` | `false` | Enable debug logging to `debug.log` |
| *(env only)* | `CLAUDE_STATUS_COMPACT` | `false` | Ultra-compact single-line mode |

## Display Modes

### Multiline (recommended)

Two lines with aligned bars, 3-character gap between window data and metadata:

```
[4h32] 5h:[████──────] 5% ~>≈13% → 3%/h
[6d02] 7d:[██────────] 2% ~>≈ 8% → 4%/d   opus-4 (42%ctx 85%hit)
```

### Single-line (default)

All segments joined with `|`:

```
[4h32] 5h:[████──────]5% ~>≈13% → 3%/h | [6d02] 7d:[██────────]2% ~>≈8% → 4%/d | opus-4 (42%ctx 85%hit)
```

### Compact

Minimal output for narrow terminals:

```
5h:5%→13 7d:2%→8 opus-4
```

## Status Elements

| Element | Example | Meaning |
|---------|---------|---------|
| `[4h32]` | Cooldown | Time until window resets |
| `[████▒▒────]` | Bar | Solid=current, shaded=projected, line=free |
| `15%` | Usage | Current usage (green/yellow/red) |
| `~>≈23%` | Projection | Estimated end-of-window usage (`~`=low, `≈`=medium confidence) |
| `↑` / `→` / `↓` | Trend | Rate acceleration vs last 30 min |
| `8%/h` | Rate | Current consumption rate |
| `!2h30m` | Time to 100% | Estimated time until limit hit (only shown when proj > 80%) |
| `peak-h` | Peak hour | Current hour is historically high-activity |
| `42%ctx` | Context | Context window consumption |
| `85%hit` | Cache | Cache read hit ratio |
| `[BYPASS]` | Bypass | Skip-permissions mode active |

## Summary Mode

```bash
claude-status --summary
```

Prints aggregate statistics from the database (historical rates, activity patterns).
