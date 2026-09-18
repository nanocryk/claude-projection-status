//! Projecting end-of-window usage.
//!
//! Consumption happens while working, so the projection multiplies an
//! intensity (percent of budget per active hour) by the active hours the
//! window still holds. Wall-clock time never multiplies an intensity, which is
//! what keeps a fresh window from being projected as seven days of work.

use chrono::TimeZone;
use serde::Deserialize;

use crate::profile::Profile;
use crate::slots::{self, SlotChunk};
use crate::storage::Prior;
use crate::units::{ActiveHours, Pct, PctPerActiveHour, Timestamp};
use crate::window::WindowKind;

/// Share of a window's expected working hours that the prior is worth. One
/// quarter lets a first working day move the estimate while still damping the
/// first minutes of a window.
const PRIOR_SHARE: f64 = 0.25;

/// Floor under the prior's weight, so a window the profile expects to be
/// entirely idle still damps a burst.
const MIN_PRIOR_WEIGHT: f64 = 0.5;

/// Floor under expected active hours, keeping the divisions finite.
const MIN_ACTIVE_HOURS: f64 = 0.25;

/// Posterior width, relative to the estimate, at which confidence steps down.
const HIGH_CONFIDENCE_WIDTH: f64 = 0.15;
const MEDIUM_CONFIDENCE_WIDTH: f64 = 0.30;

/// How much observation stands behind a projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// What a projection is computed from.
#[derive(Debug, Clone, Copy)]
pub struct Inputs<'a> {
    pub kind: WindowKind,
    pub used: Pct,
    pub resets_at: Timestamp,
    pub now: Timestamp,
    pub profile: &'a Profile,
    /// Intensity carried over from completed windows of the same kind.
    pub prior: Option<Prior>,
    /// Active hours observed since this window opened.
    pub active_elapsed: ActiveHours,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Usage expected when the window resets.
    pub projected: Pct,
    pub intensity: PctPerActiveHour,
    pub active_remaining: ActiveHours,
    /// Active hours the whole window is expected to hold.
    pub active_total: ActiveHours,
    pub confidence: Confidence,
    /// Seconds from now until usage is projected to reach 100%.
    pub seconds_to_limit: Option<f64>,
}

impl Estimate {
    /// Percent of the budget per day, at this intensity and working pattern.
    pub fn per_day(&self, window: WindowKind) -> f64 {
        let days = window.length_sec() / 86400.0;
        if days <= 0.0 {
            return 0.0;
        }
        self.intensity.get() * self.active_total.get() / days
    }
}

pub fn project<Tz: TimeZone>(inputs: &Inputs<'_>, zone: &Tz) -> Estimate {
    let window_start = inputs.kind.started_at(inputs.resets_at);
    let from = inputs.now.later_of(window_start);
    let remaining_chunks = slots::chunks(from, inputs.resets_at, zone);
    let all_chunks = slots::chunks(window_start, inputs.resets_at, zone);

    let active_remaining = inputs.profile.expected_active_hours(&remaining_chunks);
    let active_total = inputs
        .profile
        .expected_active_hours(&all_chunks)
        .at_least(MIN_ACTIVE_HOURS);

    // Weight the prior against what this window can hold, so the 5h and 7d
    // windows are damped in proportion rather than by a constant.
    let prior_weight =
        ActiveHours::new(PRIOR_SHARE * active_total.get()).at_least(MIN_PRIOR_WEIGHT);
    // With nothing to go on, assume the pattern that exactly spends the
    // budget: the projection then reads as on track, at low confidence.
    let prior_intensity = inputs
        .prior
        .map(|prior| PctPerActiveHour::new(prior.lambda))
        .unwrap_or_else(|| Pct::new(100.0) / active_total);

    let intensity =
        (prior_intensity * prior_weight + inputs.used) / (prior_weight + inputs.active_elapsed);
    let projected = inputs.used + intensity * active_remaining;

    Estimate {
        projected,
        intensity,
        active_remaining,
        active_total,
        confidence: confidence_of(inputs),
        seconds_to_limit: seconds_to_limit(
            inputs.used,
            intensity,
            &remaining_chunks,
            inputs.profile,
            inputs.now,
        ),
    }
}

/// Confidence rests on observed budget only: the mass behind the prior plus
/// what this window has spent. A cold start's invented prior adds none.
fn confidence_of(inputs: &Inputs<'_>) -> Confidence {
    let prior_mass = inputs
        .prior
        .map(|prior| prior.lambda * prior.weight)
        .unwrap_or(0.0);
    let evidence = prior_mass + inputs.used.get();
    if evidence <= 0.0 {
        return Confidence::Low;
    }
    let relative_width = 1.0 / evidence.sqrt();
    if relative_width <= HIGH_CONFIDENCE_WIDTH {
        Confidence::High
    } else if relative_width <= MEDIUM_CONFIDENCE_WIDTH {
        Confidence::Medium
    } else {
        Confidence::Low
    }
}

