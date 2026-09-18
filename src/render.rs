//! Status line layout.
//!
//! Three lines: the 5h window, the 7d window, then the model with its context
//! bar. The bars on all three start at the same column. A fourth is drawn
//! below them when there is something frivolous to put on it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use chrono::{Datelike as _, TimeZone, Timelike as _};
use serde::Deserialize;

use crate::color;
use crate::estimate::Confidence;
use crate::glyphs;
use crate::slots;
use crate::transcript::{self, FAMILY_ORDER};
use crate::units::{Pct, Timestamp};
use crate::window::WindowKind;

const BAR_WIDTH: usize = 10;
/// Visible width of the `⇒ NNN%` column. Held as blanks when a window has no
/// projection, so the segments after it stay aligned with the other window.
const PROJ_COLUMN_WIDTH: usize = 6;
const IDLE_BAR_WIDTH: usize = 5;
/// Idle past the cache TTL earns the long-idle nudge after this long.
const NUDGE_AFTER_SEC: f64 = 1800.0;
/// Pace at which the figure turns yellow, short of the limit it is heading for.
const PACE_WARN: f64 = 0.8;
/// Band around 100% that counts as spending the allowance exactly. Landing on
/// the mark is the best use of a window, so it reads as a warning rather than
/// as an alarm.
const PACE_ON_TARGET: std::ops::RangeInclusive<f64> = 0.97..=1.03;
/// A pace reads as a warning long before this, and the column has a width.
const PACE_DISPLAY_MAX: f64 = 999.0;

/// Everything one window contributes to the line it owns.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WindowView {
    pub pct: Option<Pct>,
    pub projected: Option<Pct>,
    /// Preformatted time until the window resets.
    pub cooldown: String,
    /// Preformatted moment at which usage is projected to reach 100%.
    pub time_to_100: Option<String>,
    /// Preformatted work the remaining budget still buys.
    pub work_left: Option<String>,
    pub confidence: Option<Confidence>,
    /// Percent per hour for the 5h window, per day for the 7d one.
    pub rate: Option<f64>,
    /// Intensity over the one the remaining budget affords, where 1.0 lands
    /// exactly on the limit at reset.
    pub pace: Option<f64>,
    /// Shown in place of a projection while one is not yet available.
    pub proj_eta: Option<String>,
}

/// Everything the status line draws.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct StatusView {
    pub five_hour: WindowView,
    pub seven_day: WindowView,
    pub model: String,
    pub ctx_pct: Option<f64>,
    pub ctx_size: u64,
    pub bypass: bool,
    /// Share of session tokens per model family, keyed by family letter.
    pub model_shares: BTreeMap<String, f64>,
    pub subagent_count: u32,
    pub subagent_share: f64,
    /// Absent when there is no session to report on at all.
    pub idle: Option<IdleView>,
    /// The fourth line: a fact or a phrase, of no operational value.
    pub fun: Option<String>,
}

/// Time since the prompt cache was last written, against the lifetime it was
/// written with.
#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct IdleView {
    /// Absent when the session transcript could not be read: the block keeps
    /// its place rather than disappearing, which would read as a warm cache.
    pub seconds: Option<f64>,
    pub cache_ttl: Option<u32>,
    /// The lifetime was carried over from an earlier session rather than
    /// measured in this one.
    pub ttl_inherited: bool,
    /// Something was written to the transcript moments ago.
    pub live: bool,
}

/// The three characters every bar is drawn with.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Bars {
    pub filled: char,
    /// Usually the same character as [`Bars::filled`], since the colour is
    /// what tells the two apart.
    pub projected: char,
    pub free: char,
}

impl Default for Bars {
    fn default() -> Self {
        Self {
            filled: glyphs::FILL,
            projected: glyphs::PROJ,
            free: glyphs::EMPTY,
        }
    }
}

impl Bars {
    /// Three characters: spent, projected, free.
    ///
    /// Anything else keeps the default, since a half-written setting should
    /// not produce a half-drawn bar.
    pub fn parse(text: &str) -> Option<Self> {
        let mut chars = text.chars();
        let filled = chars.next()?;
        let projected = chars.next()?;
        let free = chars.next()?;
        chars.next().is_none().then_some(Self {
            filled,
            projected,
            free,
        })
    }
}

/// Values that come from the environment rather than from the payload.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct RenderCtx {
    pub local_hour: u32,
    pub warning_pct: f64,
    pub critical_pct: f64,
    pub bars: Bars,
}

impl Default for RenderCtx {
    fn default() -> Self {
        Self {
            local_hour: 0,
            warning_pct: 40.0,
            critical_pct: 70.0,
            bars: Bars::default(),
        }
    }
}

