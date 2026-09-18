//! Scripted timelines through the whole refresh path: payload in, recorded,
//! folded, projected, rendered. Each test is one situation the status line has
//! to survive.

use chrono::FixedOffset;

use claude_status::app::{self, Analysis, SessionReport};
use claude_status::input::Payload;
use claude_status::render::{self, IdleView, RenderCtx};
use claude_status::storage::Store;
use claude_status::transcript;
use claude_status::units::Timestamp;
use claude_status::window::WindowKind;

/// 2026-09-14 00:00:00 UTC, a Monday.
const MONDAY: f64 = 1_789_344_000.0;
const HOUR: f64 = 3600.0;
const RETENTION_DAYS: u32 = 14;

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).expect("utc offset")
}

const CWD: &str = "/work/project";

struct Harness {
    store: Store,
    projects: tempfile::TempDir,
}

struct Refresh {
    analysis: Analysis,
    session: SessionReport,
    line: String,
    /// Readings the store holds for each window instance, 5h first.
    sample_counts: [usize; 2],
}

impl Refresh {
    fn projected(&self, kind: WindowKind) -> f64 {
        self.analysis
            .window(kind)
            .projected
            .expect("a projection")
            .get()
    }

    fn rate(&self, kind: WindowKind) -> f64 {
        self.analysis.window(kind).rate.expect("a rate")
    }

    fn samples(&self, kind: WindowKind) -> usize {
        match kind {
            WindowKind::FiveHour => self.sample_counts[0],
            WindowKind::SevenDay => self.sample_counts[1],
        }
    }
}

impl Harness {
    fn new(at: f64) -> Self {
        Self {
            store: Store::in_memory(Timestamp::new(at)).expect("in-memory store"),
            projects: tempfile::tempdir().expect("projects root"),
        }
    }

    /// One status line refresh, as Claude Code would trigger it.
    fn refresh(
        &self,
        at: f64,
        session: &str,
        five_hour: (f64, f64),
        seven_day: (f64, f64),
    ) -> Refresh {
        self.refresh_in(at, session, CWD, five_hour, seven_day)
    }

    /// A refresh reported from a particular working directory.
    fn refresh_in(
        &self,
        at: f64,
        session: &str,
        cwd: &str,
        five_hour: (f64, f64),
        seven_day: (f64, f64),
    ) -> Refresh {
        let raw = format!(
            r#"{{"session_id": "{session}",
                 "workspace": {{"current_dir": "{cwd}"}},
                 "model": {{"display_name": "Opus 5 (1M context)"}},
                 "context_window": {{"used_percentage": 30.0, "context_window_size": 1000000}},
                 "rate_limits": {{
                   "five_hour": {{"used_percentage": {}, "resets_at": {}}},
                   "seven_day": {{"used_percentage": {}, "resets_at": {}}}}}}}"#,
            five_hour.0, five_hour.1, seven_day.0, seven_day.1
        );
        let payload = Payload::parse(&raw);
        let now = Timestamp::new(at);
        let analysis =
            app::analyse(&self.store, &payload, RETENTION_DAYS, now, &utc()).expect("analyse");
        let session_report =
            app::read_session(&self.store, &payload, self.projects.path(), now).expect("session");
        let view = app::build_view(&payload, &analysis, &session_report, now, false, None);
        let line = render::render_status_line(&view, &RenderCtx::default());
        let sample_counts = [
            self.stored_samples(WindowKind::FiveHour, five_hour.1),
            self.stored_samples(WindowKind::SevenDay, seven_day.1),
        ];
        Refresh {
            analysis,
            session: session_report,
            line,
            sample_counts,
        }
    }

    fn stored_samples(&self, kind: WindowKind, resets_at: f64) -> usize {
        self.store
            .window_samples(kind, Timestamp::new(resets_at))
            .expect("window samples")
            .len()
    }

    /// Write a session transcript as Claude Code would.
    fn write_transcript(&self, session: &str, cwd: &str, records: &[String]) {
        let path = self
            .projects
            .path()
            .join(transcript::encode_cwd(cwd))
            .join(format!("{session}.jsonl"));
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create dirs");
        std::fs::write(&path, records.join("\n") + "\n").expect("write transcript");
    }

    /// A stretch of steady work, one refresh every five minutes.
    fn work(&self, from: f64, to: f64, five: &mut (f64, f64), seven: &mut (f64, f64)) {
        let mut at = from;
        while at < to {
            five.0 += 14.0 * (300.0 / HOUR);
            seven.0 += 1.33 * (300.0 / HOUR);
            self.refresh(at, "main", *five, *seven);
            at += 300.0;
        }
    }
}

