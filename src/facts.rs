//! Observations of no operational value, drawn from the history already kept.
//!
//! Every fact is optional: one with nothing to say returns nothing, so a young
//! database simply offers fewer of them.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike as _, NaiveDate, TimeZone, Timelike as _};

use crate::profile::Profile;
use crate::slots::{self, HOURS_PER_DAY, SlotId};
use crate::storage::{Result, SlotCount, Store};
use crate::transcript::{FAMILY_ORDER, Mix};
use crate::units::{Pct, Timestamp};
use crate::window::WindowKind;

const DAY_SEC: f64 = 86400.0;
const WEEK_SEC: f64 = 7.0 * DAY_SEC;
const MONTH_SEC: f64 = 28.0 * DAY_SEC;

/// Share of an hour that must have been worked for it to count as an hour of
/// work here. The estimator deals in fractions; a sentence does not.
const WORKED: f64 = 0.5;

/// Observed slot occurrences a claim about the shape of the day needs behind
/// it. A cold profile is flat, and a flat profile makes every comparison of
/// one hour against another come out level, which is an observation about
/// having no data rather than about the week.
const HOURS_SHAPE: f64 = 72.0;
/// A claim comparing the weekend to the week needs to have seen one.
const WEEK_SHAPE: f64 = 168.0;

/// One elapsed hour, judged.
#[derive(Debug, Clone, Copy)]
struct Hour {
    at: Timestamp,
    worked: bool,
}

/// Every fact that has something to say, in no particular order.
pub fn collect<Tz: TimeZone>(
    store: &Store,
    profile: &Profile,
    mix: &Mix,
    now: Timestamp,
    zone: &Tz,
) -> Result<Vec<String>> {
    let month: Vec<Hour> = store
        .closed_hours_between(Timestamp::new(now.get() - MONTH_SEC), now)?
        .into_iter()
        .map(|(at, active)| Hour {
            at,
            worked: active >= WORKED,
        })
        .collect();
    let week: Vec<Hour> = month
        .iter()
        .copied()
        .filter(|hour| hour.at.get() >= now.get() - WEEK_SEC)
        .collect();

    let counts = store.slot_counts()?;
    let sessions = store.sessions_seen_since(Timestamp::new(now.get() - MONTH_SEC))?;
    let day_windows = store.completed_instances(
        WindowKind::FiveHour,
        Timestamp::new(now.get() - DAY_SEC),
        now,
    )?;
    let last_week = store
        .completed_instances(
            WindowKind::SevenDay,
            Timestamp::new(now.get() - 2.0 * WEEK_SEC),
            now,
        )?
        .last()
        .map(|(_, pct)| *pct);

    // Anything that compares one part of the week against another waits until
    // the profile rests on observation rather than on its flat cold start.
    let observed = profile.observed_occurrences();
    let hours_shape = observed >= HOURS_SHAPE;
    let week_shape = observed >= WEEK_SHAPE;

    let facts = [
        hours_worked(&week),
        longest_break(&week, zone),
        every_hour_of_day(&week, zone),
        hours_shape.then(|| night_over_afternoon(profile)).flatten(),
        week_shape.then(|| weekend_like_weekday(profile)).flatten(),
        session_streak(&sessions, now, zone),
        full_allowances(&day_windows),
        hours_shape.then(|| quietest_hour(profile)).flatten(),
        day_span(&week, now, zone),
        longest_stretch(&week),
        hours_shape.then(|| busiest_hour(&counts)).flatten(),
        busiest_weekday(&month, now, zone),
        donated(last_week),
        model_mix(mix),
        subagents(mix),
    ];
    Ok(facts.into_iter().flatten().collect())
}

/// The week, in hours.
fn hours_worked(week: &[Hour]) -> Option<String> {
    let worked = week.iter().filter(|hour| hour.worked).count();
    (worked >= 20).then(|| format!("You have worked {worked} of the last {} hours.", week.len()))
}

/// The longest idle run, and what it probably was.
fn longest_break<Tz: TimeZone>(week: &[Hour], zone: &Tz) -> Option<String> {
    let (length, end) = longest_run(week, false)?;
    if length < 4 {
        return None;
    }
    let slept = week[end + 1 - length..=end]
        .iter()
        .any(|hour| (3..=5).contains(&slots::local(hour.at, zone).hour()));
    let aside = if slept { " You were asleep." } else { "" };
    Some(format!(
        "Your longest break this week was {length} hours.{aside}"
    ))
}