/// Projection thresholds, looser on the 7d window where a high number is normal.
fn proj_thresholds(kind: WindowKind) -> (f64, f64) {
    match kind {
        WindowKind::FiveHour => (75.0, 90.0),
        WindowKind::SevenDay => (85.0, 95.0),
    }
}

/// Length of a string as the terminal shows it: ANSI sequences and the
/// variation selector that forces emoji presentation take no columns.
pub fn visible_len(text: &str) -> usize {
    let mut count = 0;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            '\u{fe0f}' => {}
            _ => count += 1,
        }
    }
    count
}

/// Columns a string occupies in a terminal. Emoji take two, which is why the
/// model name on line 3 is padded one column past the window prefixes: both
/// window glyphs are emoji, and [`visible_len`] counts them as one.
pub fn display_width(text: &str) -> usize {
    let mut width = 0;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            // Variation selector, zero-width joiner, skin tone modifier.
            '\u{fe0f}' | '\u{200d}' | '\u{1f3fb}'..='\u{1f3ff}' => {}
            '\u{1f000}'..='\u{1faff}'
            | '\u{231a}'..='\u{231b}'
            | '\u{23e9}'..='\u{23fa}'
            | '\u{2614}'..='\u{2615}' => width += 2,
            _ => width += 1,
        }
    }
    width
}

fn color_for_pct(pct: f64, ctx: &RenderCtx) -> &'static str {
    if pct >= ctx.critical_pct {
        color::RED
    } else if pct >= ctx.warning_pct {
        color::YELLOW
    } else {
        color::GREEN
    }
}

fn fg_for_proj(pct: f64, warn: f64, crit: f64) -> &'static str {
    if pct >= crit {
        color::RED
    } else if pct >= warn {
        color::YELLOW
    } else {
        color::GREEN
    }
}

fn colored_pct(pct: f64, ctx: &RenderCtx) -> String {
    let text = format!("{pct:.0}%");
    format!("{}{text:>4}{}", color_for_pct(pct, ctx), color::RESET)
}

/// Solid for what is used, shaded for what is projected on top, dim for free.
fn build_two_tone_bar(
    pct: f64,
    projected: Option<f64>,
    warn: f64,
    crit: f64,
    bars: Bars,
) -> String {
    let mut filled = (pct.clamp(0.0, 100.0) / 100.0 * BAR_WIDTH as f64 + 0.5) as usize;
    if pct > 0.0 && filled == 0 {
        filled = 1;
    }
    let proj_filled = projected
        .map(|value| (value.clamp(0.0, 100.0) / 100.0 * BAR_WIDTH as f64 + 0.5) as usize)
        .map(|total| total.saturating_sub(filled))
        .unwrap_or(0);
    let empty = BAR_WIDTH.saturating_sub(filled + proj_filled);

    // A projection of exactly zero carries no colour of its own.
    let proj_color = fg_for_proj(
        projected.filter(|value| *value != 0.0).unwrap_or(pct),
        warn,
        crit,
    );

    let mut bar = String::new();
    if filled > 0 {
        let _ = write!(
            bar,
            "{}{}{}",
            color::FG_WHITE,
            bars.filled.to_string().repeat(filled),
            color::RESET
        );
    }
    if proj_filled > 0 {
        let _ = write!(
            bar,
            "{}{}{}",
            proj_color,
            bars.projected.to_string().repeat(proj_filled),
            color::RESET
        );
    }
    if empty > 0 {
        let _ = write!(
            bar,
            "{}{}{}",
            color::FAINT,
            bars.free.to_string().repeat(empty),
            color::RESET
        );
    }
    bar
}

fn format_rate(kind: WindowKind, rate: Option<f64>) -> String {
    let Some(rate) = rate else {
        return String::new();
    };
    let (unit, quiet_below, warn_above, crit_above) = match kind {
        WindowKind::FiveHour => ("%/h", 0.5, 15.0, 30.0),
        WindowKind::SevenDay => ("%/d", 1.0, 10.0, 20.0),
    };
    if rate < quiet_below {
        return format!(
            "{}{}{rate:.1}{unit}{}",
            color::DIM,
            glyphs::RATE,
            color::RESET
        );
    }
    let rate_color = if rate < warn_above {
        color::GREEN
    } else if rate < crit_above {
        color::YELLOW
    } else {
        color::RED
    };
    format!(
        "{rate_color}{}{rate:.0}{unit}{}",
        glyphs::RATE,
        color::RESET
    )
}