#[test]
fn a_first_refresh_renders_three_lines_with_a_projection() {
    let harness = Harness::new(MONDAY + 9.0 * HOUR);
    let refresh = harness.refresh(
        MONDAY + 9.0 * HOUR,
        "main",
        (2.0, MONDAY + 14.0 * HOUR),
        (1.0, MONDAY + 7.0 * 86400.0),
    );

    assert_eq!(refresh.line.lines().count(), 3);
    assert!(refresh.line.contains("5h"));
    assert!(refresh.line.contains("7d"));
    assert!(refresh.projected(WindowKind::FiveHour) >= 2.0);
    assert!(refresh.projected(WindowKind::SevenDay) >= 1.0);
}

#[test]
fn closing_the_editor_for_two_hours_does_not_inflate_the_rate() {
    let harness = Harness::new(MONDAY + 9.0 * HOUR);
    let mut five = (0.0, MONDAY + 14.0 * HOUR);
    let mut seven = (0.0, MONDAY + 7.0 * 86400.0);

    harness.work(
        MONDAY + 9.0 * HOUR,
        MONDAY + 10.0 * HOUR,
        &mut five,
        &mut seven,
    );
    let before = harness.refresh(MONDAY + 10.0 * HOUR, "main", five, seven);

    // Two hours away from the keyboard, then back at the same pace.
    let resume = MONDAY + 12.0 * HOUR;
    harness.work(resume, resume + 600.0, &mut five, &mut seven);
    let after = harness.refresh(resume + 600.0, "main", five, seven);

    let (before_rate, after_rate) = (
        before.rate(WindowKind::FiveHour),
        after.rate(WindowKind::FiveHour),
    );
    assert!(
        after_rate < before_rate * 2.0,
        "the hourly rate jumped from {before_rate:.1} to {after_rate:.1} across an idle gap"
    );
}

#[test]
fn a_window_reset_carries_nothing_across() {
    let harness = Harness::new(MONDAY + 9.0 * HOUR);
    let mut five = (0.0, MONDAY + 14.0 * HOUR);
    let mut seven = (0.0, MONDAY + 7.0 * 86400.0);
    harness.work(
        MONDAY + 9.0 * HOUR,
        MONDAY + 13.0 * HOUR,
        &mut five,
        &mut seven,
    );
    let before = harness.refresh(MONDAY + 13.0 * HOUR, "main", five, seven);
    assert!(before.projected(WindowKind::FiveHour) > 40.0);

    // The window resets: usage returns to zero against a new reset instant.
    let fresh = (0.5, MONDAY + 19.0 * HOUR);
    let after = harness.refresh(MONDAY + 14.0 * HOUR + 300.0, "main", fresh, seven);

    assert_eq!(
        after.samples(WindowKind::FiveHour),
        1,
        "the new window starts from its own readings"
    );
    assert!(
        after.projected(WindowKind::FiveHour) < before.projected(WindowKind::FiveHour),
        "the spent window's value followed the reset"
    );
}

#[test]
fn a_stale_reading_at_session_start_leaves_no_trace() {
    let start = MONDAY + 9.0 * HOUR;
    let five_reset = MONDAY + 14.0 * HOUR;
    let seven = (4.0, MONDAY + 7.0 * 86400.0);

    // A cold-started session reports the previous window's 85% first, then
    // corrects itself on the next refresh.
    let stale = Harness::new(start);
    stale.refresh(start, "cold", (85.0, five_reset), seven);
    let corrected = stale.refresh(start + 60.0, "cold", (3.0, five_reset), seven);

    // The same session without the bad first reading.
    let clean = Harness::new(start);
    let honest = clean.refresh(start + 60.0, "cold", (3.0, five_reset), seven);

    assert_eq!(corrected.samples(WindowKind::FiveHour), 1);
    assert!(
        (corrected.projected(WindowKind::FiveHour) - honest.projected(WindowKind::FiveHour)).abs()
            < 0.5,
        "the stale reading moved the projection from {:.1}% to {:.1}%",
        honest.projected(WindowKind::FiveHour),
        corrected.projected(WindowKind::FiveHour)
    );
}

