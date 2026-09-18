//! The layout must not drift.
//!
//! Each case in `tests/fixtures/render_cases.json` holds a view and the exact
//! line the renderer draws for it, so an unintended change of column, glyph or
//! colour fails here. A deliberate one is taken up by running
//! `cargo test --test render_parity -- --ignored`, which rewrites the captures
//! from the current renderer.
//!
//! The idle indicator is deliberately absent from these cases: it moved to
//! line 3, and its layout is asserted in the renderer's own tests instead.

use claude_status::render::{RenderCtx, StatusView, render_status_line};
use serde::Deserialize;

/// Key whose value each case's captured line sits in.
const EXPECTED_KEY: &str = "\"expected\": \"";

fn fixture_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render_cases.json")
}

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

/// Rewrite every captured line from the current renderer.
///
/// Splices each value in place rather than reserialising the file, so the
/// views, their order and the formatting around them stay untouched and the
/// diff shows only what the layout change did.
#[test]
#[ignore = "rewrites the fixtures; run when a layout change is intended"]
fn regenerate_the_captures() {
    let path = fixture_path();
    let raw = std::fs::read_to_string(&path).expect("fixtures readable");
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw.as_str();

    for case in &cases() {
        let key = rest.find(EXPECTED_KEY).expect("an expected field per case");
        let value = key + EXPECTED_KEY.len();
        let end = value + rest[value..].find('"').expect("a closing quote");
        // Through serde so the escaping matches what the parser expects back.
        let rendered =
            serde_json::to_string(&render_status_line(&case.view, &case.ctx)).expect("a JSON line");
        out.push_str(&rest[..value]);
        out.push_str(&rendered[1..rendered.len() - 1]);
        rest = &rest[end..];
    }
    out.push_str(rest);

    std::fs::write(&path, out).expect("fixtures writable");
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
        .find(|(_, ch)| *ch == '▨' || *ch == '□')
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
