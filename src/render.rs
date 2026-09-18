//! Status line layout.
//!
//! Three lines: the 5h window, the 7d window, then the model with its context
//! bar. The bars on all three start at the same column.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Deserialize;

use crate::color;
use crate::estimate::Confidence;
use crate::glyphs;
use crate::units::{Pct, Timestamp};
use crate::window::{Sample, WindowKind};

const BAR_WIDTH: usize = 10;
/// Visible width of the `⇒ NNN%` column. Held as blanks when a window has no
/// projection, so the segments after it stay aligned with the other window.
const PROJ_COLUMN_WIDTH: usize = 6;
const IDLE_BAR_WIDTH: usize = 5;
const SPARK_BUCKETS: usize = 8;
/// A sparkline never scales to less than this, so a quiet window stays flat
/// instead of amplifying rounding noise into full-height bars.
const SPARK_PEAK_FLOOR: f64 = 0.5;
/// Idle past the cache TTL earns the long-idle nudge after this long.
const NUDGE_AFTER_SEC: f64 = 1800.0;

/// Everything one window contributes to the line it owns.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WindowView {
    pub pct: Option<Pct>,
    pub projected: Option<Pct>,
    /// Preformatted time until the window resets.
    pub cooldown: String,
    /// Preformatted time until usage is projected to reach 100%.
    pub time_to_100: Option<String>,
    pub samples: Vec<Sample>,
    pub confidence: Option<Confidence>,
    /// Percent per hour for the 5h window, per day for the 7d one.
    pub rate: Option<f64>,
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
    pub idle_sec: Option<f64>,
    pub cache_ttl: Option<u32>,
}

/// Values that come from the environment rather than from the payload.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct RenderCtx {
    pub local_hour: u32,
    pub warning_pct: f64,
    pub critical_pct: f64,
}

