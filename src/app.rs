//! One status line refresh, end to end: record the payload, fold what has
//! elapsed, project both windows, and assemble what the renderer draws.

use chrono::TimeZone;

use crate::estimate::{self, Confidence};
use crate::history;
use crate::input::Payload;
use crate::profile::Profile;
use std::path::Path;

use crate::render::{self, IdleView, StatusView, WindowView};
use crate::storage::{self, Store};
use crate::transcript;
use crate::units::{Pct, Timestamp};
use crate::window::{Sample, WindowKind, WindowState};

/// What one window contributes beyond what the payload already states.
#[derive(Debug, Default)]
pub struct WindowReport {
    pub projected: Option<Pct>,
    pub confidence: Option<Confidence>,
    pub samples: Vec<Sample>,
    /// Percent per hour for the 5h window, per day for the 7d one.
    pub rate: Option<f64>,
    /// Intensity over the one the remaining budget affords.
    pub pace: Option<f64>,
    /// Work the remaining budget buys, on the window that reports it that way.
    pub work_left: Option<String>,
    /// The moment the limit lands, on the window that reports it that way.
    pub time_to_100: Option<String>,
}

#[derive(Debug, Default)]
pub struct Analysis {
    pub five_hour: WindowReport,
    pub seven_day: WindowReport,
}

impl Analysis {
    pub fn window(&self, kind: WindowKind) -> &WindowReport {
        match kind {
            WindowKind::FiveHour => &self.five_hour,
            WindowKind::SevenDay => &self.seven_day,
        }
    }
}

/// What the session's transcript says about the model mix and the prompt cache.
#[derive(Debug, Default)]
pub struct SessionReport {
    pub idle: Option<IdleView>,
    pub mix: transcript::Mix,
}

/// Read the session transcript: which models spent the tokens, how long the
/// prompt cache has been decaying, and how long it lives.
///
/// The transcript path, once resolved, is remembered against the session, so a
/// later change of directory cannot lose the indicator. The cache lifetime
/// falls back to this session's last measurement, then to the last one seen on
/// this machine, rather than to an assumption.
pub fn read_session(
    store: &Store,
    payload: &Payload,
    projects_root: &Path,
    now: Timestamp,
) -> storage::Result<SessionReport> {
    let session = payload.session_id();
    if session.is_empty() {
        return Ok(SessionReport::default());
    }

    let dirs = payload.candidate_dirs();
    let candidates: Vec<&str> = dirs.iter().map(String::as_str).collect();
    let path = transcript::locate(projects_root, session, &candidates)
        .or(store.session_transcript(session)?)
        .filter(|path| path.is_file());

    let Some(path) = path else {
        // Keep the indicator in place with nothing in it: a transcript that
        // cannot be read must not look like a cache that was just written.
        store.remember_session(session, None, None, now)?;
        return Ok(SessionReport {
            idle: Some(IdleView {
                seconds: None,
                cache_ttl: store.last_cache_ttl()?,
                ttl_inherited: true,
                live: false,
            }),
            mix: transcript::Mix::default(),
        });
    };

    let tail = transcript::read_tail(&path);
    store.remember_session(session, Some(&path), tail.cache_ttl, now)?;

    let (cache_ttl, ttl_inherited) = match tail.cache_ttl {
        Some(ttl) => (Some(ttl), false),
        None => match store.session_cache_ttl(session)? {
            Some(ttl) => (Some(ttl), false),
            None => (store.last_cache_ttl()?, true),
        },
    };

    Ok(SessionReport {
        idle: Some(IdleView {
            seconds: tail.idle_for(now),
            cache_ttl,
            ttl_inherited,
            live: tail.is_live(now),
        }),
        mix: transcript::read_mix(&path, session),
    })
}

