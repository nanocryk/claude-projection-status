//! Unicode glyphs used by the status line.
//!
//! Centralised so escape sequences and raw characters stay apart from the
//! layout code.

/// Consumed bar segment.
pub const FILL: char = '▰';
/// Projected bar segment: same shape as [`FILL`], colour differentiates.
pub const PROJ: char = '▰';
/// Free bar segment.
pub const EMPTY: char = '▱';

/// Spiral calendar pad plus VS16, which forces the emoji presentation.
pub const CALENDAR: &str = "🗓️";
pub const PROJ_ARROW: char = '⇒';
pub const DEADLINE: char = '⏰';
/// Work the remaining budget buys, as opposed to a moment on the clock.
pub const WORK_LEFT: char = '⌛';
/// Budget leaving the window, per unit of time.
pub const RATE: char = '💸';

/// Pace scenes, from barely spending the allowance to burning it well before
/// the window resets. The speed is the pace; the wall is the limit it is
/// heading into.
pub const PACE_ASLEEP: &str = "😴";
pub const PACE_WALK: &str = "🚶";
pub const PACE_RUN: &str = "🏃";
pub const PACE_CYCLE: &str = "🚴";
/// Spending the allowance exactly, with the window resetting as it runs out.
pub const PACE_ON_TARGET: &str = "🎯";
pub const PACE_WALL: &str = "🧱";
pub const PACE_FIRE: &str = "🔥";
pub const PACE_FIRE_TRUCK: &str = "🚒";
pub const PACE_VOLCANO: &str = "🌋";
pub const IDLE: char = '💤';
pub const COLD: char = '🥶';
pub const BEE: char = '🐝';
/// Long-idle nudge.
pub const WAVE: char = '👋';
/// Something was written to the transcript moments ago.
pub const LIVE: char = '•';

/// Clock face for one o'clock; `+k` gives `(k + 1)` o'clock.
const CLOCK_BASE: u32 = 0x1f550;

/// Clock face emoji for a 0..23 hour.
pub fn clock_glyph(hour: u32) -> char {
    let offset = (hour + 11) % 12;
    char::from_u32(CLOCK_BASE + offset).unwrap_or('🕐')
}

#[cfg(test)]
mod tests {
    use super::clock_glyph;

    #[test]
    fn clock_glyph_wraps_like_a_dial() {
        assert_eq!(clock_glyph(1), '🕐');
        assert_eq!(clock_glyph(12), '🕛');
        assert_eq!(clock_glyph(13), '🕐');
        assert_eq!(clock_glyph(0), '🕛');
        assert_eq!(clock_glyph(23), '🕚');
    }
}
