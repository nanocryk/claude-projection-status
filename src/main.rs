//! Reads the payload Claude Code pipes in, prints the status line.

use std::io::Read as _;

use chrono::{Local, Timelike as _};

use claude_status::config::{self, Config};
use claude_status::input::Payload;
use claude_status::render::{self, RenderCtx, StatusView, WindowView};
use claude_status::units::Timestamp;
use claude_status::window::{WindowKind, WindowState};

fn main() {
    let config = Config::load();
    let payload = Payload::parse(&read_stdin());
    let now = Timestamp::new(chrono::Utc::now().timestamp_millis() as f64 / 1000.0);

    let view = build_view(&payload, now);
    let ctx = RenderCtx {
        local_hour: Local::now().hour(),
        warning_pct: config.warning_pct,
        critical_pct: config.critical_pct,
    };
    println!("{}", render::render_status_line(&view, &ctx));
}

fn read_stdin() -> String {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    raw
}

fn build_view(payload: &Payload, now: Timestamp) -> StatusView {
    let context = payload.context_window.as_ref();
    StatusView {
        five_hour: window_view(payload.window(WindowKind::FiveHour), now, false),
        seven_day: window_view(payload.window(WindowKind::SevenDay), now, true),
        model: payload.model_name(),
        ctx_pct: context.and_then(|window| window.used_percentage),
        ctx_size: context
            .and_then(|window| window.context_window_size)
            .unwrap_or(0),
        bypass: config::bypass_enabled(),
        ..StatusView::default()
    }
}

fn window_view(state: Option<WindowState>, now: Timestamp, use_days: bool) -> WindowView {
    WindowView {
        pct: state.map(|state| state.used),
        cooldown: render::format_cooldown(state.map(|state| state.resets_at), now, use_days),
        ..WindowView::default()
    }
}