/// Record this refresh, fold what has elapsed, and project both windows.
pub fn analyse<Tz: TimeZone>(
    store: &Store,
    payload: &Payload,
    retention_days: u32,
    now: Timestamp,
    zone: &Tz,
) -> storage::Result<Analysis> {
    for kind in WindowKind::ALL {
        if let Some(state) = payload.window(kind) {
            store.record(kind, state, payload.session_id(), now)?;
        }
    }
    store.prune_if_due(now, retention_days)?;

    let profile = history::refresh(store, now, zone)?;

    let mut analysis = Analysis::default();
    for kind in WindowKind::ALL {
        let Some(state) = payload.window(kind) else {
            continue;
        };
        let report = report_for(store, &profile, kind, state, now, zone)?;
        match kind {
            WindowKind::FiveHour => analysis.five_hour = report,
            WindowKind::SevenDay => analysis.seven_day = report,
        }
    }
    Ok(analysis)
}

fn report_for<Tz: TimeZone>(
    store: &Store,
    profile: &Profile,
    kind: WindowKind,
    state: WindowState,
    now: Timestamp,
    zone: &Tz,
) -> storage::Result<WindowReport> {
    let samples = store.window_samples(kind, state.resets_at)?;
    // Evidence starts at the first reading of this window: what was spent
    // before the tool was watching says nothing about the pace of the work.
    let watched_from = samples.first().map(|sample| sample.at).unwrap_or(now);
    let observed_used = match (samples.first(), samples.last()) {
        (Some(first), Some(last)) => last.pct - first.pct,
        _ => Pct::new(0.0),
    };
    let active_observed = history::active_hours_since(store, watched_from, now, profile, zone)?;
    let estimate = estimate::project(
        &estimate::Inputs {
            kind,
            used: state.used,
            resets_at: state.resets_at,
            now,
            profile,
            prior: store.prior(kind)?,
            observed_used,
            active_observed,
        },
        zone,
    );
    // The 5h line reads in percent per hour of work, the 7d line per day.
    let rate = match kind {
        WindowKind::FiveHour => estimate.intensity.get(),
        WindowKind::SevenDay => estimate.per_day(kind),
    };
    // Each window says what is left in the unit its length makes readable:
    // five hours hold no night, so the work itself is the answer there, while
    // a week holds several and the date is what a plan hangs on. Neither is
    // worth a column while the pace stays inside the budget.
    let over_budget = estimate.pace.is_some_and(|pace| pace > 1.0);
    let (work_left, time_to_100) = match kind {
        WindowKind::FiveHour => (
            estimate
                .work_remaining
                .filter(|_| over_budget)
                .map(|work| render::format_work_left(work.get())),
            None,
        ),
        WindowKind::SevenDay => (
            None,
            estimate
                .seconds_to_limit
                .map(|seconds| render::format_deadline(Timestamp::new(now.get() + seconds), zone)),
        ),
    };
    Ok(WindowReport {
        projected: Some(estimate.projected),
        confidence: Some(estimate.confidence),
        samples,
        rate: Some(rate),
        pace: estimate.pace,
        work_left,
        time_to_100,
    })
}

pub fn build_view(
    payload: &Payload,
    analysis: &Analysis,
    session: &SessionReport,
    now: Timestamp,
    bypass: bool,
) -> StatusView {
    let context = payload.context_window.as_ref();
    StatusView {
        five_hour: window_view(
            payload.window(WindowKind::FiveHour),
            &analysis.five_hour,
            now,
            false,
        ),
        seven_day: window_view(
            payload.window(WindowKind::SevenDay),
            &analysis.seven_day,
            now,
            true,
        ),
        model: payload.model_name(),
        ctx_pct: context.and_then(|window| window.used_percentage),
        ctx_size: context
            .and_then(|window| window.context_window_size)
            .unwrap_or(0),
        bypass,
        model_shares: session.mix.shares.clone(),
        subagent_count: session.mix.subagent_count,
        subagent_share: session.mix.subagent_share,
        idle: session.idle,
    }
}

fn window_view(
    state: Option<WindowState>,
    report: &WindowReport,
    now: Timestamp,
    use_days: bool,
) -> WindowView {
    WindowView {
        pct: state.map(|state| state.used),
        projected: report.projected,
        cooldown: render::format_cooldown(state.map(|state| state.resets_at), now, use_days),
        time_to_100: report.time_to_100.clone(),
        work_left: report.work_left.clone(),
        samples: report.samples.clone(),
        confidence: report.confidence,
        rate: report.rate,
        pace: report.pace,
        proj_eta: None,
    }
}