/// Walk the remaining slots until the accumulated usage reaches 100%.
///
/// Same walk that produced the projection, so the two cannot disagree.
fn seconds_to_limit(
    used: Pct,
    intensity: PctPerActiveHour,
    remaining: &[SlotChunk],
    profile: &Profile,
    now: Timestamp,
) -> Option<f64> {
    let mut budget = 100.0 - used.get();
    if budget <= 0.0 {
        return Some(0.0);
    }
    for chunk in remaining {
        let consumed = intensity.get() * profile.occupancy(chunk.slot) * chunk.fraction();
        if consumed >= budget {
            let into_chunk = (budget / consumed) * (chunk.to.get() - chunk.from.get());
            return Some((chunk.from.seconds_since(now) + into_chunk).max(0.0));
        }
        budget -= consumed;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Confidence, Estimate, Inputs, project};
    use crate::profile::Profile;
    use crate::slots::SlotId;
    use crate::storage::{Prior, SlotCount};
    use crate::units::{ActiveHours, Pct, Timestamp};
    use crate::window::WindowKind;
    use chrono::FixedOffset;
    use std::collections::BTreeMap;

    /// 2026-09-14 00:00:00 UTC, a Monday.
    const MONDAY_MIDNIGHT: f64 = 1_789_344_000.0;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("utc offset")
    }

    /// Someone who works 09:00 to 18:00 on weekdays: 45 hours a week.
    fn nine_to_five() -> Profile {
        let mut counts = BTreeMap::new();
        for weekday in 0..7u8 {
            for hour in 0..24u8 {
                let working = weekday < 5 && (9..18).contains(&hour);
                counts.insert(
                    SlotId::new(weekday, hour),
                    SlotCount {
                        occurrences: 40.0,
                        active: if working { 40.0 } else { 0.0 },
                    },
                );
            }
        }
        Profile::from_counts(&counts)
    }

    fn seven_day(
        used: f64,
        elapsed_hours: f64,
        elapsed_active: f64,
        prior: Option<Prior>,
    ) -> Estimate {
        let profile = nine_to_five();
        let now = Timestamp::new(MONDAY_MIDNIGHT + elapsed_hours * 3600.0);
        let resets_at = Timestamp::new(MONDAY_MIDNIGHT + 7.0 * 86400.0);
        project(
            &Inputs {
                kind: WindowKind::SevenDay,
                used: Pct::new(used),
                resets_at,
                now,
                profile: &profile,
                prior,
                active_elapsed: ActiveHours::new(elapsed_active),
            },
            &utc(),
        )
    }

    #[test]
    fn the_profile_recognises_a_working_week() {
        let profile = nine_to_five();
        assert!(profile.occupancy(SlotId::new(2, 14)) > 0.85);
        assert!(profile.occupancy(SlotId::new(2, 3)) < 0.15);
        assert!(profile.occupancy(SlotId::new(6, 14)) < 0.25);
    }

    #[test]
    fn a_burst_minutes_into_a_fresh_week_projects_a_normal_week() {
        // Ten minutes of work, 2% spent, against a 1.5%/active-hour habit.
        let estimate = seven_day(
            2.0,
            1.5,
            0.17,
            Some(Prior {
                lambda: 1.5,
                weight: 90.0,
            }),
        );
        assert!(
            estimate.projected.get() < 90.0,
            "projected {} from ten minutes of work",
            estimate.projected.get()
        );
        assert!(estimate.projected.get() > 40.0);
    }

    #[test]
    fn the_same_burst_without_the_working_week_still_cannot_reach_the_clock() {
        // The failure mode being designed out: 2% in ten minutes extrapolated
        // over every remaining wall-clock minute is over 2000%.
        let estimate = seven_day(2.0, 1.5, 0.17, None);
        assert!(
            estimate.projected.get() < 200.0,
            "projected {}",
            estimate.projected.get()
        );
    }

    #[test]
    fn a_first_ever_window_reads_as_on_budget() {
        let estimate = seven_day(0.0, 0.0, 0.0, None);
        assert!((estimate.projected.get() - 100.0).abs() < 1.0);
        assert_eq!(estimate.confidence, Confidence::Low);
    }

    #[test]
    fn observation_overtakes_the_prior_as_the_window_fills() {
        let prior = Some(Prior {
            lambda: 1.0,
            weight: 90.0,
        });
        // Twice the usual intensity, sustained over most of the week.
        let early = seven_day(4.0, 4.0, 2.0, prior);
        let late = seven_day(72.0, 5.0 * 24.0, 36.0, prior);
        assert!(
            late.intensity.get() > early.intensity.get(),
            "early {} late {}",
            early.intensity.get(),
            late.intensity.get()
        );
        assert!(late.intensity.get() > 1.6);
    }

    #[test]
    fn a_projection_never_falls_below_what_is_already_spent() {
        for used in [0.0, 5.0, 50.0, 99.0, 140.0] {
            let estimate = seven_day(used, 48.0, 12.0, None);
            assert!(estimate.projected.get() >= used);
        }
    }

    #[test]
    fn a_heavier_habit_projects_higher() {
        let calm = seven_day(
            10.0,
            24.0,
            8.0,
            Some(Prior {
                lambda: 0.5,
                weight: 90.0,
            }),
        );
        let busy = seven_day(
            10.0,
            24.0,
            8.0,
            Some(Prior {
                lambda: 3.0,
                weight: 90.0,
            }),
        );
        assert!(busy.projected.get() > calm.projected.get());
    }

    #[test]
    fn the_deadline_agrees_with_the_projection() {
        let over = seven_day(
            60.0,
            48.0,
            16.0,
            Some(Prior {
                lambda: 4.0,
                weight: 90.0,
            }),
        );
        assert!(over.projected.get() > 100.0);
        let deadline = over.seconds_to_limit.expect("a crossing before reset");
        assert!(deadline > 0.0 && deadline < 5.0 * 86400.0);

        let under = seven_day(
            10.0,
            48.0,
            16.0,
            Some(Prior {
                lambda: 0.3,
                weight: 90.0,
            }),
        );
        assert!(under.projected.get() < 100.0);
        assert_eq!(under.seconds_to_limit, None);
    }

    #[test]
    fn the_deadline_lands_inside_a_working_stretch() {
        let estimate = seven_day(
            95.0,
            24.0,
            8.0,
            Some(Prior {
                lambda: 2.0,
                weight: 90.0,
            }),
        );
        let deadline = estimate.seconds_to_limit.expect("a crossing");
        let at = MONDAY_MIDNIGHT + 24.0 * 3600.0 + deadline;
        let hour = ((at / 3600.0) as i64 % 24) as u8;
        assert!(
            (9..18).contains(&hour),
            "the limit is reached at {hour}:00, outside working hours"
        );
    }

    #[test]
    fn a_spent_window_needs_no_walk() {
        let estimate = seven_day(100.0, 48.0, 16.0, None);
        assert_eq!(estimate.seconds_to_limit, Some(0.0));
    }

    #[test]
    fn confidence_grows_with_observed_usage() {
        assert_eq!(seven_day(0.0, 0.0, 0.0, None).confidence, Confidence::Low);
        assert_eq!(
            seven_day(15.0, 8.0, 4.0, None).confidence,
            Confidence::Medium
        );
        assert_eq!(
            seven_day(
                15.0,
                8.0,
                4.0,
                Some(Prior {
                    lambda: 1.5,
                    weight: 90.0
                })
            )
            .confidence,
            Confidence::High
        );
    }

    #[test]
    fn a_window_past_its_reset_projects_what_is_spent() {
        let estimate = seven_day(42.0, 7.0 * 24.0, 40.0, None);
        assert_eq!(estimate.active_remaining.get(), 0.0);
        assert_eq!(estimate.projected.get(), 42.0);
    }

    #[test]
    fn the_five_hour_window_uses_the_same_estimator() {
        let profile = nine_to_five();
        // Two hours into a 5h window that started at 10:00 on the Monday.
        // A window open from 10:00 to 15:00, entirely inside working hours.
        let resets_at = Timestamp::new(MONDAY_MIDNIGHT + 15.0 * 3600.0);
        let estimate = project(
            &Inputs {
                kind: WindowKind::FiveHour,
                used: Pct::new(20.0),
                resets_at,
                now: Timestamp::new(MONDAY_MIDNIGHT + 12.0 * 3600.0),
                profile: &profile,
                prior: Some(Prior {
                    lambda: 9.0,
                    weight: 20.0,
                }),
                active_elapsed: ActiveHours::new(2.0),
            },
            &utc(),
        );
        assert!(estimate.active_total.get() > 3.0 && estimate.active_total.get() <= 5.0);
        assert!(estimate.projected.get() > 20.0 && estimate.projected.get() < 80.0);
    }

    #[test]
    fn the_daily_rate_follows_the_intensity() {
        let estimate = seven_day(
            10.0,
            24.0,
            8.0,
            Some(Prior {
                lambda: 2.0,
                weight: 90.0,
            }),
        );
        let per_day = estimate.per_day(WindowKind::SevenDay);
        // Roughly nine working hours a day at about two percent an hour.
        assert!(per_day > 8.0 && per_day < 24.0, "per day {per_day}");
    }
}
