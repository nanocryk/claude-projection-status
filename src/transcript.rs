//! Reading Claude Code session transcripts.
//!
//! Claude Code writes each session under `~/.claude/projects/<encoded-cwd>/`:
//! `<session_id>.jsonl` is the main conversation, and
//! `<session_id>/subagents/agent-<hash>.jsonl` holds one file per spawned
//! subagent.
//!
//! An assistant turn is written once per content block, so several lines can
//! carry the same `message.id` and the same `usage`. Tokens are counted once
//! per id.

use std::collections::{BTreeMap, HashSet};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::units::Timestamp;

/// How much of the end of a transcript to read for the idle anchor. Large
/// enough to hold many turns, small enough to stay cheap on a long session.
const TAIL_BYTES: u64 = 256 * 1024;

/// A record written this recently means the conversation is still moving.
pub const LIVE_WITHIN_SEC: f64 = 30.0;

/// Render order for known families; unknown ones follow, alphabetically.
pub const FAMILY_ORDER: [&str; 4] = ["o", "s", "h", "f"];

const FAMILY_PATTERNS: [(&str, &str); 4] = [
    ("opus", "o"),
    ("sonnet", "s"),
    ("haiku", "h"),
    ("fable", "f"),
];

/// Mirror Claude Code's project directory encoding: every character that is
/// not a letter or digit becomes a dash.
pub fn encode_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

/// Find a session's main transcript under one of the directories it might
/// have been started in.
pub fn locate(projects_root: &Path, session_id: &str, candidate_dirs: &[&str]) -> Option<PathBuf> {
    if session_id.is_empty() {
        return None;
    }
    candidate_dirs
        .iter()
        .filter(|dir| !dir.is_empty())
        .map(|dir| {
            projects_root
                .join(encode_cwd(dir))
                .join(format!("{session_id}.jsonl"))
        })
        .find(|path| path.is_file())
}

/// What the end of a transcript says about the conversation.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Tail {
    /// Newest assistant record: when the prompt cache was last written.
    pub last_assistant: Option<Timestamp>,
    /// Newest record of any kind: whether anything is still being written.
    pub last_record: Option<Timestamp>,
    /// Cache lifetime this session is using, when it has written one.
    pub cache_ttl: Option<u32>,
}

impl Tail {
    /// Seconds since the prompt cache was last written.
    pub fn idle_for(&self, now: Timestamp) -> Option<f64> {
        self.last_assistant.map(|at| now.seconds_since(at).max(0.0))
    }

    /// Whether the conversation was still writing moments ago.
    ///
    /// Derived from the newest record's age, so it ages out by itself: a tool
    /// whose end is never recorded stops looking live within
    /// [`LIVE_WITHIN_SEC`] rather than staying stuck.
    pub fn is_live(&self, now: Timestamp) -> bool {
        self.last_record
            .is_some_and(|at| now.seconds_since(at) < LIVE_WITHIN_SEC)
    }
}

#[derive(Debug, Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,
    message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_creation: Option<CacheCreation>,
}

#[derive(Debug, Deserialize)]
struct CacheCreation {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

impl Usage {
    /// Everything billed for one turn.
    fn tokens(&self) -> u64 {
        self.input_tokens.unwrap_or(0)
            + self.output_tokens.unwrap_or(0)
            + self.cache_read_input_tokens.unwrap_or(0)
            + self.cache_creation_input_tokens.unwrap_or(0)
    }
}

impl CacheCreation {
    /// Which lifetime the most recent cache write used, when it wrote anything.
    fn ttl(&self) -> Option<u32> {
        let long = self.ephemeral_1h_input_tokens.unwrap_or(0);
        let short = self.ephemeral_5m_input_tokens.unwrap_or(0);
        if long > 0 && long >= short {
            Some(3600)
        } else if short > 0 {
            Some(300)
        } else {
            None
        }
    }
}

impl Record {
    fn is_main_assistant(&self) -> bool {
        self.kind.as_deref() == Some("assistant") && !self.is_sidechain.unwrap_or(false)
    }

    fn at(&self) -> Option<Timestamp> {
        let raw = self.timestamp.as_deref()?;
        chrono::DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|moment| Timestamp::new(moment.timestamp() as f64))
    }
}

fn family_of(model: &str) -> Option<&'static str> {
    if model.is_empty() || model.starts_with('<') {
        return None;
    }
    FAMILY_PATTERNS
        .iter()
        .find(|(needle, _)| model.contains(needle))
        .map(|(_, code)| *code)
        .or(Some("?"))
}