/// A speed while the allowance covers the pace, and how badly it does not once
/// it stops covering it.
fn pace_scene(pace: f64) -> &'static str {
    if pace < 0.20 {
        glyphs::PACE_ASLEEP
    } else if pace < 0.50 {
        glyphs::PACE_WALK
    } else if pace < 0.85 {
        glyphs::PACE_RUN
    } else if pace < *PACE_ON_TARGET.start() {
        glyphs::PACE_CYCLE
    } else if PACE_ON_TARGET.contains(&pace) {
        glyphs::PACE_ON_TARGET
    } else if pace < 1.5 {
        glyphs::PACE_WALL
    } else if pace < 2.5 {
        glyphs::PACE_FIRE
    } else if pace < 4.0 {
        glyphs::PACE_FIRE_TRUCK
    } else {
        glyphs::PACE_VOLCANO
    }
}

/// The intensity being kept, against the one the remaining budget affords.
/// Unitless on purpose: 100% is the pace that lands exactly on the limit, and
/// the figure stays finite as the window empties.
fn format_pace(pace: f64) -> String {
    let percent = (pace * 100.0).min(PACE_DISPLAY_MAX);
    let tint = if pace > *PACE_ON_TARGET.end() {
        color::RED
    } else if pace >= PACE_WARN {
        color::YELLOW
    } else {
        color::GREEN
    };
    format!("{tint}{}{percent:.0}%{}", pace_scene(pace), color::RESET)
}

/// Work a budget still buys, in the hours the estimator counts rather than as
/// a countdown: idle time does not consume it.
pub fn format_work_left(hours: f64) -> String {
    if hours >= 10.0 {
        format!("{hours:.0}h")
    } else {
        format!("{hours:.1}h")
    }
}

fn format_window(
    kind: WindowKind,
    view: &WindowView,
    ctx: &RenderCtx,
    prefix_width: usize,
) -> String {
    let Some(pct) = view.pct else {
        return format!("{}[--] {}: --%{}", color::DIM, kind.label(), color::RESET);
    };
    let (warn, crit) = proj_thresholds(kind);

    let glyph = match kind {
        WindowKind::FiveHour => glyphs::clock_glyph(ctx.local_hour).to_string(),
        WindowKind::SevenDay => glyphs::CALENDAR.to_string(),
    };
    let mut prefix = format!(
        "{}{glyph} {}/{}{}",
        color::DIM,
        view.cooldown,
        kind.label(),
        color::RESET
    );
    // Columns, not characters: the clock and the calendar are emoji.
    let pad = prefix_width.saturating_sub(display_width(&prefix));
    prefix.push_str(&" ".repeat(pad));

    let bar = build_two_tone_bar(
        pct.get(),
        view.projected.map(Pct::get),
        warn,
        crit,
        ctx.bars,
    );
    let mut parts = vec![format!("{prefix} {bar}{}", colored_pct(pct.get(), ctx))];

    if let Some(projected) = view.projected {
        let emphasis = match view.confidence {
            Some(Confidence::High) => color::BOLD,
            Some(Confidence::Low) => color::FAINT,
            _ => "",
        };
        let text = format!("{:.0}%", projected.get());
        parts.push(format!(
            "{}{}{} {emphasis}{}{text:>4}{}",
            color::DIM,
            glyphs::PROJ_ARROW,
            color::RESET,
            fg_for_proj(projected.get(), warn, crit),
            color::RESET
        ));
    } else if let Some(eta) = &view.proj_eta {
        parts.push(format!(
            "{}{} {eta:>4}{}",
            color::DIM,
            glyphs::PROJ_ARROW,
            color::RESET
        ));
    } else {
        parts.push(" ".repeat(PROJ_COLUMN_WIDTH));
    }

    let rate = format_rate(kind, view.rate);
    if !rate.is_empty() {
        parts.push(rate);
    }

    if let Some(pace) = view.pace {
        parts.push(format_pace(pace));
    }

    // One time-shaped figure per window: the work a budget buys on the 5h
    // line, the moment it runs out on the 7d one.
    if let Some(work_left) = &view.work_left {
        parts.push(format!(
            "{}{}{}{work_left}{}",
            color::BOLD,
            color::RED,
            glyphs::WORK_LEFT,
            color::RESET
        ));
    }

    if let Some(deadline) = &view.time_to_100 {
        parts.push(format!(
            "{}{}{}{deadline}{}",
            color::BOLD,
            color::RED,
            glyphs::DEADLINE,
            color::RESET
        ));
    }

    // The projection column holds blanks to keep the two windows aligned; with
    // nothing after it on this line they are only trailing whitespace.
    parts.join(" ").trim_end().to_string()
}

