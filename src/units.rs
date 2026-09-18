//! Newtypes for the quantities the status line deals in.
//!
//! Percentages, instants and the estimator's rates are all bare `f64` at the
//! arithmetic level; wrapping them keeps a rate from being multiplied by the
//! wrong kind of time.

use serde::Deserialize;

/// Percentage of a rate-limit window's budget.
///
/// Observed values sit in 0..100; projections can exceed 100.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Deserialize)]
#[serde(transparent)]
pub struct Pct(f64);

impl Pct {
    pub fn new(value: f64) -> Self {
        Self(value)
    }

    pub fn get(self) -> f64 {
        self.0
    }

    /// Value clamped to the 0..100 range a bar can draw.
    pub fn clamped(self) -> f64 {
        self.0.clamp(0.0, 100.0)
    }
}

/// Unix timestamp in seconds.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(f64);

impl Timestamp {
    pub fn new(seconds: f64) -> Self {
        Self(seconds)
    }

    pub fn get(self) -> f64 {
        self.0
    }

    /// Seconds from `earlier` to `self`, negative when `self` is in the past.
    pub fn seconds_since(self, earlier: Self) -> f64 {
        self.0 - earlier.0
    }
}
