//! The JSON payload Claude Code pipes to the status line on each refresh.
//!
//! Every field is optional: a payload from a newer or older client, or a
//! truncated one, parses into whatever it does carry.

use serde::Deserialize;

use crate::units::{Pct, Timestamp};
use crate::window::{WindowKind, WindowState};

#[derive(Debug, Default, Deserialize)]
pub struct Payload {
    pub session_id: Option<String>,
    pub rate_limits: Option<RateLimits>,
    pub model: Option<ModelField>,
    pub context_window: Option<ContextWindow>,
    pub workspace: Option<Workspace>,
    pub cwd: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RateLimits {
    pub five_hour: Option<WindowLimit>,
    pub seven_day: Option<WindowLimit>,
}

#[derive(Debug, Default, Deserialize)]
pub struct WindowLimit {
    pub used_percentage: Option<f64>,
    pub resets_at: Option<f64>,
}

/// `model` arrives as an object from current clients, as a bare name from older ones.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ModelField {
    Named { display_name: Option<String> },
    Name(String),
}

#[derive(Debug, Default, Deserialize)]
pub struct ContextWindow {
    pub used_percentage: Option<f64>,
    pub context_window_size: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Workspace {
    pub current_dir: Option<String>,
    pub project_dir: Option<String>,
}

impl Payload {
    /// Parse a payload, falling back to an empty one on malformed JSON.
    pub fn parse(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_default()
    }

    pub fn session_id(&self) -> &str {
        self.session_id.as_deref().unwrap_or_default()
    }

    /// Usage and reset instant for one window, when the payload carries both.
    pub fn window(&self, kind: WindowKind) -> Option<WindowState> {
        let limits = self.rate_limits.as_ref()?;
        let limit = match kind {
            WindowKind::FiveHour => limits.five_hour.as_ref(),
            WindowKind::SevenDay => limits.seven_day.as_ref(),
        }?;
        Some(WindowState {
            used: Pct::new(limit.used_percentage?),
            resets_at: Timestamp::new(limit.resets_at?),
        })
    }

    /// Display name of the model, or `Unknown` when the payload omits it.
    pub fn model_name(&self) -> String {
        let named = match &self.model {
            Some(ModelField::Named { display_name }) => display_name.clone(),
            Some(ModelField::Name(name)) => Some(name.clone()),
            None => None,
        };
        named
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    /// Directory the session runs in, used to locate its transcript.
    pub fn cwd(&self) -> String {
        let from_workspace = self.workspace.as_ref().and_then(|workspace| {
            workspace
                .current_dir
                .clone()
                .or_else(|| workspace.project_dir.clone())
        });
        from_workspace
            .or_else(|| self.cwd.clone())
            .filter(|dir| !dir.is_empty())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::Payload;
    use crate::window::WindowKind;

    #[test]
    fn empty_input_yields_an_empty_payload() {
        let payload = Payload::parse("");
        assert!(payload.window(WindowKind::FiveHour).is_none());
        assert_eq!(payload.model_name(), "Unknown");
        assert_eq!(payload.session_id(), "");
    }

    #[test]
    fn malformed_json_yields_an_empty_payload() {
        let payload = Payload::parse("{\"rate_limits\": {\"five_hour\": ");
        assert!(payload.window(WindowKind::FiveHour).is_none());
    }

    #[test]
    fn nulls_are_treated_as_absent() {
        let payload = Payload::parse(r#"{"rate_limits": null, "model": null, "cwd": null}"#);
        assert!(payload.window(WindowKind::SevenDay).is_none());
        assert_eq!(payload.model_name(), "Unknown");
    }

    #[test]
    fn a_window_needs_both_usage_and_reset() {
        let payload =
            Payload::parse(r#"{"rate_limits": {"five_hour": {"used_percentage": 12.5}}}"#);
        assert!(payload.window(WindowKind::FiveHour).is_none());
    }

    #[test]
    fn full_window_parses() {
        let payload = Payload::parse(
            r#"{"rate_limits": {"five_hour": {"used_percentage": 12.5, "resets_at": 1758200000},
                                 "seven_day": {"used_percentage": 3.0, "resets_at": 1758700000}}}"#,
        );
        let five_hour = payload
            .window(WindowKind::FiveHour)
            .expect("five hour window");
        assert_eq!(five_hour.used.get(), 12.5);
        assert_eq!(five_hour.resets_at.get(), 1_758_200_000.0);
        assert_eq!(
            payload.window(WindowKind::SevenDay).map(|w| w.used.get()),
            Some(3.0)
        );
    }

    #[test]
    fn model_reads_from_object_or_bare_name() {
        let object = Payload::parse(r#"{"model": {"display_name": "Opus 5 (1M context)"}}"#);
        assert_eq!(object.model_name(), "Opus 5 (1M context)");
        let bare = Payload::parse(r#"{"model": "Sonnet 5"}"#);
        assert_eq!(bare.model_name(), "Sonnet 5");
    }

    #[test]
    fn workspace_wins_over_cwd() {
        let payload =
            Payload::parse(r#"{"workspace": {"current_dir": "/work/here"}, "cwd": "/elsewhere"}"#);
        assert_eq!(payload.cwd(), "/work/here");
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let payload = Payload::parse(r#"{"session_id": "abc", "something_new": {"a": 1}}"#);
        assert_eq!(payload.session_id(), "abc");
    }
}