/// No hour of the day left untouched.
fn every_hour_of_day<Tz: TimeZone>(week: &[Hour], zone: &Tz) -> Option<String> {
    let mut seen = [false; HOURS_PER_DAY];
    for hour in week.iter().filter(|hour| hour.worked) {
        seen[slots::local(hour.at, zone).hour() as usize] = true;
    }
    seen.iter().all(|hit| *hit).then(|| {
        "You have been active at every hour of the day this week. All of them.".to_string()
    })
}

/// The night beating the afternoon.
fn night_over_afternoon(profile: &Profile) -> Option<String> {
    let afternoon = hour_occupancy(profile, 15);
    let (hour, occupancy) = [22u8, 23, 0, 1, 2]
        .into_iter()
        .map(|hour| (hour, hour_occupancy(profile, hour)))
        .max_by(|left, right| left.1.total_cmp(&right.1))?;
    (occupancy > afternoon && afternoon > 0.0)
        .then(|| format!("You work at {hour:02}:00 more often than at 15:00."))
}

/// The weekend that forgot what it was for.
fn weekend_like_weekday(profile: &Profile) -> Option<String> {
    let mean = |days: &[u8]| -> f64 {
        let total: f64 = days
            .iter()
            .flat_map(|weekday| (0..24).map(|hour| profile.occupancy(SlotId::new(*weekday, hour))))
            .sum();
        total / (days.len() * HOURS_PER_DAY) as f64
    };
    let weekdays = mean(&[0, 1, 2, 3, 4]);
    if weekdays <= 0.0 {
        return None;
    }
    let ratio = mean(&[5, 6]) / weekdays;
    if ratio >= 0.9 {
        return Some("Your Saturday looks exactly like your Monday.".to_string());
    }
    (ratio >= 0.4).then(|| {
        format!(
            "Your weekend is {:.0}% as busy as your weekdays.",
            ratio * 100.0
        )
    })
}

/// A session every day, without fail.
fn session_streak<Tz: TimeZone>(
    sessions: &[Timestamp],
    now: Timestamp,
    zone: &Tz,
) -> Option<String> {
    let seen: BTreeSet<NaiveDate> = sessions
        .iter()
        .map(|at| slots::local(*at, zone).date_naive())
        .collect();
    let mut streak = 0;
    let mut cursor = slots::local(now, zone).date_naive();
    while seen.contains(&cursor) {
        streak += 1;
        cursor = cursor.pred_opt()?;
    }
    (streak >= 3).then(|| format!("You have started a session every day for {streak} days."))
}

/// Allowances, plural.
fn full_allowances(windows: &[(Timestamp, Pct)]) -> Option<String> {
    let spent = windows.iter().filter(|(_, pct)| pct.get() >= 95.0).count();
    (spent >= 2)
        .then(|| format!("{spent} full 5h allowances in the last day. There are only 4.8 in one."))
}

/// The quiet hour that is not quiet.
fn quietest_hour(profile: &Profile) -> Option<String> {
    let (hour, occupancy) = (0..24u8)
        .map(|hour| (hour, hour_occupancy(profile, hour)))
        .min_by(|left, right| left.1.total_cmp(&right.1))?;
    (occupancy > 0.15).then(|| format!("Your quietest hour is {hour:02}:00. It is not quiet."))
}

/// How long today has been going on.
fn day_span<Tz: TimeZone>(week: &[Hour], now: Timestamp, zone: &Tz) -> Option<String> {
    let today = slots::local(now, zone).date_naive();
    let first = week
        .iter()
        .find(|hour| hour.worked && slots::local(hour.at, zone).date_naive() == today)?;
    let first_hour = slots::local(first.at, zone).hour();
    let current = slots::local(now, zone).hour();
    (current >= first_hour + 12)
        .then(|| format!("First active hour today: {first_hour:02}:00. It is now {current:02}:00."))
}

