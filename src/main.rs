//! Reads the payload Claude Code pipes in, records it, prints the status line.

use std::io::Read as _;
use std::process::ExitCode;

use chrono::{DateTime, Local, TimeZone, Timelike as _};

use claude_status::app::{self, Analysis, SessionReport};
use claude_status::config::{self, Config};
use claude_status::input::Payload;
use claude_status::render::{self, RenderCtx};
use claude_status::slots::SlotId;
use claude_status::storage::{Store, Summary};
use claude_status::units::Timestamp;
use claude_status::window::WindowKind;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check-threshold") => check_threshold(&args[1..]),
        _ if args.iter().any(|arg| arg == "--summary") => print_summary(),
        _ => {
            print_status_line();
            ExitCode::SUCCESS
        }
    }
}

fn now() -> Timestamp {
    Timestamp::new(chrono::Utc::now().timestamp_millis() as f64 / 1000.0)
}

fn print_status_line() {
    let config = Config::load();
    let payload = Payload::parse(&read_stdin());
    let now = now();

    // A storage failure costs the projections, never the status line.
    let (analysis, session, fun) = analyse(&config, &payload, now).unwrap_or_else(|error| {
        if config.debug {
            eprintln!("claude-status: {error}");
        }
        (Analysis::default(), SessionReport::default(), None)
    });

    let view = app::build_view(
        &payload,
        &analysis,
        &session,
        now,
        config::bypass_enabled(),
        fun,
    );
    let ctx = RenderCtx {
        local_hour: Local::now().hour(),
        warning_pct: config.warning_pct,
        critical_pct: config.critical_pct,
    };
    println!("{}", render::render_status_line(&view, &ctx));
}

fn analyse(
    config: &Config,
    payload: &Payload,
    now: Timestamp,
) -> claude_status::storage::Result<(Analysis, SessionReport, Option<String>)> {
    let store = Store::open(&config.db_path(), now)?;
    let analysis = app::analyse(&store, payload, config.retention_days, now, &Local)?;
    let session = if config.show_model_mix {
        app::read_session(&store, payload, &config.projects_root, now)?
    } else {
        SessionReport::default()
    };
    let fun = app::fun_line(
        &store,
        config.fun_line,
        &config.fun_phrases,
        &session,
        now,
        &Local,
    )?;
    Ok((analysis, session, fun))
}

fn read_stdin() -> String {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    raw
}

/// `check-threshold --session-id <id> --window <5h|7d> --threshold <pct>`
///
/// Prints the session's latest usage when it has reached the threshold, and
/// nothing otherwise. Hooks read the presence of output, so a failure to reach
/// the database must not look like a crossing.
fn check_threshold(args: &[String]) -> ExitCode {
    let session = flag_value(args, "--session-id");
    let window = flag_value(args, "--window").and_then(|label| WindowKind::from_label(&label));
    let threshold = flag_value(args, "--threshold").and_then(|value| value.parse::<f64>().ok());

    let (Some(session), Some(window), Some(threshold)) = (session, window, threshold) else {
        eprintln!(
            "usage: claude-status check-threshold --session-id <id> \
             --window <5h|7d> --threshold <percentage>"
        );
        return ExitCode::from(2);
    };

    let config = Config::load();
    let store = match Store::open(&config.db_path(), now()) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("claude-status: {error}");
            return ExitCode::from(1);
        }
    };
    match store.latest_pct(window, &session) {
        Ok(Some(pct)) if pct.get() >= threshold => {
            println!("{:.1}", pct.get());
            ExitCode::SUCCESS
        }
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("claude-status: {error}");
            ExitCode::from(1)
        }
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == name {
            return iter.next().cloned();
        }
        if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

fn print_summary() -> ExitCode {
    let config = Config::load();
    let store = match Store::open(&config.db_path(), now()) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("claude-status: {error}");
            return ExitCode::from(1);
        }
    };
    match store.summary() {
        Ok(summary) => {
            print!("{}", format_summary(&summary));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("claude-status: {error}");
            ExitCode::from(1)
        }
    }
}

fn format_summary(summary: &Summary) -> String {
    let mut out = String::from("=== claude-status history ===\n");
    out.push_str(&format!("samples: {}\n", summary.samples));
    if let (Some(oldest), Some(newest)) = (summary.oldest, summary.newest) {
        out.push_str(&format!(
            "range:   {} to {}\n",
            local_stamp(oldest),
            local_stamp(newest)
        ));
    }
    if let Some(started) = summary.history_started {
        out.push_str(&format!("since:   {}\n", local_stamp(started)));
    }
    for (label, count) in &summary.instances {
        out.push_str(&format!("  {label} windows tracked: {count}\n"));
    }

    if !summary.slots.is_empty() {
        out.push_str("\nactivity profile (active / occurrences, decayed):\n");
        for hour in 0..24u8 {
            let mut row = format!("  {hour:02}:00 ");
            for weekday in 0..7u8 {
                let slot = SlotId::new(weekday, hour);
                match summary.slots.get(&slot) {
                    Some(count) if count.occurrences > 0.0 => {
                        row.push_str(&format!(
                            " {:>4.1}/{:<4.1}",
                            count.active, count.occurrences
                        ));
                    }
                    _ => row.push_str("      .   "),
                }
            }
            out.push_str(&row);
            out.push('\n');
        }
        out.push_str("         ");
        for day in ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] {
            out.push_str(&format!(" {day:<9}"));
        }
        out.push('\n');
    }

    if !summary.priors.is_empty() {
        out.push_str("\nintensity priors (% of budget per active hour):\n");
        for (kind, prior) in &summary.priors {
            out.push_str(&format!(
                "  {}: {:.3} over {:.1} active hours\n",
                kind.label(),
                prior.lambda,
                prior.weight
            ));
        }
    }
    out
}

fn local_stamp(at: Timestamp) -> String {
    Local
        .timestamp_opt(at.get() as i64, 0)
        .single()
        .map(|moment: DateTime<Local>| moment.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "?".to_string())
}