/// Time since the conversation last yielded to the user, against the prompt
/// cache TTL: the bar drains as the cache decays, and the tag names the TTL
/// the client is using.
fn format_idle(idle: &IdleView, bars: Bars) -> String {
    let Some(idle_sec) = idle.seconds else {
        // Nothing to anchor on. The block holds its place with an empty bar,
        // so an unreadable transcript cannot be mistaken for a warm cache.
        return format!(
            "{} {}{}{} {} --{}",
            glyphs::IDLE,
            color::FAINT,
            bars.free.to_string().repeat(IDLE_BAR_WIDTH),
            color::RESET,
            color::DIM,
            color::RESET
        );
    };
    let cache_ttl = idle.cache_ttl;
    let total = idle_sec.max(0.0) as u64;
    let time_str = if total >= 3600 {
        format!("{}h{:02}m", total / 3600, (total % 3600) / 60)
    } else if total >= 60 {
        format!("{}m", total / 60)
    } else {
        format!("{total:02}s")
    };

    let ttl = match cache_ttl {
        Some(3600) => 3600.0,
        _ => 300.0,
    };
    let ttl_tag = match cache_ttl {
        Some(3600) => "/1h",
        Some(300) => "/5m",
        _ => "",
    };

    let (glyph, tint, bold) = if idle_sec >= ttl {
        (glyphs::COLD, color::COLD_BLUE, color::BOLD)
    } else if idle_sec >= ttl * 0.8 {
        (glyphs::IDLE, color::RED, "")
    } else if idle_sec >= ttl * 0.6 {
        (glyphs::IDLE, color::YELLOW, "")
    } else {
        (glyphs::IDLE, color::DIM, "")
    };

    // Cells count time left before the cache goes cold, so the bar drains.
    let filled = if idle_sec < ttl {
        ((1.0 - idle_sec / ttl).max(0.0) * IDLE_BAR_WIDTH as f64 + 0.5) as usize
    } else {
        0
    };
    let empty = IDLE_BAR_WIDTH.saturating_sub(filled);
    let mut bar = String::new();
    if filled > 0 {
        let _ = write!(
            bar,
            "{tint}{}{}",
            bars.filled.to_string().repeat(filled),
            color::RESET
        );
    }
    if empty > 0 {
        let _ = write!(
            bar,
            "{}{}{}",
            color::FAINT,
            bars.free.to_string().repeat(empty),
            color::RESET
        );
    }
    let nudge = if idle_sec >= NUDGE_AFTER_SEC {
        format!(" {}", glyphs::WAVE)
    } else {
        String::new()
    };
    // A lifetime carried over from another session is dimmed, so a measured
    // one is distinguishable at a glance.
    let tag = if idle.ttl_inherited && !ttl_tag.is_empty() {
        format!("{}{}{ttl_tag}{}", color::RESET, color::DIM, color::RESET)
    } else {
        format!("{ttl_tag}{}", color::RESET)
    };
    // Derived from the newest record's age, so it cannot stay stuck on.
    let live = if idle.live {
        format!(" {}{}{}", color::DIM, glyphs::LIVE, color::RESET)
    } else {
        String::new()
    };
    format!("{glyph} {bar} {bold}{tint}{time_str}{tag}{live}{nudge}")
}

fn build_ctx_bar(ctx_pct: f64, bars: Bars) -> String {
    let mut filled = (ctx_pct.clamp(0.0, 100.0) / 100.0 * BAR_WIDTH as f64 + 0.5) as usize;
    if ctx_pct > 0.0 && filled == 0 {
        filled = 1;
    }
    let empty = BAR_WIDTH.saturating_sub(filled);
    let tint = ctx_color(ctx_pct);
    let mut bar = String::new();
    if filled > 0 {
        let _ = write!(
            bar,
            "{tint}{}{}",
            bars.filled.to_string().repeat(filled),
            color::RESET
        );
    }
    if empty > 0 {
        let _ = write!(
            bar,
            "{}{}{}",
            color::FAINT,
            bars.free.to_string().repeat(empty),
            color::RESET
        );
    }
    bar
}

fn ctx_color(ctx_pct: f64) -> &'static str {
    if ctx_pct < 50.0 {
        color::GREEN
    } else if ctx_pct < 80.0 {
        color::YELLOW
    } else {
        color::RED
    }
}

fn family_color(family: &str) -> &'static str {
    match family {
        "s" => color::YELLOW,
        "h" => color::GREEN,
        "f" => color::COLD_BLUE,
        _ => color::DIM,
    }
}

fn round_to_pct(share: f64) -> i64 {
    (share * 100.0).round_ties_even() as i64
}

