//! Folding what has been observed into the stored profile and priors.
//!
//! The seam between the pure estimation in [`crate::profile`] and the rows in
//! [`crate::storage`].

use chrono::TimeZone;

use crate::profile::{self, HALF_LIFE_SEC, Profile};
use crate::slots;
use crate::storage::{META_CLOSED_THROUGH, META_DECAYED_AT, Prior, Result, Store};
use crate::units::{ActiveHours, Timestamp};
use crate::window::WindowKind;

/// How far before a slot to look for the reading that precedes it, so a rise
/// spanning the boundary is not lost.
const LOOKBACK_SEC: f64 = 2.0 * 3600.0;

/// Activity is read from the 5h window: its percentages move in larger steps
/// than the weekly ones, so an hour of work is visible in them.
const ACTIVITY_WINDOW: WindowKind = WindowKind::FiveHour;

/// Bring the profile and the priors up to date, and return the profile.
pub fn refresh<Tz: TimeZone>(store: &Store, now: Timestamp, zone: &Tz) -> Result<Profile> {
    close_elapsed_slots(store, now, zone)?;
    let profile = Profile::from_counts(&store.slot_counts()?);
    fold_completed_windows(store, &profile, now, zone)?;
    Ok(profile)
}

/// Fold every slot that has fully elapsed into the profile's counters.
///
/// Slots are only closed once the hour is over, so the current hour never
/// counts as idle merely because it has just begun.
fn close_elapsed_slots<Tz: TimeZone>(store: &Store, now: Timestamp, zone: &Tz) -> Result<()> {
    let boundary = slots::slot_start(now, zone);
    let Some(closed_through) = store.meta(META_CLOSED_THROUGH)?.map(Timestamp::new) else {
        // Nothing is known about the hours before the first run.
        store.set_meta(META_CLOSED_THROUGH, boundary.get())?;
        return Ok(());
    };
    if closed_through.get() >= boundary.get() {
        return Ok(());
    }

    let profile = Profile::from_counts(&store.slot_counts()?);
    let samples = store.samples_between(
        ACTIVITY_WINDOW,
        Timestamp::new(closed_through.get() - LOOKBACK_SEC),
        boundary,
    )?;
    let chunks = slots::chunks(closed_through, boundary, zone);
    let weights = profile::activity_weights(&chunks, &samples, &profile);

    // Age what is already counted before adding to it.
    let decayed_at = store.meta(META_DECAYED_AT)?.unwrap_or(now.get());
    let factor = profile::decay_factor(now.get() - decayed_at, HALF_LIFE_SEC);
    if factor < 1.0 {
        store.decay_slots(factor)?;
    }
    store.set_meta(META_DECAYED_AT, now.get())?;

    for (chunk, weight) in chunks.iter().zip(&weights) {
        store.add_slot_occurrence(chunk.slot, *weight)?;
    }
    store.set_meta(META_CLOSED_THROUGH, boundary.get())?;
    Ok(())
}

/// Active hours observed since a window opened.
pub fn active_hours_so_far<Tz: TimeZone>(
    store: &Store,
    kind: WindowKind,
    resets_at: Timestamp,
    now: Timestamp,
    profile: &Profile,
    zone: &Tz,
) -> Result<ActiveHours> {
    let opened_at = kind.started_at(resets_at);
    if now.get() <= opened_at.get() {
        return Ok(ActiveHours::new(0.0));
    }
    let samples = store.samples_between(
        ACTIVITY_WINDOW,
        Timestamp::new(opened_at.get() - LOOKBACK_SEC),
        now,
    )?;
    let chunks = slots::chunks(opened_at, now, zone);
    Ok(profile::observed_active_hours(&chunks, &samples, profile))
}

fn covered_key(kind: WindowKind) -> String {
    format!("prior_covered_{}", kind.label())
}

fn updated_key(kind: WindowKind) -> String {
    format!("prior_updated_{}", kind.label())
}

