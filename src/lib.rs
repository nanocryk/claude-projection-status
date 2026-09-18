//! A Claude Code status line: usage bars for the 5h and 7d rate-limit windows,
//! end-of-window projections, and session context.

pub mod app;
pub mod color;
pub mod config;
pub mod estimate;
pub mod glyphs;
pub mod history;
pub mod input;
pub mod profile;
pub mod render;
pub mod slots;
pub mod storage;
pub mod units;
pub mod window;
