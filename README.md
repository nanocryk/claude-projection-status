# claude-projection-status

A status line for [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
that replaces the default rate-limit display with usage bars, end-of-window
projections learned from your own working pattern, and the state of the prompt
cache.

```
🕔 2h29m/5h ▰▰▰▰▰▰▰▱▱▱ 22% ⇒  72% 80%/h
🗓️ 4d23h/7d ▰▰▰▰▰▰▰▰▰▰ 31% ⇒ 102% 14%/d ⏰ 4d19h
Opus 5      ▰▰▰▰▰▰▱▱▱▱ 55%ctx  💤 ▰▰▰▰▰ 16s/1h •
```

Line 1 is the 5h rate-limit window, line 2 the 7d one, line 3 the conversation:
the model, how full its context is, and how long the prompt cache has been
decaying.

## What the projection means

Consumption happens while you work, not while time passes. The projection is
therefore an intensity, in percent of the window's budget per **active hour**,
multiplied by the active hours the window still holds:

```
projected = used + intensity * active_hours_remaining
```

The working hours come from a profile of your own week, learned from when usage
actually rose, one hour of the local week at a time. The intensity comes from a
posterior that blends what this window has been watched spending with what past
windows of the same kind spent. Neither number is configurable, and nothing
needs to be filled in: the profile fills itself over the first few days and
keeps following your habits with a four-week half-life.

Wall-clock time never multiplies an intensity, so a burst of work ten minutes
into a fresh week cannot be projected as seven days of it.

`notes/projection-model.md` describes the model in full.

## Reading the line

| Element | Example | Meaning |
|---|---|---|
| Window prefix | `🕔 2h29m/5h` | Time until the window resets. The clock face follows the hour |
| Bar | `▰▰▰▰▰▰▰▱▱▱` | Solid is spent, shaded is projected on top, dim is free |
| Usage | `22%` | Spent now. Yellow past `warning_pct`, red past `critical_pct` |
| Projection | `⇒ 72%` | Expected at reset. Bold when well supported, faint when barely |
| Rate | `80%/h`, `14%/d` | Percent of the budget per working hour, and per day |
| Sparkline | `▁▂▅▃▁▁▂▁` | Usage per bucket across the window |
| Deadline | `⏰ 4d19h` | When the projection crosses 100%. Absent when it does not |
| Context | `55%ctx` | How full the conversation's context window is |
| Idle | `💤 ▰▰▰▰▰ 16s/1h` | Time since the last API call, against the cache lifetime |
| Live | `•` | Something was written to the transcript moments ago |
| Cold | `🥶` | The prompt cache has passed its lifetime |
| Nudge | `👋` | Idle for over half an hour |
| Model mix | `62%o 31%s 7%h` | Share of session tokens per model family. Hidden when only one |
| Subagents | `🐝3 18%` | Subagents spawned, and their share of the session's tokens |
| Bypass | `[BYPASS]` | Permission prompts are being skipped |

The idle bar drains as the cache decays. Its lifetime is read from the
session's own cache writes; until the session has made one, the last lifetime
seen on this machine is used and its tag renders dimmed. When the transcript
cannot be read at all the block stays in place showing `--`, so a missing
reading never looks like a warm cache.

## Install

```bash
cargo install --path .
```

That puts `claude-status` on your PATH. Then point Claude Code at it in
`~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "claude-status",
    "padding": 0,
    "refreshInterval": 10
  }
}
```

## Configuration

Read from `config.json` in your platform's configuration directory, and
overridden by environment variables.

| Platform | Configuration | Database |
|---|---|---|
| Linux | `$XDG_CONFIG_HOME/claude-projection-status` | `$XDG_CACHE_HOME/claude-projection-status` |
| macOS | `~/Library/Application Support/claude-projection-status` | `~/Library/Caches/claude-projection-status` |
| Windows | `%APPDATA%\claude-projection-status` | `%LOCALAPPDATA%\claude-projection-status` |

| Setting | Environment variable | Default | Meaning |
|---|---|---|---|
| `warning_pct` | `CLAUDE_STATUS_WARNING` | `40` | Usage turns yellow here |
| `critical_pct` | `CLAUDE_STATUS_CRITICAL` | `70` | Usage turns red here |
| `show_model_mix` | `CLAUDE_STATUS_MODEL_MIX` | `true` | Read transcripts for the mix and cache indicators |
| `cache_dir` | `CLAUDE_STATUS_CACHE` | platform cache | Where `state.db` lives |
| `projects_root` | `CLAUDE_STATUS_PROJECTS` | `~/.claude/projects` | Where Claude Code files transcripts |
| `retention_days` | `CLAUDE_STATUS_RETENTION` | `14` | Days of raw readings to keep |
| `debug` | `CLAUDE_STATUS_DEBUG` | `false` | Report storage errors on stderr |
| | `CLAUDE_STATUS_CONFIG` | | Read the configuration from this file instead |

The configuration file holds display and location settings only. Nothing about
the projection is tunable by hand.

## Storage

One SQLite database, `state.db`, holding the readings of the current windows,
the activity profile's decayed counters, the verdict for each elapsed hour, the
intensity carried over from finished windows, and what has been learned about
each session. Raw readings are pruned after `retention_days`; the 168-slot
profile is kept and ages by decay rather than deletion. Concurrent sessions
share it under write-ahead logging.

Deleting the database costs the learned profile and nothing else: the tool
starts again from an empty history the next time it runs.

## Other commands

```bash
claude-status --summary
```

Prints what the database holds: how many readings, over what range, the
activity profile as an hour-by-weekday grid, and the intensity priors.

```bash
claude-status check-threshold --session-id <id> --window 5h --threshold 90
```

Prints the session's latest usage for that window when it has reached the
threshold, and nothing otherwise. Intended for hooks that want to act as a
limit approaches. It stays silent on failure rather than reporting a crossing
it cannot verify.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The suite covers the estimator's arithmetic, the profile's estimation, the
storage layer, transcript reading against committed session files, scripted
timelines through the whole refresh path, and a calibration run: a simulated
user works a known pattern for two weeks, and the estimator has to recover the
end-of-window usage that pattern produces without being told any of it.

## Credit

The concept comes from
[leeguooooo/claude-code-usage-bar](https://github.com/leeguooooo/claude-code-usage-bar),
a real-time status line with token usage and burn rate.