/// Read the end of a transcript.
///
/// Reading only the tail keeps the cost flat as a session grows; when the tail
/// holds no assistant turn, the whole file is read rather than reporting
/// nothing.
pub fn read_tail(path: &Path) -> Tail {
    let tail = match read_last_bytes(path, TAIL_BYTES) {
        Some(text) => text,
        None => return Tail::default(),
    };
    let from_tail = scan_tail(&tail);
    if from_tail.last_assistant.is_some() {
        return from_tail;
    }
    match std::fs::read_to_string(path) {
        Ok(whole) => scan_tail(&whole),
        Err(_) => from_tail,
    }
}

fn scan_tail(text: &str) -> Tail {
    let mut tail = Tail::default();
    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<Record>(line) else {
            continue;
        };
        let Some(at) = record.at() else {
            continue;
        };
        if tail.last_record.is_none_or(|newest| at > newest) {
            tail.last_record = Some(at);
        }
        if !record.is_main_assistant() {
            continue;
        }
        if tail.last_assistant.is_none_or(|newest| at > newest) {
            tail.last_assistant = Some(at);
        }
        if let Some(ttl) = record
            .message
            .as_ref()
            .and_then(|message| message.usage.as_ref())
            .and_then(|usage| usage.cache_creation.as_ref())
            .and_then(CacheCreation::ttl)
        {
            tail.cache_ttl = Some(ttl);
        }
    }
    tail
}

/// Read at most `limit` bytes from the end of a file, dropping the leading
/// partial line.
fn read_last_bytes(path: &Path, limit: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let from = length.saturating_sub(limit);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut buffer = Vec::with_capacity(limit.min(length) as usize);
    file.read_to_end(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer).into_owned();
    if from == 0 {
        return Some(text);
    }
    // The first line was cut in the middle.
    match text.find('\n') {
        Some(index) => Some(text[index + 1..].to_string()),
        None => Some(String::new()),
    }
}

/// Token shares across the session and what its subagents accounted for.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Mix {
    /// Share of session tokens per model family, summing to 1.
    pub shares: BTreeMap<String, f64>,
    pub subagent_count: u32,
    /// Share of session tokens spent inside subagents.
    pub subagent_share: f64,
}

/// Count tokens across a session's main transcript and its subagents.
pub fn read_mix(main: &Path, session_id: &str) -> Mix {
    let mut totals: BTreeMap<String, u64> = BTreeMap::new();
    let main_tokens = accumulate(main, &mut totals);

    let mut subagent_tokens = 0;
    let mut subagent_count = 0;
    for agent in subagent_files(main, session_id) {
        subagent_count += 1;
        subagent_tokens += accumulate(&agent, &mut totals);
    }

    let grand = main_tokens + subagent_tokens;
    if grand == 0 {
        return Mix {
            subagent_count,
            ..Mix::default()
        };
    }
    Mix {
        shares: totals
            .into_iter()
            .map(|(family, tokens)| (family, tokens as f64 / grand as f64))
            .collect(),
        subagent_count,
        subagent_share: subagent_tokens as f64 / grand as f64,
    }
}

fn subagent_files(main: &Path, session_id: &str) -> Vec<PathBuf> {
    let Some(project) = main.parent() else {
        return Vec::new();
    };
    let directory = project.join(session_id).join("subagents");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("agent-"))
        })
        .collect();
    files.sort();
    files
}

/// Add one file's tokens to the per-family totals, returning its own total.
///
/// Turns are counted once per `message.id`, since a turn spanning several
/// content blocks is written as several records carrying the same usage.
fn accumulate(path: &Path, totals: &mut BTreeMap<String, u64>) -> u64 {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let mut counted: HashSet<String> = HashSet::new();
    let mut file_total = 0;
    for line in text.lines() {
        if !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Record>(line) else {
            continue;
        };
        if record.kind.as_deref() != Some("assistant") {
            continue;
        }
        let Some(message) = record.message.as_ref() else {
            continue;
        };
        let Some(family) = message.model.as_deref().and_then(family_of) else {
            continue;
        };
        let tokens = message.usage.as_ref().map(Usage::tokens).unwrap_or(0);
        if tokens == 0 {
            continue;
        }
        if let Some(id) = message.id.as_deref() {
            if !counted.insert(id.to_string()) {
                continue;
            }
        }
        *totals.entry(family.to_string()).or_insert(0) += tokens;
        file_total += tokens;
    }
    file_total
}

