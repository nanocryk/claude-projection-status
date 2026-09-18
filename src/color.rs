//! ANSI escape sequences used by the renderer.

pub const GREEN: &str = "\x1b[38;5;78m";
pub const YELLOW: &str = "\x1b[33m";
pub const RED: &str = "\x1b[31m";
pub const COLD_BLUE: &str = "\x1b[38;5;45m";
pub const BOLD: &str = "\x1b[1m";
/// Reduced intensity, used to de-emphasise a low-confidence projection.
pub const FAINT: &str = "\x1b[2m";
pub const DIM: &str = "\x1b[38;5;249m";
pub const RESET: &str = "\x1b[0m";
/// Foreground of the consumed bar segment.
pub const FG_WHITE: &str = "\x1b[97m";
/// Sparkline bucket with no data behind it.
pub const SPARK_GAP: &str = "\x1b[38;5;238m";
/// Sparkline bucket below the warning level.
pub const SPARK_LOW: &str = "\x1b[38;5;252m";
