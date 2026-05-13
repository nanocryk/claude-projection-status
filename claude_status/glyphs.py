"""Unicode glyphs used by the status line renderer.

Centralised here so the rest of the code can refer to them by name. Keeping
escape sequences and raw chars apart in one file avoids mixed-form edit
hazards in render.py.
"""

# Bar segment chars (matches the two-tone usage bar)
FILL = "▰"      # ▰ black parallelogram
PROJ = "▰"      # ▰ (same as FILL; colour differentiates)
EMPTY = "▱"     # ▱ white parallelogram

# Sparkline level chars, lowest to highest
SPARK_LEVELS = "▁▂▃▄▅▆▇█"

# Window glyphs
CALENDAR = "🗓️"   # spiral calendar pad + VS16 (forces emoji)
CLOCK_BASE = 0x1f550             # 🕐 (1 o'clock); +k for (k+1) o'clock


def clock_glyph(hour: int) -> str:
    """Return the clock face emoji for the given 0-23 hour."""
    return chr(CLOCK_BASE + ((hour - 1) % 12))


# Inline markers
PROJ_ARROW = "⇒"   # ⇒ double rightwards arrow
DEADLINE = "⏰"     # ⏰ alarm clock emoji

# Idle / peak markers
IDLE = "💤"     # 💤 ZZZ
COLD = "🥶"     # 🥶 freezing face
BEE = "🐝"     # 🐝 subagent indicator
WAVE = "👋"     # 👋 long-idle nudge

# Non-breaking space: used for line indentation that must survive
# Claude Code's per-line leading-whitespace strip.
NBSP = " "

# White foreground (used for "consumed" bar segment)
FG_WHITE = "\033[97m"
