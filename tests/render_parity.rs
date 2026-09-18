//! The layout must not drift.
//!
//! Each case in `tests/fixtures/render_cases.json` holds a view and the line
//! the original Python implementation drew for it, captured from that
//! implementation at commit `ae09abe` before it was replaced. The idle
//! indicator is deliberately absent from these cases: it moved to line 3, and
//! its layout is asserted in the renderer's own tests instead.

use claude_status::render::{RenderCtx, StatusView, render_status_line};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    ctx: RenderCtx,
    view: StatusView,
    expected: String,
}

fn cases() -> Vec<Case> {
    let raw = include_str!("fixtures/render_cases.json");
    serde_json::from_str(raw).expect("render fixtures parse")
}

/// ANSI sequences make a failure unreadable; show the escapes.
fn escaped(text: &str) -> String {
    text.replace('\x1b', "\\e")
}

#[test]
fn every_case_renders_exactly_as_it_was_captured() {
    let cases = cases();
    assert!(cases.len() >= 13, "fixtures look truncated");
    for case in &cases {
        let rendered = render_status_line(&case.view, &case.ctx);
        assert_eq!(
            escaped(&rendered),
            escaped(&case.expected),
            "case {}",
            case.name
        );
    }
}

#[test]
fn every_case_keeps_the_window_bars_in_one_column() {
    for case in &cases() {
        let rendered = render_status_line(&case.view, &case.ctx);
        let columns: Vec<Option<usize>> = rendered.lines().map(bar_column).collect();
        let (Some(five_hour), Some(seven_day)) = (columns[0], columns[1]) else {
            continue;
        };
        assert_eq!(
            five_hour, seven_day,
            "case {} misaligns the window bars",
            case.name
        );
    }
}

#[test]
fn the_context_bar_never_sits_left_of_the_window_bars() {
    for case in &cases() {
        let rendered = render_status_line(&case.view, &case.ctx);
        let columns: Vec<Option<usize>> = rendered.lines().map(bar_column).collect();
        let (Some(window), Some(context)) = (columns[0], columns[2]) else {
            continue;
        };
        // A model name longer than the prefix pushes the context bar right
        // rather than breaking the alignment of the two window lines.
        assert!(
            context >= window,
            "case {} puts the context bar at {context}, left of the window bars at {window}",
            case.name
        );
    }
}

/// Terminal column where a line's first bar segment starts, if it has one.
fn bar_column(line: &str) -> Option<usize> {
    let plain: String = strip_ansi(line);
    let offset = plain
        .char_indices()
        .find(|(_, ch)| *ch == '▰' || *ch == '▱')
        .map(|(index, _)| index)?;
    Some(claude_status::render::display_width(&plain[..offset]))
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