#[test]
fn a_first_ever_refresh_reads_as_on_budget() {
    // With no history the projection leans on the budget-neutral prior, so a
    // fresh window reads as spending about the whole budget.
    let start = MONDAY + 9.0 * HOUR;
    let harness = Harness::new(start);
    // Both windows open at this instant, so the whole budget is still ahead.
    let refresh = harness.refresh(
        start,
        "first",
        (0.0, start + 5.0 * HOUR),
        (0.0, start + 7.0 * 86400.0),
    );
    for kind in [WindowKind::FiveHour, WindowKind::SevenDay] {
        let projected = refresh.projected(kind);
        assert!(
            (99.0..=101.0).contains(&projected),
            "{} projected {projected:.1}% on a first ever run",
            kind.label()
        );
    }
}

#[test]
fn two_sessions_reporting_the_same_window_agree() {
    let harness = Harness::new(MONDAY + 9.0 * HOUR);
    let five_reset = MONDAY + 14.0 * HOUR;
    let seven = (4.0, MONDAY + 7.0 * 86400.0);

    harness.refresh(MONDAY + 9.0 * HOUR, "first", (10.0, five_reset), seven);
    let both = harness.refresh(
        MONDAY + 9.0 * HOUR + 5.0,
        "second",
        (10.0, five_reset),
        seven,
    );

    assert_eq!(
        both.samples(WindowKind::FiveHour),
        1,
        "concurrent reports of one window merged into one reading"
    );
    assert!(both.projected(WindowKind::FiveHour) >= 10.0);
}

/// 2026-09-14 09:00:00 UTC, matching `MONDAY + 9h`.
const NINE_AM: &str = "2026-09-14T09:00:00Z";

fn assistant(at: &str, ttl: Option<u32>) -> String {
    let cache = match ttl {
        Some(3600) => {
            r#", "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 900}"#
        }
        Some(_) => {
            r#", "cache_creation": {"ephemeral_5m_input_tokens": 900, "ephemeral_1h_input_tokens": 0}"#
        }
        None => "",
    };
    format!(
        r#"{{"type": "assistant", "isSidechain": false, "timestamp": "{at}", "message": {{"id": "msg_{at}", "model": "claude-opus-5", "usage": {{"input_tokens": 100, "output_tokens": 10, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0{cache}}}}}}}"#
    )
}