/// The longest stretch without stopping.
fn longest_stretch(week: &[Hour]) -> Option<String> {
    let (length, _) = longest_run(week, true)?;
    (length >= 4).then(|| format!("Longest unbroken stretch this week: {length} hours."))
}

/// The hour of the week that sees the most work.
fn busiest_hour(counts: &BTreeMap<SlotId, SlotCount>) -> Option<String> {
    let (slot, count) = counts
        .iter()
        .max_by(|left, right| left.1.active.total_cmp(&right.1.active))?;
    (count.active > 0.0).then(|| {
        format!(
            "Busiest hour of your week: {} {:02}:00.",
            weekday_name(slot.weekday),
            slot.hour
        )
    })
}

/// Today against every other one of its weekday this month.
fn busiest_weekday<Tz: TimeZone>(month: &[Hour], now: Timestamp, zone: &Tz) -> Option<String> {
    let today = slots::local(now, zone).date_naive();
    let mut per_day: BTreeMap<NaiveDate, usize> = BTreeMap::new();
    for hour in month.iter().filter(|hour| hour.worked) {
        let date = slots::local(hour.at, zone).date_naive();
        if date.weekday() == today.weekday() {
            *per_day.entry(date).or_default() += 1;
        }
    }
    let worked_today = *per_day.get(&today)?;
    (worked_today > 0 && per_day.len() >= 3 && per_day.values().all(|hours| *hours <= worked_today))
        .then(|| {
            format!(
                "Your busiest {} this month.",
                weekday_name(weekday_index(today))
            )
        })
}

/// What went back unspent.
fn donated(last_week: Option<Pct>) -> Option<String> {
    let left = 100.0 - last_week?.get();
    (left >= 5.0).then(|| format!("You donated {left:.0}% of last week's tokens to Anthropic 💸"))
}

/// Where this session's tokens went.
fn model_mix(mix: &Mix) -> Option<String> {
    if mix.shares.len() < 2 {
        return None;
    }
    let parts: Vec<String> = FAMILY_ORDER
        .iter()
        .filter_map(|family| {
            let share = (mix.shares.get(*family)? * 100.0).round();
            (share > 0.0).then(|| format!("{share:.0}% {}", family_name(family)))
        })
        .collect();
    (parts.len() >= 2).then(|| format!("{} this session.", parts.join(", ")))
}

/// The swarm.
fn subagents(mix: &Mix) -> Option<String> {
    (mix.subagent_count > 0).then(|| {
        format!(
            "{} subagents this session. They did {:.0}% of the work.",
            mix.subagent_count,
            mix.subagent_share * 100.0
        )
    })
}

/// Length and end index of the longest run of hours sharing a verdict.
fn longest_run(hours: &[Hour], worked: bool) -> Option<(usize, usize)> {
    let mut best = (0, 0);
    let mut run = 0;
    for (index, hour) in hours.iter().enumerate() {
        if hour.worked == worked {
            run += 1;
            if run > best.0 {
                best = (run, index);
            }
        } else {
            run = 0;
        }
    }
    (best.0 > 0).then_some(best)
}

/// Mean occupancy of one hour of the day, across the week.
fn hour_occupancy(profile: &Profile, hour: u8) -> f64 {
    (0..7)
        .map(|weekday| profile.occupancy(SlotId::new(weekday, hour)))
        .sum::<f64>()
        / 7.0
}

fn weekday_index(date: NaiveDate) -> u8 {
    date.weekday().num_days_from_monday() as u8
}

fn weekday_name(weekday: u8) -> &'static str {
    match weekday {
        0 => "Monday",
        1 => "Tuesday",
        2 => "Wednesday",
        3 => "Thursday",
        4 => "Friday",
        5 => "Saturday",
        _ => "Sunday",
    }
}