/// Per-family token shares plus the subagent count. Shares stay hidden while
/// one family holds everything, since there is no mix to convey.
fn format_model_stats(
    shares: &BTreeMap<String, f64>,
    subagent_count: u32,
    subagent_share: f64,
) -> Vec<String> {
    let mut parts = Vec::new();

    if shares.len() >= 2 {
        let visible: BTreeMap<&str, i64> = shares
            .iter()
            .map(|(family, share)| (family.as_str(), round_to_pct(*share)))
            .filter(|(_, pct)| *pct > 0)
            .collect();
        if visible.len() >= 2 {
            for family in FAMILY_ORDER {
                if let Some(pct) = visible.get(family) {
                    // The letter stands in for a family with no glyph.
                    let mark = glyphs::family_glyph(family).unwrap_or(family);
                    parts.push(format!(
                        "{}{mark}{pct}%{}",
                        family_color(family),
                        color::RESET
                    ));
                }
            }
            for (family, pct) in &visible {
                if !FAMILY_ORDER.contains(family) {
                    let mark = glyphs::family_glyph(family).unwrap_or(family);
                    parts.push(format!("{}{mark}{pct}%{}", color::DIM, color::RESET));
                }
            }
        }
    }

    if subagent_count > 0 {
        let share_pct = round_to_pct(subagent_share);
        // Joined rather than spaced: the count and the share are one fact.
        let suffix = if share_pct > 0 {
            format!("–{share_pct}%")
        } else {
            String::new()
        };
        parts.push(format!(
            "{}{}{subagent_count}{suffix}{}",
            color::DIM,
            glyphs::BEE,
            color::RESET
        ));
    }

    parts
}

