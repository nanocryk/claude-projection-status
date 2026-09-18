//! Unicode glyphs used by the status line.
//!
//! Centralised so escape sequences and raw characters stay apart from the
//! layout code.

/// Consumed bar segment.
///
/// Geometric Shapes carry an ambiguous cell width, so a font missing them is
/// substituted from a proportional one and the segments overlap. Squares are
/// covered widely enough to survive that, unlike the rarer shapes of the same
/// block, and they sit on the text's own line rather than filling the cell.
/// Overridable through the `bar_glyphs` setting.
pub const FILL: char = '▨';
/// Projected bar segment: same shape as [`FILL`], colour differentiates.
pub const PROJ: char = '▨';
/// Free bar segment.
pub const EMPTY: char = '□';

/// Spiral calendar pad plus VS16, which forces the emoji presentation.
pub const CALENDAR: &str = "🗓️";
const BEACH: &str = "🏖️";
const PUMPKIN: &str = "🎃";
const TREE: &str = "🎄";

/// What the week's window wears today.
///
/// The calendar knows the date, so it may as well say something about it: a
/// beach at the weekend, and a couple of days a year that speak for
/// themselves. `weekday` counts from Monday.
pub fn calendar_glyph(month: u32, day: u32, weekday: u8) -> &'static str {
    match (month, day) {
        (10, 31) => PUMPKIN,
        (12, 19..=25) => TREE,
        _ if weekday >= 5 => BEACH,
        _ => CALENDAR,
    }
}
pub const PROJ_ARROW: char = '➜';
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

/// Registers the fourth line's observations fall into: what the week says
/// about you, what it says in your favour, and where the tokens went.
pub const FACT_GUILT: &str = "👀";
pub const FACT_VANITY: &str = "😎";
pub const FACT_ACCOUNTING: &str = "🧾";

/// Stand-in for a model family, by the letter the transcript reader gives it.
///
/// A family with no glyph of its own keeps its name and its letter.
pub fn family_glyph(family: &str) -> Option<&'static str> {
    match family {
        "o" => Some("🐶"),
        "s" => Some("🤡"),
        "h" => Some("🍤"),
        "f" => Some("🗿"),
        // What the transcript reader calls a model it does not recognise.
        "?" => Some("👽"),
        _ => None,
    }
}

/// Clock face for one o'clock; `+k` gives `(k + 1)` o'clock.
const CLOCK_BASE: u32 = 0x1f550;

/// Clock face emoji for a 0..23 hour.
pub fn clock_glyph(hour: u32) -> char {
    let offset = (hour + 11) % 12;
    char::from_u32(CLOCK_BASE + offset).unwrap_or('🕐')
}

#[cfg(test)]
mod tests {
    use super::{CALENDAR, calendar_glyph, clock_glyph};

    #[test]
    fn the_calendar_dresses_for_the_occasion() {
        // A Wednesday in March, a Saturday, and the two days of the year.
        assert_eq!(calendar_glyph(3, 11, 2), CALENDAR);
        assert_eq!(calendar_glyph(3, 14, 5), "🏖️");
        assert_eq!(calendar_glyph(10, 31, 2), "🎃");
        assert_eq!(calendar_glyph(12, 24, 3), "🎄");
        // The season outranks the weekend.
        assert_eq!(calendar_glyph(12, 20, 6), "🎄");
    }

    #[test]
    fn clock_glyph_wraps_like_a_dial() {
        assert_eq!(clock_glyph(1), '🕐');
        assert_eq!(clock_glyph(12), '🕛');
        assert_eq!(clock_glyph(13), '🕐');
        assert_eq!(clock_glyph(0), '🕛');
        assert_eq!(clock_glyph(23), '🕚');
    }
}
