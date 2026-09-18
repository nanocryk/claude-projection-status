//! The two rate-limit windows Claude Code reports.

use crate::units::{Pct, Timestamp};

/// A rate-limit window, identified by the period it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind {
    FiveHour,
    SevenDay,
}

impl WindowKind {
    pub const ALL: [WindowKind; 2] = [WindowKind::FiveHour, WindowKind::SevenDay];

    /// Label as it appears in the status line and in stored rows.
    pub fn label(self) -> &'static str {
        match self {
            WindowKind::FiveHour => "5h",
            WindowKind::SevenDay => "7d",
        }
    }

    /// Full length of the window in seconds.
    pub fn length_sec(self) -> f64 {
        match self {
            WindowKind::FiveHour => 5.0 * 3600.0,
            WindowKind::SevenDay => 7.0 * 86400.0,
        }
    }

    /// Instant the window opened, where usage was zero by definition.
    pub fn started_at(self, resets_at: Timestamp) -> Timestamp {
        Timestamp::new(resets_at.get() - self.length_sec())
    }
}

/// What the payload reports for one window.
#[derive(Debug, Clone, Copy)]
pub struct WindowState {
    pub used: Pct,
    pub resets_at: Timestamp,
}

/// One recorded usage reading.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct Sample {
    pub at: Timestamp,
    pub pct: Pct,
}