fn tool_result(at: &str) -> String {
    format!(r#"{{"type": "user", "isSidechain": false, "timestamp": "{at}"}}"#)
}

fn idle_of(refresh: &Refresh) -> IdleView {
    refresh.session.idle.expect("an idle indicator")
}

#[test]
fn a_trailing_tool_result_does_not_take_the_timer_away() {
    let start = MONDAY + 9.0 * HOUR;
    let harness = Harness::new(start);
    // The turn ended, then a tool result landed after it: the shape that made
    // the Python drop the indicator entirely.
    harness.write_transcript(
        "live",
        CWD,
        &[
            assistant(NINE_AM, Some(3600)),
            tool_result("2026-09-14T09:00:20Z"),
        ],
    );

    let refresh = harness.refresh_in(
        start + 25.0,
        "live",
        CWD,
        (5.0, MONDAY + 14.0 * HOUR),
        (2.0, MONDAY + 7.0 * 86400.0),
    );
    let idle = idle_of(&refresh);
    assert_eq!(
        idle.seconds,
        Some(25.0),
        "the timer is counting from the turn"
    );
    assert_eq!(idle.cache_ttl, Some(3600));
    assert!(!idle.ttl_inherited, "the lifetime was measured here");
    assert!(idle.live, "a record was written moments ago");
    assert!(refresh.line.contains("1h"), "the lifetime tag is shown");
}

#[test]
fn a_long_tool_run_keeps_counting_instead_of_looking_live() {
    let start = MONDAY + 9.0 * HOUR;
    let harness = Harness::new(start);
    harness.write_transcript("slow", CWD, &[assistant(NINE_AM, Some(3600))]);

    // Ten minutes into a tool that has written nothing since.
    let refresh = harness.refresh_in(
        start + 600.0,
        "slow",
        CWD,
        (5.0, MONDAY + 14.0 * HOUR),
        (2.0, MONDAY + 7.0 * 86400.0),
    );
    let idle = idle_of(&refresh);
    assert_eq!(idle.seconds, Some(600.0));
    assert!(
        !idle.live,
        "liveness must age out rather than stay stuck on a tool whose end was never recorded"
    );
}

#[test]
fn the_cache_lifetime_carries_over_to_a_session_that_has_not_written_one() {
    let start = MONDAY + 9.0 * HOUR;
    let harness = Harness::new(start);

    // An earlier session measured an hour-long cache.
    harness.write_transcript("earlier", CWD, &[assistant(NINE_AM, Some(3600))]);
    let earlier = harness.refresh_in(
        start + 60.0,
        "earlier",
        CWD,
        (5.0, MONDAY + 14.0 * HOUR),
        (2.0, MONDAY + 7.0 * 86400.0),
    );
    assert!(!idle_of(&earlier).ttl_inherited);

    // A new session that has only read from the cache so far.
    harness.write_transcript("fresh", CWD, &[assistant("2026-09-14T09:05:00Z", None)]);
    let fresh = harness.refresh_in(
        start + 310.0,
        "fresh",
        CWD,
        (6.0, MONDAY + 14.0 * HOUR),
        (2.0, MONDAY + 7.0 * 86400.0),
    );
    let idle = idle_of(&fresh);
    assert_eq!(
        idle.cache_ttl,
        Some(3600),
        "an unknown lifetime falls back to the last one seen, not to five minutes"
    );
    assert!(idle.ttl_inherited, "and says so, so the tag renders dimmed");
}

#[test]
fn an_unreadable_transcript_keeps_the_block_in_place() {
    let start = MONDAY + 9.0 * HOUR;
    let harness = Harness::new(start);
    let refresh = harness.refresh_in(
        start,
        "missing",
        CWD,
        (5.0, MONDAY + 14.0 * HOUR),
        (2.0, MONDAY + 7.0 * 86400.0),
    );
    let idle = idle_of(&refresh);
    assert_eq!(idle.seconds, None);
    assert!(
        refresh.line.contains("--"),
        "a missing transcript reads as unknown, not as a warm cache"
    );
}

#[test]
fn moving_to_another_directory_does_not_lose_the_session() {
    let start = MONDAY + 9.0 * HOUR;
    let harness = Harness::new(start);
    harness.write_transcript("wandering", CWD, &[assistant(NINE_AM, Some(3600))]);

    let found = harness.refresh_in(
        start + 30.0,
        "wandering",
        CWD,
        (5.0, MONDAY + 14.0 * HOUR),
        (2.0, MONDAY + 7.0 * 86400.0),
    );
    assert_eq!(idle_of(&found).seconds, Some(30.0));

    // The session reports from a subdirectory it was not filed under.
    let moved = harness.refresh_in(
        start + 90.0,
        "wandering",
        "/work/project/crates/inner",
        (5.0, MONDAY + 14.0 * HOUR),
        (2.0, MONDAY + 7.0 * 86400.0),
    );
    assert_eq!(
        idle_of(&moved).seconds,
        Some(90.0),
        "the resolved transcript is remembered against the session"
    );
}

#[test]
fn the_model_mix_counts_each_turn_once() {
    let start = MONDAY + 9.0 * HOUR;
    let harness = Harness::new(start);
    // One turn written as three content blocks, all carrying the same usage.
    let block = assistant(NINE_AM, Some(3600));
    harness.write_transcript("mix", CWD, &[block.clone(), block.clone(), block]);

    let refresh = harness.refresh_in(
        start + 30.0,
        "mix",
        CWD,
        (5.0, MONDAY + 14.0 * HOUR),
        (2.0, MONDAY + 7.0 * 86400.0),
    );
    assert_eq!(refresh.session.mix.shares.get("o"), Some(&1.0));
    assert_eq!(refresh.session.mix.subagent_count, 0);
}

#[test]
fn a_days_work_leaves_the_line_well_formed_throughout() {
    let harness = Harness::new(MONDAY + 9.0 * HOUR);
    let mut five = (0.0, MONDAY + 14.0 * HOUR);
    let mut seven = (0.0, MONDAY + 7.0 * 86400.0);

    let mut at = MONDAY + 9.0 * HOUR;
    while at < MONDAY + 13.0 * HOUR {
        five.0 += 14.0 * (300.0 / HOUR);
        seven.0 += 1.33 * (300.0 / HOUR);
        let refresh = harness.refresh(at, "main", five, seven);
        assert_eq!(refresh.line.lines().count(), 3, "at {at}");
        let projected = refresh.projected(WindowKind::SevenDay);
        assert!(
            projected.is_finite() && projected >= seven.0,
            "7d projection {projected} at {at}"
        );
        at += 300.0;
    }
}