impl Default for RenderCtx {
    fn default() -> Self {
        Self {
            local_hour: 0,
            warning_pct: 40.0,
            critical_pct: 70.0,
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

/// How far back the sparkline reaches.
fn spark_lookback_sec(kind: WindowKind) -> f64 {
    match kind {
        WindowKind::FiveHour => 5.0 * 3600.0,
        WindowKind::SevenDay => 24.0 * 3600.0,
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
fn build_two_tone_bar(pct: f64, projected: Option<f64>, warn: f64, crit: f64) -> String {
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
            glyphs::FILL.to_string().repeat(filled),
            color::RESET
        );
    }
    if proj_filled > 0 {
        let _ = write!(
            bar,
            "{}{}{}",
            proj_color,
            glyphs::PROJ.to_string().repeat(proj_filled),
            color::RESET
        );
    }
    if empty > 0 {
        let _ = write!(
            bar,
            "{}{}{}",
            color::DIM,
            glyphs::EMPTY.to_string().repeat(empty),
            color::RESET
        );
    }
    bar
}

/// Usage increase per bucket over the lookback window. Buckets older than the
/// first sample render as a dim baseline rather than as zero activity.
fn build_sparkline(samples: &[Sample], lookback_sec: f64) -> String {
    if samples.len() < 2 {
        return String::new();
    }
    let now = samples[samples.len() - 1].at.get();
    let start = now - lookback_sec;
    let bucket_dur = lookback_sec / SPARK_BUCKETS as f64;
    let earliest = samples[0];

    let value_at = |instant: f64| -> f64 {
        samples
            .iter()
            .rev()
            .find(|sample| sample.at.get() <= instant)
            .map(|sample| sample.pct.get())
            .unwrap_or(earliest.pct.get())
    };

    let mut deltas: Vec<Option<f64>> = Vec::with_capacity(SPARK_BUCKETS);
    for index in 0..SPARK_BUCKETS {
        let bucket_start = start + index as f64 * bucket_dur;
        let bucket_end = bucket_start + bucket_dur;
        if bucket_end < earliest.at.get() {
            deltas.push(None);
            continue;
        }
        deltas.push(Some(
            (value_at(bucket_end) - value_at(bucket_start)).max(0.0),
        ));
    }

    let peak = deltas
        .iter()
        .flatten()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    if !peak.is_finite() {
        return String::new();
    }
    let peak = peak.max(SPARK_PEAK_FLOOR);

    let mut out = String::new();
    for delta in &deltas {
        match delta {
            None => {
                let _ = write!(
                    out,
                    "{}{}{}",
                    color::SPARK_GAP,
                    glyphs::SPARK_LEVELS[0],
                    color::RESET
                );
            }
            Some(value) => {
                let level = ((value / peak * 6.0 + 0.5) as usize).min(6);
                let level_color = if level >= 5 {
                    color::RED
                } else if level >= 3 {
                    color::YELLOW
                } else {
                    color::SPARK_LOW
                };
                let _ = write!(
                    out,
                    "{}{}{}",
                    level_color,
                    glyphs::SPARK_LEVELS[level],
                    color::RESET
                );
            }
        }
    }
    out
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
        return format!("{}{rate:.1}{unit}{}", color::DIM, color::RESET);
    }
    let rate_color = if rate < warn_above {
        color::GREEN
    } else if rate < crit_above {
        color::YELLOW
    } else {
        color::RED
    };
    format!("{rate_color}{rate:.0}{unit}{}", color::RESET)
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
    let pad = prefix_width.saturating_sub(visible_len(&prefix));
    prefix.push_str(&" ".repeat(pad));

    let bar = build_two_tone_bar(pct.get(), view.projected.map(Pct::get), warn, crit);
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

    if !view.samples.is_empty() {
        let spark = build_sparkline(&view.samples, spark_lookback_sec(kind));
        if !spark.is_empty() {
            parts.push(spark);
        }
    }

    let rate = format_rate(kind, view.rate);
    if !rate.is_empty() {
        parts.push(rate);
    }

    if let Some(deadline) = &view.time_to_100 {
        parts.push(format!(
            "{}{}{} {deadline}{}",
            color::BOLD,
            color::RED,
            glyphs::DEADLINE,
            color::RESET
        ));
    }

    parts.join(" ")
}

/// Time since the conversation last yielded to the user, against the prompt
/// cache TTL: the bar drains as the cache decays, and the tag names the TTL
/// the client is using.
fn format_idle(idle_sec: Option<f64>, cache_ttl: Option<u32>) -> String {
    let Some(idle_sec) = idle_sec else {
        return String::new();
    };
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
            glyphs::FILL.to_string().repeat(filled),
            color::RESET
        );
    }
    if empty > 0 {
        let _ = write!(
            bar,
            "{}{}{}",
            color::DIM,
            glyphs::EMPTY.to_string().repeat(empty),
            color::RESET
        );
    }
    let nudge = if idle_sec >= NUDGE_AFTER_SEC {
        format!(" {}", glyphs::WAVE)
    } else {
        String::new()
    };
    format!(
        "{glyph} {bar} {bold}{tint}{time_str}{ttl_tag}{}{nudge}",
        color::RESET
    )
}