#[cfg(test)]
mod tests {
    use super::{Tail, encode_cwd, family_of, locate, read_mix, read_tail};
    use crate::units::Timestamp;
    use std::path::Path;

    /// 2026-09-18T15:26:31Z.
    const AT: f64 = 1_789_745_191.0;

    fn write(path: &Path, lines: &[String]) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create dirs");
        std::fs::write(path, lines.join("\n") + "\n").expect("write transcript");
    }

    fn assistant(at: &str, model: &str, id: &str, tokens: u64, ttl: Option<u32>) -> String {
        let cache = match ttl {
            Some(3600) => {
                r#", "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 900}"#
            }
            Some(_) => {
                r#", "cache_creation": {"ephemeral_5m_input_tokens": 900, "ephemeral_1h_input_tokens": 0}"#
            }
            None => "",
        };
        format!(
            r#"{{"type": "assistant", "isSidechain": false, "timestamp": "{at}",
                 "message": {{"id": "{id}", "model": "{model}",
                 "usage": {{"input_tokens": {tokens}, "output_tokens": 0,
                            "cache_read_input_tokens": 0,
                            "cache_creation_input_tokens": 0{cache}}}}}}}"#
        )
        .replace('\n', "")
    }

    fn user(at: &str) -> String {
        format!(r#"{{"type": "user", "isSidechain": false, "timestamp": "{at}"}}"#)
    }

    #[test]
    fn a_directory_name_keeps_only_letters_and_digits() {
        assert_eq!(encode_cwd("/home/user/.claude"), "-home-user--claude");
        assert_eq!(
            encode_cwd(r"E:\Libraries\vigie workspace\claude-projection-status"),
            "E--Libraries-vigie-workspace-claude-projection-status"
        );
    }

    #[test]
    fn a_transcript_is_found_under_any_directory_the_session_may_have_started_in() {
        let root = tempfile::tempdir().expect("temp dir");
        let project = "/work/project";
        let path = root
            .path()
            .join(encode_cwd(project))
            .join("session-1.jsonl");
        write(&path, &[user("2026-09-18T15:26:23Z")]);

        assert_eq!(
            locate(root.path(), "session-1", &["/work/project/sub", project]),
            Some(path)
        );
        assert_eq!(locate(root.path(), "session-1", &["/elsewhere"]), None);
        assert_eq!(locate(root.path(), "", &[project]), None);
    }

    #[test]
    fn the_anchor_is_the_newest_assistant_turn_whatever_trails_it() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("session.jsonl");
        write(
            &path,
            &[
                assistant("2026-09-18T15:20:00Z", "claude-opus-5", "msg_1", 10, None),
                assistant("2026-09-18T15:26:31Z", "claude-opus-5", "msg_2", 10, None),
                // A tool result lands after the turn; the anchor stays put.
                user("2026-09-18T15:26:39Z"),
            ],
        );
        let tail = read_tail(&path);
        let anchor = tail.last_assistant.expect("an anchor");
        assert_eq!(anchor.get(), AT);
        // Counted from the assistant turn, not from the trailing record.
        assert_eq!(tail.idle_for(Timestamp::new(AT + 600.0)), Some(600.0));
    }

    #[test]
    fn a_session_with_no_assistant_turn_has_no_anchor() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("session.jsonl");
        write(&path, &[user("2026-09-18T15:26:23Z")]);
        let tail = read_tail(&path);
        assert_eq!(tail.last_assistant, None);
        assert_eq!(tail.idle_for(Timestamp::new(AT)), None);
        assert!(tail.last_record.is_some());
    }

    #[test]
    fn an_unreadable_transcript_reports_nothing_rather_than_guessing() {
        let tail = read_tail(Path::new("no-such-file.jsonl"));
        assert_eq!(tail, Tail::default());
        assert_eq!(tail.idle_for(Timestamp::new(AT)), None);
        assert!(!tail.is_live(Timestamp::new(AT)));
    }

    #[test]
    fn liveness_ages_out_on_its_own() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("session.jsonl");
        write(
            &path,
            &[assistant(
                "2026-09-18T15:26:31Z",
                "claude-opus-5",
                "msg_1",
                10,
                None,
            )],
        );
        let tail = read_tail(&path);
        assert!(tail.is_live(Timestamp::new(AT + 10.0)));
        // Nothing new written: a tool whose end was never recorded stops
        // looking live instead of staying stuck.
        assert!(!tail.is_live(Timestamp::new(AT + 45.0)));
    }

    #[test]
    fn the_cache_lifetime_comes_from_the_most_recent_write() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("session.jsonl");
        write(
            &path,
            &[
                assistant(
                    "2026-09-18T15:20:00Z",
                    "claude-opus-5",
                    "msg_1",
                    10,
                    Some(300),
                ),
                assistant(
                    "2026-09-18T15:26:31Z",
                    "claude-opus-5",
                    "msg_2",
                    10,
                    Some(3600),
                ),
            ],
        );
        assert_eq!(read_tail(&path).cache_ttl, Some(3600));
    }

    #[test]
    fn a_session_that_has_only_read_the_cache_reports_no_lifetime() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("session.jsonl");
        write(
            &path,
            &[assistant(
                "2026-09-18T15:26:31Z",
                "claude-opus-5",
                "msg_1",
                10,
                None,
            )],
        );
        assert_eq!(read_tail(&path).cache_ttl, None);
    }

    #[test]
    fn a_turn_written_as_several_blocks_is_counted_once() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("session.jsonl");
        let block = assistant("2026-09-18T15:26:31Z", "claude-opus-5", "msg_1", 100, None);
        write(
            &path,
            &[
                block.clone(),
                block.clone(),
                block,
                assistant(
                    "2026-09-18T15:27:00Z",
                    "claude-haiku-4-5",
                    "msg_2",
                    100,
                    None,
                ),
            ],
        );
        let mix = read_mix(&path, "session");
        assert_eq!(mix.shares.get("o"), Some(&0.5));
        assert_eq!(mix.shares.get("h"), Some(&0.5));
    }

    #[test]
    fn subagent_files_count_toward_the_session() {
        let root = tempfile::tempdir().expect("temp dir");
        let project = root.path().join("project");
        let main = project.join("session.jsonl");
        write(
            &main,
            &[assistant(
                "2026-09-18T15:26:31Z",
                "claude-opus-5",
                "msg_1",
                300,
                None,
            )],
        );
        for (index, tokens) in [(1, 50), (2, 50)] {
            write(
                &project
                    .join("session")
                    .join("subagents")
                    .join(format!("agent-{index}.jsonl")),
                &[assistant(
                    "2026-09-18T15:26:31Z",
                    "claude-sonnet-5",
                    &format!("msg_sub_{index}"),
                    tokens,
                    None,
                )],
            );
        }

        let mix = read_mix(&main, "session");
        assert_eq!(mix.subagent_count, 2);
        assert_eq!(mix.shares.get("s"), Some(&0.25));
        assert_eq!(mix.subagent_share, 0.25);
    }

    #[test]
    fn a_session_without_subagents_reports_none() {
        let root = tempfile::tempdir().expect("temp dir");
        let main = root.path().join("project").join("session.jsonl");
        write(
            &main,
            &[assistant(
                "2026-09-18T15:26:31Z",
                "claude-opus-5",
                "msg_1",
                10,
                None,
            )],
        );
        let mix = read_mix(&main, "session");
        assert_eq!(mix.subagent_count, 0);
        assert_eq!(mix.subagent_share, 0.0);
        assert_eq!(mix.shares.get("o"), Some(&1.0));
    }

    #[test]
    fn model_names_map_to_family_letters() {
        assert_eq!(family_of("claude-opus-5"), Some("o"));
        assert_eq!(family_of("claude-sonnet-5"), Some("s"));
        assert_eq!(family_of("claude-haiku-4-5-20251001"), Some("h"));
        assert_eq!(family_of("claude-fable-5-1"), Some("f"));
        assert_eq!(family_of("some-other-model"), Some("?"));
        assert_eq!(family_of("<synthetic>"), None);
        assert_eq!(family_of(""), None);
    }

    #[test]
    fn a_truncated_last_line_does_not_lose_the_anchor() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("session.jsonl");
        let mut text = assistant("2026-09-18T15:26:31Z", "claude-opus-5", "msg_1", 10, None);
        text.push('\n');
        // A line half written by the client while the status line reads.
        text.push_str("{\"type\": \"assistant\", \"timesta");
        std::fs::write(&path, text).expect("write");
        assert_eq!(read_tail(&path).last_assistant.map(|at| at.get()), Some(AT));
    }
}
