# claude-projection-status

A status line for [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
that replaces the default rate-limit display with usage bars, end-of-window
projections learned from your own working pattern, and the state of the prompt
cache.

```
🕔 2h29m/5h ▨▨▨▨▨▨▨□□□ 22% ➜  72% 💸80%/h 🏃64%
🗓️ 4d23h/7d ▨▨▨▨▨▨▨▨▨▨ 31% ➜ 102% 💸14%/d 🎯103% ⏰Mon 18h
🐶 Opus 5   ▨▨▨▨▨▨□□□□ 55%ctx  💤 ▨▨▨▨▨ 16s/1h •
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

## Pace, and what is left

The rate says how fast the budget is going. The pace says whether that is too
fast: the rate over the fastest rate the remaining budget affords. Under 100%
the window ends with budget to spare, over 100% it runs out early, and 100%
lands exactly on the limit at the reset. Being a ratio it has no unit to
misread, and it stays finite as the window empties.

The glyph beside the number draws the band it falls in, so the shape of the
situation reads before the number does: a speed while the allowance covers the
pace, and something rather less encouraging once it stops covering it. Landing
on the mark, where the allowance runs out exactly as the window resets, has a
glyph of its own. The rest are found by earning them.

Past 100% each window says what is left. The two lines answer different
questions, in the unit that suits their length:

- `⌛0.4h` on the 5h line is an **amount of work**: the budget covers 24 more
  minutes of working. It moves only while work is happening, so a break does
  not shorten it and working faster does.
- `⏰Mon 18h` on the 7d line is a **moment**: the limit lands Monday around
  18:00, if the week is worked the way the profile says it usually is. The
  nights and the weekend sit inside that date and push it out.

Five hours hold no night, so over that span the work left and the moment it
runs out are nearly the same statement. Across a week they are not, and the
date is what a plan hangs on. Each line carries one of the two, never both.

`notes/pace.md` describes both.

## Reading the line

| Element | Example | Meaning |
|---|---|---|
| Window prefix | `🕔 2h29m/5h` | Time until the window resets. The clock face follows the hour |
| Bar | `▨▨▨▨▨▨▨□□□` | Solid is spent, shaded is projected on top, dim is free |
| Usage | `22%` | Spent now. Yellow past `warning_pct`, red past `critical_pct` |
| Projection | `➜ 72%` | Expected at reset. Bold when well supported, faint when barely |
| Rate | `💸80%/h`, `💸14%/d` | Percent of the budget per working hour, and per day |
| Pace | `🏃64%` | The rate over the fastest the remaining budget affords. 100% lands exactly on the limit at reset |
| Work left | `⌛0.4h` | 5h line: hours of work the budget still buys. Idle time does not consume it. Absent below 100% pace |
| Deadline | `⏰Mon 18h` | 7d line: when the limit lands, on the local clock. Absent below 100% pace |
| Context | `55%ctx` | How full the conversation's context window is |
| Idle | `💤 ▨▨▨▨▨ 16s/1h` | Time since the last API call, against the cache lifetime |
| Live | `•` | Something was written to the transcript moments ago |
| Cold | `🥶` | The prompt cache has passed its lifetime |
| Nudge | `👋` | Idle for over half an hour |
| Model mix | `🐶62% 🤡31% 🍤7%` | Share of session tokens per model family. Hidden when only one |
| Subagents | `🐝3–18%` | Subagents spawned, and their share of the session's tokens |
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
| `bar_glyphs` | `CLAUDE_STATUS_BARS` | `▨▨□` | Characters for spent, projected and free |
| `fun_line` | `CLAUDE_STATUS_FUN` | `start` | When to draw the fourth line: `start`, `always` or `never` |
| `fun_phrases` | | | Phrases of your own for that line |
| | `CLAUDE_STATUS_CONFIG` | | Read the configuration from this file instead |

The configuration file holds display and location settings only. Nothing about
the projection is tunable by hand.

### Bar characters

`bar_glyphs` is exactly three characters, in the order spent, projected, free,
and anything else keeps the default rather than drawing a broken bar. All three
bars use them: the two windows, the context and the idle indicator. Spent and
projected are the same character by default, since the colour is what tells
them apart.

```json
{ "bar_glyphs": "▨▨□" }
```

Sets that hold their column in most fonts: `███░` for solid blocks, `▓▓░` for
a lighter texture at the same size, `══─` for rules centred on the text line,
`━━┄` for a thinner pair, and `=+-` for pure ASCII. Block Elements and Box
Drawing are the safest families, since every monospace font draws them one cell
wide. Geometric Shapes such as `▰▱` are missing from many phone fonts, which
substitute a proportional glyph and let the segments overlap.

## The fourth line

Below the three that mean something sits one that does not: an observation
drawn from your own history, or a phrase of your own, changing every five
minutes and alternating between the two. `fun_line` decides when it appears.
The default, `start`, draws it only before the session's first reply, so it
costs a terminal row exactly while there is nothing else to look at.

```json
{
  "fun_line": "always",
  "fun_phrases": ["Reticulating splines", "Consulting the oracle"]
}
```

The facts are computed from the hour-by-hour verdicts, the activity profile,
the sessions table, finished windows and the model mix of the session in front
of you. One with nothing to say stays quiet, so a young database offers fewer
of them. Each carries the mark of its register, `👀` for what the week says
about you, `😎` for what it says in your favour and `🧾` for where the tokens
went, and your own phrases are indented to match. `notes/fun-line.md` describes
the whole arrangement.

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