/// Carry each finished window's intensity into the prior for its kind.
fn fold_completed_windows<Tz: TimeZone>(
    store: &Store,
    profile: &Profile,
    now: Timestamp,
    zone: &Tz,
) -> Result<()> {
    for kind in WindowKind::ALL {
        let covered = Timestamp::new(store.meta(&covered_key(kind))?.unwrap_or(0.0));
        let instances = store.completed_instances(kind, covered, now)?;
        if instances.is_empty() {
            continue;
        }

        let stored = store.prior(kind)?;
        let updated_at = store.meta(&updated_key(kind))?.unwrap_or(now.get());
        let decay = profile::decay_factor(now.get() - updated_at, HALF_LIFE_SEC);
        let mut intensity = stored.map(|prior| prior.lambda).unwrap_or(0.0);
        let mut weight = stored.map(|prior| prior.weight * decay).unwrap_or(0.0);
        let mut latest = covered;

        for (resets_at, final_pct) in instances {
            latest = resets_at;
            let opened_at = kind.started_at(resets_at);
            let samples = store.samples_between(ACTIVITY_WINDOW, opened_at, resets_at)?;
            let chunks = slots::chunks(opened_at, resets_at, zone);
            let active = profile::observed_active_hours(&chunks, &samples, profile);
            if active.get() <= 0.0 {
                continue;
            }
            let mass = intensity * weight + final_pct.get();
            weight += active.get();
            intensity = mass / weight;
        }

        if weight > 0.0 {
            store.set_prior(
                kind,
                Prior {
                    lambda: intensity,
                    weight,
                },
            )?;
        }
        store.set_meta(&covered_key(kind), latest.get())?;
        store.set_meta(&updated_key(kind), now.get())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{covered_key, refresh};
    use crate::profile::Profile;
    use crate::slots::SlotId;
    use crate::storage::{META_CLOSED_THROUGH, Store};
    use crate::units::{Pct, Timestamp};
    use crate::window::{WindowKind, WindowState};
    use chrono::FixedOffset;

    /// 2026-09-14 00:00:00 UTC, a Monday.
    const MONDAY_MIDNIGHT: f64 = 1_789_344_000.0;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("utc offset")
    }

    fn store_at(now: f64) -> Store {
        Store::in_memory(Timestamp::new(now)).expect("in-memory store")
    }

    fn record(store: &Store, at: f64, pct: f64, resets_at: f64) {
        store
            .record(
                WindowKind::FiveHour,
                WindowState {
                    used: Pct::new(pct),
                    resets_at: Timestamp::new(resets_at),
                },
                "session",
                Timestamp::new(at),
            )
            .expect("record");
    }

    #[test]
    fn the_first_run_claims_no_knowledge_of_earlier_hours() {
        let store = store_at(MONDAY_MIDNIGHT + 9.5 * 3600.0);
        refresh(
            &store,
            Timestamp::new(MONDAY_MIDNIGHT + 9.5 * 3600.0),
            &utc(),
        )
        .expect("refresh");
        assert!(store.slot_counts().expect("counts").is_empty());
        assert_eq!(
            store.meta(META_CLOSED_THROUGH).expect("meta"),
            Some(MONDAY_MIDNIGHT + 9.0 * 3600.0)
        );
    }

    #[test]
    fn a_worked_hour_is_counted_once_the_hour_is_over() {
        let start = MONDAY_MIDNIGHT + 9.0 * 3600.0;
        let store = store_at(start);
        refresh(&store, Timestamp::new(start), &utc()).expect("first run");

        let resets = start + 5.0 * 3600.0;
        record(&store, start + 300.0, 1.0, resets);
        record(&store, start + 900.0, 4.0, resets);

        // Half past the next hour, so the worked hour has closed.
        let later = Timestamp::new(start + 5400.0);
        refresh(&store, later, &utc()).expect("second run");

        let counts = store.slot_counts().expect("counts");
        let worked = counts[&SlotId::new(0, 9)];
        assert_eq!(worked.occurrences, 1.0);
        assert_eq!(worked.active, 1.0);
        assert_eq!(counts.len(), 1, "only the closed hour is counted");
    }

    #[test]
    fn an_hour_without_usage_counts_as_idle() {
        let start = MONDAY_MIDNIGHT + 3.0 * 3600.0;
        let store = store_at(start);
        refresh(&store, Timestamp::new(start), &utc()).expect("first run");
        refresh(&store, Timestamp::new(start + 2.0 * 3600.0), &utc()).expect("second run");

        let counts = store.slot_counts().expect("counts");
        assert_eq!(counts[&SlotId::new(0, 3)].occurrences, 1.0);
        assert_eq!(counts[&SlotId::new(0, 3)].active, 0.0);
        assert_eq!(counts[&SlotId::new(0, 4)].occurrences, 1.0);
    }

    #[test]
    fn a_fortnight_away_ages_the_profile_rather_than_freezing_it() {
        let start = MONDAY_MIDNIGHT + 9.0 * 3600.0;
        let store = store_at(start);
        refresh(&store, Timestamp::new(start), &utc()).expect("first run");
        let resets = start + 5.0 * 3600.0;
        record(&store, start + 300.0, 1.0, resets);
        record(&store, start + 900.0, 4.0, resets);
        refresh(&store, Timestamp::new(start + 5400.0), &utc()).expect("second run");
        let before = store.slot_counts().expect("counts")[&SlotId::new(0, 9)];

        // Four weeks later: one half-life.
        let away = Timestamp::new(start + 28.0 * 86400.0 + 5400.0);
        refresh(&store, away, &utc()).expect("after the break");
        let after = store.slot_counts().expect("counts")[&SlotId::new(0, 9)];

        assert!(
            after.active < before.active,
            "active {} did not decay from {}",
            after.active,
            before.active
        );
        assert!((after.active - 0.5).abs() < 0.01, "active {}", after.active);
        // The four idle weeks were counted, so the slot is no longer certain.
        let profile = Profile::from_counts(&store.slot_counts().expect("counts"));
        assert!(profile.occupancy(SlotId::new(0, 9)) < 0.5);
    }

    #[test]
    fn a_finished_window_becomes_the_prior() {
        let start = MONDAY_MIDNIGHT + 9.0 * 3600.0;
        let store = store_at(start);
        refresh(&store, Timestamp::new(start), &utc()).expect("first run");

        let resets = start + 5.0 * 3600.0;
        for step in 0..8 {
            let at = start + f64::from(step) * 1800.0;
            record(&store, at, f64::from(step) * 4.0, resets);
        }

        let after_reset = Timestamp::new(resets + 1800.0);
        refresh(&store, after_reset, &utc()).expect("after the reset");

        let prior = store
            .prior(WindowKind::FiveHour)
            .expect("prior")
            .expect("a prior after a finished window");
        // Four hours of work spending 28% of the budget.
        assert!(
            prior.weight >= 3.5 && prior.weight <= 4.5,
            "weight {}",
            prior.weight
        );
        assert!(
            prior.lambda > 5.0 && prior.lambda < 9.0,
            "intensity {}",
            prior.lambda
        );
        assert_eq!(
            store
                .meta(&covered_key(WindowKind::FiveHour))
                .expect("meta"),
            Some(resets)
        );
    }

    #[test]
    fn a_finished_window_is_folded_in_only_once() {
        let start = MONDAY_MIDNIGHT + 9.0 * 3600.0;
        let store = store_at(start);
        refresh(&store, Timestamp::new(start), &utc()).expect("first run");
        let resets = start + 5.0 * 3600.0;
        for step in 0..8 {
            record(
                &store,
                start + f64::from(step) * 1800.0,
                f64::from(step) * 4.0,
                resets,
            );
        }
        refresh(&store, Timestamp::new(resets + 1800.0), &utc()).expect("after the reset");
        let first = store
            .prior(WindowKind::FiveHour)
            .expect("prior")
            .expect("prior");

        refresh(&store, Timestamp::new(resets + 5400.0), &utc()).expect("again");
        let second = store
            .prior(WindowKind::FiveHour)
            .expect("prior")
            .expect("prior");

        assert!(
            (second.weight - first.weight).abs() < 1e-9,
            "weight moved from {} to {}",
            first.weight,
            second.weight
        );
    }
}