fn family_name(family: &str) -> &'static str {
    match family {
        "o" => "Opus",
        "s" => "Sonnet",
        "h" => "Haiku",
        "f" => "Fable",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Hour, day_span, every_hour_of_day, hours_worked, longest_break, longest_stretch,
        session_streak, weekend_like_weekday,
    };
    use crate::profile::Profile;
    use crate::slots::SlotId;
    use crate::storage::SlotCount;
    use crate::units::Timestamp;
    use chrono::FixedOffset;
    use std::collections::BTreeMap;

    /// 2026-09-14 00:00:00 UTC, a Monday.
    const MONDAY_MIDNIGHT: f64 = 1_789_344_000.0;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("utc offset")
    }

    /// Hours from midnight on, worked where the pattern says so.
    fn hours(pattern: &[bool]) -> Vec<Hour> {
        pattern
            .iter()
            .enumerate()
            .map(|(index, worked)| Hour {
                at: Timestamp::new(MONDAY_MIDNIGHT + index as f64 * 3600.0),
                worked: *worked,
            })
            .collect()
    }

    #[test]
    fn a_quiet_week_says_nothing_about_hours() {
        assert_eq!(hours_worked(&hours(&[true; 19])), None);
        assert_eq!(
            hours_worked(&hours(&[true; 20])).as_deref(),
            Some("You have worked 20 of the last 20 hours.")
        );
    }

    #[test]
    fn a_break_over_the_small_hours_reads_as_sleep() {
        // Worked until 01:00, idle through 06:00, back at 07:00.
        let mut pattern = [true; 12];
        pattern[2..7].fill(false);
        let fact = longest_break(&hours(&pattern), &utc()).expect("a break");
        assert_eq!(
            fact,
            "Your longest break this week was 5 hours. You were asleep."
        );
    }

    #[test]
    fn a_break_outside_the_small_hours_carries_no_aside() {
        let mut pattern = [true; 24];
        pattern[14..18].fill(false);
        let fact = longest_break(&hours(&pattern), &utc()).expect("a break");
        assert_eq!(fact, "Your longest break this week was 4 hours.");
    }

    #[test]
    fn a_short_break_is_not_worth_mentioning() {
        let mut pattern = [true; 12];
        pattern[4..7].fill(false);
        assert_eq!(longest_break(&hours(&pattern), &utc()), None);
    }

    #[test]
    fn the_stretch_counts_consecutive_hours_only() {
        let mut pattern = [true; 12];
        pattern[5] = false;
        assert_eq!(
            longest_stretch(&hours(&pattern)).as_deref(),
            Some("Longest unbroken stretch this week: 6 hours.")
        );
    }

    #[test]
    fn every_hour_of_the_day_needs_all_of_them() {
        assert_eq!(every_hour_of_day(&hours(&[true; 23]), &utc()), None);
        assert!(every_hour_of_day(&hours(&[true; 24]), &utc()).is_some());
    }

    #[test]
    fn the_day_span_needs_twelve_hours_of_it() {
        let week = hours(&[true; 24]);
        let noon = Timestamp::new(MONDAY_MIDNIGHT + 11.0 * 3600.0);
        assert_eq!(day_span(&week, noon, &utc()), None);
        let evening = Timestamp::new(MONDAY_MIDNIGHT + 18.0 * 3600.0);
        assert_eq!(
            day_span(&week, evening, &utc()).as_deref(),
            Some("First active hour today: 00:00. It is now 18:00.")
        );
    }

    #[test]
    fn a_streak_needs_three_days_running() {
        let days: Vec<Timestamp> = (0..3)
            .map(|day| Timestamp::new(MONDAY_MIDNIGHT + f64::from(day) * 86400.0 + 3600.0))
            .collect();
        let now = Timestamp::new(MONDAY_MIDNIGHT + 2.0 * 86400.0 + 7200.0);
        assert_eq!(
            session_streak(&days, now, &utc()).as_deref(),
            Some("You have started a session every day for 3 days.")
        );
        assert_eq!(session_streak(&days[..2], now, &utc()), None);
    }

    #[test]
    fn a_weekend_as_busy_as_the_week_says_so() {
        let mut counts = BTreeMap::new();
        for weekday in 0..7u8 {
            for hour in 0..24u8 {
                counts.insert(
                    SlotId::new(weekday, hour),
                    SlotCount {
                        occurrences: 10.0,
                        active: if (9..18).contains(&hour) { 10.0 } else { 0.0 },
                    },
                );
            }
        }
        let profile = Profile::from_counts(&counts);
        assert_eq!(
            weekend_like_weekday(&profile).as_deref(),
            Some("Your Saturday looks exactly like your Monday.")
        );
    }
}
