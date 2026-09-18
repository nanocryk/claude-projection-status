//! The transcript reader against a committed session tree.
//!
//! These files are records as Claude Code writes them, including the awkward
//! ones: a synthetic model, a line truncated mid-write, a turn with an empty
//! usage object, and subagents on their own sidechains.

use std::path::PathBuf;

use claude_status::transcript::{self, read_mix, read_tail};
use claude_status::units::Timestamp;

fn projects_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("projects")
}

fn session() -> PathBuf {
    transcript::locate(&projects_root(), "sess1", &["/fake/proj"]).expect("the fixture session")
}

fn at(stamp: &str) -> Timestamp {
    Timestamp::new(
        chrono::DateTime::parse_from_rfc3339(stamp)
            .expect("a timestamp")
            .timestamp() as f64,
    )
}

#[test]
fn a_session_is_found_from_its_working_directory() {
    let path = session();
    assert!(path.ends_with("sess1.jsonl"));
    assert_eq!(
        transcript::locate(&projects_root(), "sess1", &["/other/place"]),
        None
    );
}

#[test]
fn the_anchor_is_the_last_assistant_turn_in_the_file() {
    let tail = read_tail(&session());
    assert_eq!(tail.last_assistant, Some(at("2026-04-30T08:15:00Z")));
    assert_eq!(tail.last_record, Some(at("2026-04-30T08:15:00Z")));
    // Nothing in this session ever wrote to the cache.
    assert_eq!(tail.cache_ttl, None);
    assert_eq!(tail.idle_for(at("2026-04-30T08:20:00Z")), Some(300.0));
}

#[test]
fn tokens_are_counted_per_family_across_the_subagents() {
    let mix = read_mix(&session(), "sess1");

    // 2000 tokens on the main chain, 150 in one subagent, 200 in the other.
    let opus = mix.shares.get("o").copied().expect("an opus share");
    let haiku = mix.shares.get("h").copied().expect("a haiku share");
    let sonnet = mix.shares.get("s").copied().expect("a sonnet share");
    assert!((opus - 2000.0 / 2350.0).abs() < 1e-9);
    assert!((haiku - 150.0 / 2350.0).abs() < 1e-9);
    assert!((sonnet - 200.0 / 2350.0).abs() < 1e-9);
    assert!((opus + haiku + sonnet - 1.0).abs() < 1e-9);

    assert_eq!(mix.subagent_count, 2);
    assert!((mix.subagent_share - 350.0 / 2350.0).abs() < 1e-9);
}

#[test]
fn unusable_records_are_skipped_rather_than_counted() {
    let mix = read_mix(&session(), "sess1");
    // A `<synthetic>` model has no family of its own, and a turn carrying an
    // empty usage object spent nothing, so neither reaches the totals.
    assert_eq!(mix.shares.len(), 3);
    assert!(!mix.shares.contains_key("?"));
}

#[test]
fn a_session_with_no_files_reports_nothing() {
    let missing = projects_root().join("-fake-proj").join("nothing.jsonl");
    let mix = read_mix(&missing, "nothing");
    assert!(mix.shares.is_empty());
    assert_eq!(mix.subagent_count, 0);
    assert_eq!(read_tail(&missing).last_assistant, None);
}
