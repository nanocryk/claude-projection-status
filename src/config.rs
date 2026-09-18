//! Settings, read from `~/.config/claude-projection-status/config.json` and
//! overridden by environment variables.

use std::path::PathBuf;

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Config {
    /// Usage percentage at which the current-usage figure turns yellow.
    pub warning_pct: f64,
    /// Usage percentage at which it turns red.
    pub critical_pct: f64,
    /// Read session transcripts for the model mix and idle indicators.
    pub show_model_mix: bool,
    pub cache_dir: PathBuf,
    /// Days of raw samples to keep.
    pub retention_days: u32,
    pub debug: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            warning_pct: 40.0,
            critical_pct: 70.0,
            show_model_mix: true,
            cache_dir: default_cache_dir(),
            retention_days: 14,
            debug: false,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let file = load_config_file();
        let defaults = Self::default();
        Self {
            warning_pct: number(&file, "warning_pct", "CLAUDE_STATUS_WARNING")
                .unwrap_or(defaults.warning_pct),
            critical_pct: number(&file, "critical_pct", "CLAUDE_STATUS_CRITICAL")
                .unwrap_or(defaults.critical_pct),
            show_model_mix: flag(&file, "show_model_mix", "CLAUDE_STATUS_MODEL_MIX")
                .unwrap_or(defaults.show_model_mix),
            cache_dir: setting(&file, "cache_dir", "CLAUDE_STATUS_CACHE")
                .map(PathBuf::from)
                .unwrap_or(defaults.cache_dir),
            retention_days: number(&file, "retention_days", "CLAUDE_STATUS_RETENTION")
                .map(|days| days as u32)
                .unwrap_or(defaults.retention_days),
            debug: flag(&file, "debug", "CLAUDE_STATUS_DEBUG").unwrap_or(defaults.debug),
        }
    }

    /// History database. Distinct from the Python tool's `history.db`, which
    /// stays where it is.
    pub fn db_path(&self) -> PathBuf {
        self.cache_dir.join("state.db")
    }
}

pub fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key).map(PathBuf::from)
}

fn default_cache_dir() -> PathBuf {
    let base = home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".cache").join("claude-projection-status")
}

fn config_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os("CLAUDE_STATUS_CONFIG") {
        return PathBuf::from(explicit);
    }
    let base = home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".config")
        .join("claude-projection-status")
        .join("config.json")
}

fn load_config_file() -> Value {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(Value::Null)
}

/// Environment variable first, then the config file.
fn setting(file: &Value, key: &str, env_key: &str) -> Option<String> {
    if let Some(from_env) = std::env::var_os(env_key) {
        return Some(from_env.to_string_lossy().into_owned());
    }
    match file.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn number(file: &Value, key: &str, env_key: &str) -> Option<f64> {
    setting(file, key, env_key)?.trim().parse().ok()
}

fn flag(file: &Value, key: &str, env_key: &str) -> Option<bool> {
    let raw = setting(file, key, env_key)?;
    Some(matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    ))
}

/// Whether the session runs with permission prompts skipped.
pub fn bypass_enabled() -> bool {
    let from_env = std::env::var("CLAUDE_SKIP_PERMISSIONS")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    if from_env {
        return true;
    }
    let Some(home) = home_dir() else {
        return false;
    };
    let settings = std::fs::read_to_string(home.join(".claude").join("settings.json")).ok();
    settings
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.get("defaultMode")?.as_str().map(str::to_owned))
        .is_some_and(|mode| mode == "bypassPermissions")
}

#[cfg(test)]
mod tests {
    use super::{flag, number, setting};
    use serde_json::json;

    #[test]
    fn file_values_are_read_whatever_their_json_type() {
        let file = json!({"warning_pct": 55, "critical_pct": 80.5, "debug": true,
                          "cache_dir": "/tmp/cache"});
        assert_eq!(
            number(&file, "warning_pct", "CLAUDE_STATUS_TEST_UNSET"),
            Some(55.0)
        );
        assert_eq!(
            number(&file, "critical_pct", "CLAUDE_STATUS_TEST_UNSET"),
            Some(80.5)
        );
        assert_eq!(flag(&file, "debug", "CLAUDE_STATUS_TEST_UNSET"), Some(true));
        assert_eq!(
            setting(&file, "cache_dir", "CLAUDE_STATUS_TEST_UNSET").as_deref(),
            Some("/tmp/cache")
        );
    }

    #[test]
    fn a_missing_key_falls_through() {
        let file = json!({});
        assert_eq!(
            number(&file, "warning_pct", "CLAUDE_STATUS_TEST_UNSET"),
            None
        );
        assert_eq!(flag(&file, "debug", "CLAUDE_STATUS_TEST_UNSET"), None);
    }

    #[test]
    fn an_unparseable_value_falls_through() {
        let file = json!({"warning_pct": "high"});
        assert_eq!(
            number(&file, "warning_pct", "CLAUDE_STATUS_TEST_UNSET"),
            None
        );
    }
}