/// Drop a parenthesised context note from a model name, as in
/// `Opus 5 (1M context)`.
fn strip_context_note(model: &str) -> String {
    let mut out = String::with_capacity(model.len());
    let mut rest = model;
    while let Some(open) = rest.find('(') {
        let Some(close_offset) = rest[open..].find(')') else {
            break;
        };
        let close = open + close_offset;
        if !rest[open..close].contains("context") {
            out.push_str(&rest[..=close]);
            rest = &rest[close + 1..];
            continue;
        }
        out.push_str(rest[..open].trim_end_matches([' ', '\t', '\n', '\r']));
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The moment the limit is reached, as a local weekday and hour.
///
/// A date rather than a span: a countdown would read as a second timer beside
/// the window's own, and the hour is as precise as the estimate deserves.
pub fn format_deadline<Tz: TimeZone>(at: Timestamp, zone: &Tz) -> String {
    let moment = slots::local(at, zone);
    format!("{} {:02}h", moment.weekday(), moment.hour())
}

/// Time until a window resets, right-aligned in the five columns the prefix
/// reserves for it.
pub fn format_cooldown(resets_at: Option<Timestamp>, now: Timestamp, use_days: bool) -> String {
    let Some(resets_at) = resets_at else {
        return "   --".to_string();
    };
    let total = resets_at.seconds_since(now).max(0.0) as u64;
    let days = total / 86400;
    let hours = (total % 86400) / 3600;
    let minutes = (total % 3600) / 60;
    let text = if use_days && days > 0 {
        format!("{days}d{hours:02}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m")
    };
    format!("{text:>5}")
}

pub fn render_status_line(view: &StatusView, ctx: &RenderCtx) -> String {
    // Model name stays left-flush so it survives the leading-whitespace strip.
    let name = strip_context_note(&view.model);
    // The display name carries the family too, in the case the ids do not use.
    let mark = transcript::family_of(&name.to_lowercase()).and_then(glyphs::family_glyph);
    let head = match mark {
        Some(mark) => format!("{mark} {}{name}{}", color::DIM, color::RESET),
        None => format!("{}{name}{}", color::DIM, color::RESET),
    };

    // One column for all three bars, wide enough for whichever line needs the
    // most. A long model name therefore moves every bar together rather than
    // stepping its own out of line.
    let prefix_5h = format!("🕒 {}/5h", view.five_hour.cooldown);
    let prefix_7d = format!("{} {}/7d", glyphs::CALENDAR, view.seven_day.cooldown);
    let prefix_width = display_width(&prefix_5h)
        .max(display_width(&prefix_7d))
        .max(display_width(&head));

    let mut line1 = format_window(WindowKind::FiveHour, &view.five_hour, ctx, prefix_width);
    if view.bypass {
        let _ = write!(
            line1,
            " {}{}[BYPASS]{}",
            color::BOLD,
            color::RED,
            color::RESET
        );
    }

    let line2 = format_window(WindowKind::SevenDay, &view.seven_day, ctx, prefix_width);

    let pad = (prefix_width + 1)
        .saturating_sub(display_width(&head))
        .max(1);
    let mut line3_parts: Vec<String> = Vec::new();
    if let Some(ctx_pct) = view.ctx_pct.filter(|_| view.ctx_size > 0) {
        line3_parts.push(format!(
            "{} {}{ctx_pct:.0}%ctx{}",
            build_ctx_bar(ctx_pct, ctx.bars),
            ctx_color(ctx_pct),
            color::RESET
        ));
    }
    // Cache decay belongs with the conversation's state rather than with the
    // rate limits, and line 1 is the one that overflows a narrow terminal.
    if let Some(idle) = view.idle.as_ref() {
        line3_parts.push(format_idle(idle, ctx.bars));
    }
    let model_stats =
        format_model_stats(&view.model_shares, view.subagent_count, view.subagent_share);
    if !model_stats.is_empty() {
        line3_parts.push(model_stats.join(" "));
    }
    let line3 = format!("{head}{}{}", " ".repeat(pad), line3_parts.join("  "));

    let mut out = format!("{line1}\n{line2}\n{line3}");
    // Further back than the rest of the line: nothing here is worth reading
    // before the three above it.
    if let Some(fun) = view.fun.as_ref().filter(|text| !text.trim().is_empty()) {
        let _ = write!(out, "\n{}{}{}", color::DIMMER, fun.trim(), color::RESET);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Bars, Confidence, IdleView, RenderCtx, StatusView, WindowView, build_two_tone_bar,
        display_width, format_cooldown, format_deadline, format_window, pace_scene,
        render_status_line, strip_context_note, visible_len,
    };
    use crate::units::{Pct, Timestamp};
    use crate::window::WindowKind;
    use chrono::FixedOffset;

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
            out.push(ch);
        }
        out
    }

    #[test]
    fn visible_len_ignores_ansi_and_variation_selectors() {
        assert_eq!(visible_len("\x1b[38;5;249mabc\x1b[0m"), 3);
        assert_eq!(visible_len("🗓️"), 1);
        assert_eq!(visible_len("🕒 4h32/5h"), 9);
    }

    #[test]
    fn bar_segments_always_total_the_bar_width() {
        for pct in [0.0, 0.4, 15.0, 99.9, 100.0, 140.0] {
            for projected in [None, Some(0.0), Some(12.0), Some(100.0), Some(310.0)] {
                let bar = build_two_tone_bar(pct, projected, 75.0, 90.0, Bars::default());
                assert_eq!(
                    visible_len(&bar),
                    10,
                    "pct {pct} projected {projected:?} produced {bar:?}"
                );
            }
        }
    }

    #[test]
    fn a_nonzero_usage_always_lights_one_cell() {
        let bar = strip_ansi(&build_two_tone_bar(0.4, None, 75.0, 90.0, Bars::default()));
        assert!(bar.starts_with('▨'), "{bar:?}");
    }

    #[test]
    fn cooldown_is_right_aligned_in_five_columns() {
        let now = Timestamp::new(1_000_000.0);
        assert_eq!(format_cooldown(None, now, false), "   --");
        assert_eq!(
            format_cooldown(Some(Timestamp::new(1_000_000.0 + 16_320.0)), now, false),
            "4h32m"
        );
        assert_eq!(
            format_cooldown(Some(Timestamp::new(1_000_000.0 + 1_800.0)), now, false),
            "  30m"
        );
        assert_eq!(
            format_cooldown(Some(Timestamp::new(1_000_000.0 + 525_600.0)), now, true),
            "6d02h"
        );
        assert_eq!(
            format_cooldown(Some(Timestamp::new(999_000.0)), now, false),
            "   0m"
        );
    }

    #[test]
    fn a_window_without_usage_renders_a_placeholder() {
        let line = format_window(
            WindowKind::FiveHour,
            &WindowView::default(),
            &RenderCtx::default(),
            10,
        );
        assert_eq!(strip_ansi(&line), "[--] 5h: --%");
    }

    #[test]
    fn the_projection_column_holds_its_width_when_empty() {
        // Both carry a rate, so what is measured is whether the segment after
        // the column lands in the same place either way.
        let with_projection = format_window(
            WindowKind::FiveHour,
            &WindowView {
                pct: Some(Pct::new(15.0)),
                projected: Some(Pct::new(23.0)),
                cooldown: "4h32m".to_string(),
                confidence: Some(Confidence::Medium),
                rate: Some(8.0),
                ..WindowView::default()
            },
            &RenderCtx::default(),
            10,
        );
        let without = format_window(
            WindowKind::FiveHour,
            &WindowView {
                pct: Some(Pct::new(15.0)),
                cooldown: "4h32m".to_string(),
                rate: Some(8.0),
                ..WindowView::default()
            },
            &RenderCtx::default(),
            10,
        );
        assert_eq!(visible_len(&with_projection), visible_len(&without));
    }

    #[test]
    fn a_line_ending_at_the_projection_column_carries_no_trailing_blanks() {
        let line = format_window(
            WindowKind::SevenDay,
            &WindowView {
                pct: Some(Pct::new(3.0)),
                cooldown: "5d20h".to_string(),
                ..WindowView::default()
            },
            &RenderCtx::default(),
            10,
        );
        assert_eq!(strip_ansi(&line), "🗓️ 5d20h/7d ▨□□□□□□□□□  3%");
    }

    #[test]
    fn the_fourth_line_is_drawn_only_when_there_is_something_on_it() {
        let bare = StatusView {
            model: "Opus 5".to_string(),
            ..StatusView::default()
        };
        assert_eq!(
            render_status_line(&bare, &RenderCtx::default())
                .lines()
                .count(),
            3
        );

        let with_fun = StatusView {
            fun: Some("You have worked 71 of the last 168 hours.".to_string()),
            ..bare
        };
        let rendered = render_status_line(&with_fun, &RenderCtx::default());
        assert_eq!(rendered.lines().count(), 4);
        assert_eq!(
            strip_ansi(rendered.lines().nth(3).expect("a fourth line")),
            "You have worked 71 of the last 168 hours."
        );
    }

    #[test]
    fn context_notes_are_dropped_from_the_model_name() {
        assert_eq!(strip_context_note("Opus 5 (1M context)"), "Opus 5");
        assert_eq!(strip_context_note("Sonnet 5"), "Sonnet 5");
        assert_eq!(strip_context_note("Fable (preview)"), "Fable (preview)");
        assert_eq!(
            strip_context_note("Opus 5 (1M context) build (beta)"),
            "Opus 5 build (beta)"
        );
    }

    #[test]
    fn the_pace_and_the_work_left_follow_the_rate() {
        let line = format_window(
            WindowKind::FiveHour,
            &WindowView {
                pct: Some(Pct::new(81.0)),
                projected: Some(Pct::new(126.0)),
                cooldown: "1h12m".to_string(),
                rate: Some(45.0),
                // (126 - 81) / (100 - 81), the projection restated.
                pace: Some(2.37),
                work_left: Some("0.4h".to_string()),
                confidence: Some(Confidence::High),
                ..WindowView::default()
            },
            &RenderCtx::default(),
            10,
        );
        assert_eq!(
            strip_ansi(&line),
            "🕛 1h12m/5h ▨▨▨▨▨▨▨▨▨▨ 81% ⇒ 126% 💸45%/h 🔥237% ⌛0.4h"
        );
    }

    #[test]
    fn a_bar_setting_needs_exactly_three_characters() {
        let bars = Bars::parse("▨▨□").expect("three characters");
        assert_eq!((bars.filled, bars.projected, bars.free), ('▨', '▨', '□'));
        assert!(Bars::parse("▨□").is_none());
        assert!(Bars::parse("▨▨□□").is_none());
        assert!(Bars::parse("").is_none());
    }

    #[test]
    fn the_bars_are_drawn_with_the_configured_characters() {
        let ctx = RenderCtx {
            bars: Bars::parse("=+-").expect("three characters"),
            ..RenderCtx::default()
        };
        let line = format_window(
            WindowKind::FiveHour,
            &WindowView {
                pct: Some(Pct::new(22.0)),
                projected: Some(Pct::new(46.0)),
                cooldown: "2h07m".to_string(),
                ..WindowView::default()
            },
            &ctx,
            10,
        );
        let plain = strip_ansi(&line);
        assert!(plain.contains("==+++-----"), "{plain}");
    }

    #[test]
    fn the_pace_scene_follows_the_band() {
        let scenes: Vec<&str> = [0.05, 0.3, 0.7, 0.9, 1.0, 1.2, 2.0, 3.0, 9.0]
            .iter()
            .map(|pace| pace_scene(*pace))
            .collect();
        assert_eq!(
            scenes,
            vec!["😴", "🚶", "🏃", "🚴", "🎯", "🧱", "🔥", "🚒", "🌋"]
        );
    }

    #[test]
    fn a_pace_inside_the_budget_hides_the_work_left() {
        let line = format_window(
            WindowKind::FiveHour,
            &WindowView {
                pct: Some(Pct::new(22.0)),
                projected: Some(Pct::new(46.0)),
                cooldown: "2h07m".to_string(),
                rate: Some(45.0),
                pace: Some(0.31),
                ..WindowView::default()
            },
            &RenderCtx::default(),
            10,
        );
        assert_eq!(
            strip_ansi(&line),
            "🕛 2h07m/5h ▨▨▨▨▨□□□□□ 22% ⇒  46% 💸45%/h 🚶31%"
        );
    }

    #[test]
    fn the_deadline_reads_as_a_local_moment() {
        // 2026-09-14 16:00 UTC, a Monday.
        let at = Timestamp::new(1_789_401_600.0);
        let paris = FixedOffset::east_opt(2 * 3600).expect("paris summer offset");
        assert_eq!(
            format_deadline(at, &FixedOffset::east_opt(0).unwrap()),
            "Mon 16h"
        );
        assert_eq!(format_deadline(at, &paris), "Mon 18h");
    }

    #[test]
    fn the_three_lines_share_a_bar_column() {
        let view = StatusView {
            five_hour: WindowView {
                pct: Some(Pct::new(15.0)),
                projected: Some(Pct::new(23.0)),
                cooldown: "4h32m".to_string(),
                ..WindowView::default()
            },
            seven_day: WindowView {
                pct: Some(Pct::new(2.0)),
                projected: Some(Pct::new(8.0)),
                cooldown: "6d02h".to_string(),
                ..WindowView::default()
            },
            model: "Opus 5 (1M context)".to_string(),
            ctx_pct: Some(42.0),
            ctx_size: 1_000_000,
            ..StatusView::default()
        };
        let rendered = render_status_line(&view, &RenderCtx::default());
        let columns: Vec<usize> = rendered.lines().map(bar_column).collect();
        assert_eq!(columns, vec![columns[0]; 3], "{rendered}");
    }

    /// The idle indicator's line, with colour stripped.
    fn idle_line(idle: IdleView) -> String {
        let view = StatusView {
            five_hour: WindowView {
                pct: Some(Pct::new(15.0)),
                cooldown: "4h32m".to_string(),
                ..WindowView::default()
            },
            seven_day: WindowView {
                pct: Some(Pct::new(2.0)),
                cooldown: "6d02h".to_string(),
                ..WindowView::default()
            },
            model: "Opus 5".to_string(),
            ctx_pct: Some(42.0),
            ctx_size: 1_000_000,
            idle: Some(idle),
            ..StatusView::default()
        };
        let rendered = render_status_line(&view, &RenderCtx::default());
        strip_ansi(rendered.lines().nth(2).expect("a third line"))
    }

    #[test]
    fn the_idle_indicator_sits_after_the_context_bar() {
        let line = idle_line(IdleView {
            seconds: Some(45.0),
            cache_ttl: Some(3600),
            ttl_inherited: false,
            live: false,
        });
        assert_eq!(line, "🐶 Opus 5   ▨▨▨▨□□□□□□ 42%ctx  💤 ▨▨▨▨▨ 45s/1h");
    }

    #[test]
    fn the_idle_bar_drains_as_the_cache_decays() {
        let drained = idle_line(IdleView {
            seconds: Some(200.0),
            cache_ttl: Some(300),
            ..IdleView::default()
        });
        assert_eq!(drained, "🐶 Opus 5   ▨▨▨▨□□□□□□ 42%ctx  💤 ▨▨□□□ 3m/5m");

        let cold = idle_line(IdleView {
            seconds: Some(5400.0),
            cache_ttl: Some(300),
            ..IdleView::default()
        });
        assert_eq!(cold, "🐶 Opus 5   ▨▨▨▨□□□□□□ 42%ctx  🥶 □□□□□ 1h30m/5m 👋");
    }

    #[test]
    fn a_live_turn_adds_a_marker_beside_the_timer() {
        let line = idle_line(IdleView {
            seconds: Some(4.0),
            cache_ttl: Some(3600),
            ttl_inherited: false,
            live: true,
        });
        assert_eq!(line, "🐶 Opus 5   ▨▨▨▨□□□□□□ 42%ctx  💤 ▨▨▨▨▨ 04s/1h •");
    }

    #[test]
    fn an_unknown_anchor_keeps_the_block_with_a_placeholder() {
        let line = idle_line(IdleView {
            seconds: None,
            cache_ttl: Some(3600),
            ttl_inherited: true,
            live: false,
        });
        assert_eq!(line, "🐶 Opus 5   ▨▨▨▨□□□□□□ 42%ctx  💤 □□□□□  --");
    }

    #[test]
    fn an_inherited_lifetime_is_dimmed_rather_than_hidden() {
        let inherited = IdleView {
            seconds: Some(45.0),
            cache_ttl: Some(3600),
            ttl_inherited: true,
            live: false,
        };
        let measured = IdleView {
            ttl_inherited: false,
            ..inherited
        };
        // Same text either way: only the colour of the tag differs.
        assert_eq!(idle_line(inherited), idle_line(measured));
        let view = StatusView {
            model: "Opus 5".to_string(),
            idle: Some(inherited),
            ..StatusView::default()
        };
        let rendered = render_status_line(&view, &RenderCtx::default());
        assert!(
            rendered.contains(&format!("{}{}/1h", crate::color::RESET, crate::color::DIM)),
            "the inherited tag should carry its own dim colour"
        );
    }

    /// Terminal column where a line's first bar segment starts.
    fn bar_column(line: &str) -> usize {
        let plain = strip_ansi(line);
        let offset = plain
            .char_indices()
            .find(|(_, ch)| *ch == '▨' || *ch == '□')
            .map(|(index, _)| index)
            .expect("a line with a bar");
        display_width(&plain[..offset])
    }
}
