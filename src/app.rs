//! One status line refresh, end to end: record the payload, fold what has
//! elapsed, project both windows, and assemble what the renderer draws.

use chrono::TimeZone;

use crate::estimate::{self, Confidence};
use crate::history;
use crate::input::Payload;
use crate::profile::Profile;
use crate::render::{self, StatusView, WindowView};
use crate::storage::{self, Store};
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
    Ok(WindowReport {
        projected: Some(estimate.projected),
        confidence: Some(estimate.confidence),
        samples,
        rate: Some(rate),
        time_to_100: estimate.seconds_to_limit.map(render::format_deadline),
    })
}

pub fn build_view(
    payload: &Payload,
    analysis: &Analysis,
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
        ..StatusView::default()
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
        samples: report.samples.clone(),
        confidence: report.confidence,
        rate: report.rate,
        proj_eta: None,
    }
}