fn build_ctx_bar(ctx_pct: f64) -> String {
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
            glyphs::FILL.to_string().repeat(filled),
            color::RESET
        );
    }
    if empty > 0 {
        let _ = write!(
            bar,
            "{}{}{}",
            color::DIM,
            glyphs::EMPTY.to_string().repeat(empty),
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

/// Render order for known families; unknown ones follow, alphabetically.
const FAMILY_ORDER: [&str; 3] = ["o", "s", "h"];

fn family_color(family: &str) -> &'static str {
    match family {
        "o" => color::DIM,
        "s" => color::YELLOW,
        "h" => color::GREEN,
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
                    parts.push(format!(
                        "{}{pct}%{family}{}",
                        family_color(family),
                        color::RESET
                    ));
                }
            }
            for (family, pct) in &visible {
                if !FAMILY_ORDER.contains(family) {
                    parts.push(format!("{}{pct}%{family}{}", color::DIM, color::RESET));
                }
            }
        }
    }

    if subagent_count > 0 {
        let share_pct = round_to_pct(subagent_share);
        let suffix = if share_pct > 0 {
            format!(" {share_pct}%")
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

/// Time until the limit is reached, in the compact form the deadline column
/// uses.
pub fn format_deadline(seconds: f64) -> String {
    let minutes = (seconds / 60.0).max(0.0) as u64;
    if minutes < 1 {
        return "<1m".to_string();
    }
    if minutes >= 1440 {
        return format!("{}d{:02}h", minutes / 1440, (minutes % 1440) / 60);
    }
    if minutes >= 60 {
        return format!("{}h{:02}m", minutes / 60, minutes % 60);
    }
    format!("{minutes}m")
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
    // Both window prefixes get the same width so the bars line up, and line 3
    // pads the model name to the same column.
    let prefix_5h = format!("🕒 {}/5h", view.five_hour.cooldown);
    let prefix_7d = format!("{} {}/7d", glyphs::CALENDAR, view.seven_day.cooldown);
    let prefix_width = visible_len(&prefix_5h).max(visible_len(&prefix_7d));

    let mut line1 = format_window(WindowKind::FiveHour, &view.five_hour, ctx, prefix_width);
    let idle = format_idle(view.idle_sec, view.cache_ttl);
    if !idle.is_empty() {
        line1.push(' ');
        line1.push_str(&idle);
    }
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

    // Model name stays left-flush so it survives the leading-whitespace strip;
    // a long name pushes the context bar right rather than breaking alignment.
    let head = format!(
        "{}{}{}",
        color::DIM,
        strip_context_note(&view.model),
        color::RESET
    );
    let pad = (prefix_width + 2).saturating_sub(visible_len(&head)).max(1);
    let mut line3_parts: Vec<String> = Vec::new();
    if let Some(ctx_pct) = view.ctx_pct.filter(|_| view.ctx_size > 0) {
        line3_parts.push(format!(
            "{} {}{ctx_pct:.0}%ctx{}",
            build_ctx_bar(ctx_pct),
            ctx_color(ctx_pct),
            color::RESET
        ));
    }
    let model_stats =
        format_model_stats(&view.model_shares, view.subagent_count, view.subagent_share);
    if !model_stats.is_empty() {
        line3_parts.push(model_stats.join(" "));
    }
    let line3 = format!("{head}{}{}", " ".repeat(pad), line3_parts.join("  "));

    format!("{line1}\n{line2}\n{line3}")
}

#[cfg(test)]
mod tests {
    use super::{
        Confidence, RenderCtx, StatusView, WindowView, build_sparkline, build_two_tone_bar,
        display_width, format_cooldown, format_window, render_status_line, strip_context_note,
        visible_len,
    };
    use crate::units::{Pct, Timestamp};
    use crate::window::{Sample, WindowKind};

    fn samples(points: &[(f64, f64)]) -> Vec<Sample> {
        points
            .iter()
            .map(|(at, pct)| Sample {
                at: Timestamp::new(*at),
                pct: Pct::new(*pct),
            })
            .collect()
    }

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
                let bar = build_two_tone_bar(pct, projected, 75.0, 90.0);
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
        let bar = strip_ansi(&build_two_tone_bar(0.4, None, 75.0, 90.0));
        assert!(bar.starts_with('▰'), "{bar:?}");
    }

    #[test]
    fn sparkline_marks_buckets_older_than_the_first_sample() {
        let spark = build_sparkline(&samples(&[(9000.0, 1.0), (18000.0, 4.0)]), 18000.0);
        let plain = strip_ansi(&spark);
        assert_eq!(plain.chars().count(), 8);
        assert!(plain.starts_with('▁'));
    }

    #[test]
    fn sparkline_needs_two_samples() {
        assert_eq!(build_sparkline(&samples(&[(1.0, 1.0)]), 3600.0), "");
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
        let with_projection = format_window(
            WindowKind::FiveHour,
            &WindowView {
                pct: Some(Pct::new(15.0)),
                projected: Some(Pct::new(23.0)),
                cooldown: "4h32m".to_string(),
                confidence: Some(Confidence::Medium),
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
                ..WindowView::default()
            },
            &RenderCtx::default(),
            10,
        );
        assert_eq!(visible_len(&with_projection), visible_len(&without));
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

    /// Terminal column where a line's first bar segment starts.
    fn bar_column(line: &str) -> usize {
        let plain = strip_ansi(line);
        let offset = plain
            .char_indices()
            .find(|(_, ch)| *ch == '▰' || *ch == '▱')
            .map(|(index, _)| index)
            .expect("a line with a bar");
        display_width(&plain[..offset])
    }
}
