"""Tests for per-model token-share computation."""

import unittest
from pathlib import Path

from claude_status import transcript
from claude_status.transcript import (
    detected_cache_ttl,
    format_mix,
    last_main_assistant_ts,
    model_token_shares,
    session_dir,
    session_mix_string,
    subagent_count,
)

FIXTURES = Path(__file__).parent / "fixtures" / "projects"
CWD = "/fake/proj"  # encodes to "-fake-proj"
SESSION = "sess1"


class TestEncoding(unittest.TestCase):
    def test_session_dir_encodes_cwd(self):
        self.assertEqual(session_dir(CWD, FIXTURES), FIXTURES / "-fake-proj")

    def test_session_dir_missing_returns_none(self):
        self.assertIsNone(session_dir("/nonexistent/path", FIXTURES))

    def test_session_dir_empty_cwd(self):
        self.assertIsNone(session_dir("", FIXTURES))

    def test_session_dir_encodes_underscores_and_dots(self):
        # Claude Code replaces every non-alphanumeric char with '-', not just
        # '/'. So '_' and '.' must be transformed too, otherwise sessions in
        # paths like '/home/u/foo_bar' or '/home/u/.claude' silently miss.
        import tempfile
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "-home-u-foo-bar").mkdir()
            self.assertEqual(
                session_dir("/home/u/foo_bar", root),
                root / "-home-u-foo-bar",
            )
            (root / "-home-u--claude").mkdir()
            self.assertEqual(
                session_dir("/home/u/.claude", root),
                root / "-home-u--claude",
            )


class TestModelTokenShares(unittest.TestCase):
    def test_real_fixture_totals(self):
        # main: 2*1000 = 2000 opus (third turn has no usage, skipped)
        # haiku subagent: 100 + 50 = 150
        # sonnet subagent: 200
        # grand: 2350
        shares = model_token_shares(SESSION, CWD, FIXTURES)
        self.assertAlmostEqual(sum(shares.values()), 1.0, places=6)
        self.assertAlmostEqual(shares["o"], 2000 / 2350, places=6)
        self.assertAlmostEqual(shares["h"], 150 / 2350, places=6)
        self.assertAlmostEqual(shares["s"], 200 / 2350, places=6)

    def test_synthetic_skipped(self):
        # The fixture has a "<synthetic>" turn; if it weren't skipped, opus
        # would still be the only family but a "?" key would never appear.
        shares = model_token_shares(SESSION, CWD, FIXTURES)
        self.assertNotIn("?", shares)

    def test_unknown_session_returns_empty(self):
        self.assertEqual(model_token_shares("nope", CWD, FIXTURES), {})

    def test_unknown_cwd_returns_empty(self):
        self.assertEqual(model_token_shares(SESSION, "/no/such", FIXTURES), {})

    def test_empty_session_id_returns_empty(self):
        self.assertEqual(model_token_shares("", CWD, FIXTURES), {})

    def test_main_only_session(self, tmpdir=None):
        # Build an isolated fixture: main file only, single model.
        import tempfile
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "-x").mkdir()
            (root / "-x" / "only.jsonl").write_text(
                '{"type":"assistant","isSidechain":false,'
                '"message":{"model":"claude-opus-4-7",'
                '"usage":{"input_tokens":10,"output_tokens":20}}}\n'
            )
            shares = model_token_shares("only", "/x", root)
            self.assertEqual(shares, {"o": 1.0})


class TestFormatMix(unittest.TestCase):
    def test_two_families_fixed_order(self):
        # opus first even when haiku has higher share
        out = format_mix({"h": 0.7, "o": 0.3})
        self.assertEqual(out, "30%o 70%h")

    def test_three_families_order(self):
        out = format_mix({"s": 0.2, "h": 0.1, "o": 0.7})
        self.assertEqual(out, "70%o 20%s 10%h")

    def test_single_family_omitted(self):
        self.assertEqual(format_mix({"o": 1.0}), "")

    def test_empty_omitted(self):
        self.assertEqual(format_mix({}), "")

    def test_zero_pct_dropped(self):
        # 0.4% rounds to 0 → only opus visible → returns ""
        self.assertEqual(format_mix({"o": 0.996, "h": 0.004}), "")

    def test_unknown_family_appended(self):
        out = format_mix({"o": 0.5, "?": 0.5})
        self.assertEqual(out, "50%o 50%?")


class TestSessionMixString(unittest.TestCase):
    def test_real_fixture_renders(self):
        # 2000/2350=85.1, 200/2350=8.5, 150/2350=6.4 → 85%o 9%s 6%h
        out = session_mix_string(SESSION, CWD, FIXTURES)
        self.assertEqual(out, "85%o 9%s 6%h")


class TestLastMainAssistantTs(unittest.TestCase):
    def test_returns_latest_main_assistant_ts(self):
        # Fixture's last main-assistant line is 2026-04-30T08:15:00Z
        from datetime import datetime, timezone
        expected = datetime(2026, 4, 30, 8, 15, 0, tzinfo=timezone.utc).timestamp()
        ts = last_main_assistant_ts(SESSION, CWD, FIXTURES)
        self.assertIsNotNone(ts)
        self.assertAlmostEqual(ts, expected, places=3)

    def test_unknown_session_returns_none(self):
        self.assertIsNone(last_main_assistant_ts("nope", CWD, FIXTURES))

    def test_skips_sidechain_assistant_lines(self):
        import tempfile
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "-x").mkdir()
            (root / "-x" / "s.jsonl").write_text(
                '{"type":"assistant","isSidechain":true,"timestamp":"2026-04-30T09:00:00.000Z",'
                '"message":{"model":"claude-haiku-4-5","usage":{}}}\n'
                '{"type":"assistant","isSidechain":false,"timestamp":"2026-04-30T08:00:00.000Z",'
                '"message":{"model":"claude-opus-4-7","usage":{}}}\n'
            )
            from datetime import datetime, timezone
            expected = datetime(2026, 4, 30, 8, 0, 0, tzinfo=timezone.utc).timestamp()
            ts = last_main_assistant_ts("s", "/x", root)
            self.assertAlmostEqual(ts, expected, places=3)

    def test_no_assistant_lines_returns_none(self):
        import tempfile
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "-x").mkdir()
            (root / "-x" / "s.jsonl").write_text(
                '{"type":"user","message":{"role":"user","content":"hi"}}\n'
            )
            self.assertIsNone(last_main_assistant_ts("s", "/x", root))

    def test_returns_none_when_latest_is_tool_use(self):
        # Conversation in-flight (latest assistant line is mid-tool-call):
        # cache stays warm, so don't emit an idle timestamp at all.
        import tempfile
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "-x").mkdir()
            (root / "-x" / "s.jsonl").write_text(
                '{"type":"assistant","isSidechain":false,"timestamp":"2026-04-30T08:00:00.000Z",'
                '"message":{"model":"claude-opus-4-7","stop_reason":"end_turn","usage":{}}}\n'
                '{"type":"assistant","isSidechain":false,"timestamp":"2026-04-30T09:00:00.000Z",'
                '"message":{"model":"claude-opus-4-7","stop_reason":"tool_use","usage":{}}}\n'
            )
            self.assertIsNone(last_main_assistant_ts("s", "/x", root))

    def test_returns_none_when_trailing_user_is_fresh(self):
        # Fresh trailing user prompt (< STALE_TRAILING_USER_SEC): model is
        # actively processing, cache stays warm, no idle to report.
        import tempfile
        from datetime import datetime, timezone
        from unittest import mock
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "-x").mkdir()
            (root / "-x" / "s.jsonl").write_text(
                '{"type":"assistant","isSidechain":false,"timestamp":"2026-04-30T08:00:00.000Z",'
                '"message":{"model":"claude-opus-4-7","stop_reason":"end_turn","usage":{}}}\n'
                '{"type":"user","isSidechain":false,"timestamp":"2026-04-30T08:30:00.000Z",'
                '"message":{"role":"user","content":"next prompt"}}\n'
            )
            now = datetime(2026, 4, 30, 8, 30, 5, tzinfo=timezone.utc).timestamp()
            with mock.patch.object(transcript.time, "time", return_value=now):
                self.assertIsNone(last_main_assistant_ts("s", "/x", root))

    def test_falls_back_to_prior_yield_when_trailing_user_is_stale(self):
        # Stale trailing user (>= STALE_TRAILING_USER_SEC since submission):
        # turn was cancelled/killed before the assistant wrote back. Anchor
        # idle on the most recent prior assistant yield.
        import tempfile
        from datetime import datetime, timezone
        from unittest import mock
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "-x").mkdir()
            (root / "-x" / "s.jsonl").write_text(
                '{"type":"assistant","isSidechain":false,"timestamp":"2026-04-30T08:00:00.000Z",'
                '"message":{"model":"claude-opus-4-7","stop_reason":"end_turn","usage":{}}}\n'
                '{"type":"user","isSidechain":false,"timestamp":"2026-04-30T08:30:00.000Z",'
                '"message":{"role":"user","content":"next prompt"}}\n'
            )
            now = datetime(2026, 4, 30, 8, 31, 0, tzinfo=timezone.utc).timestamp()
            expected = datetime(2026, 4, 30, 8, 0, 0, tzinfo=timezone.utc).timestamp()
            with mock.patch.object(transcript.time, "time", return_value=now):
                ts = last_main_assistant_ts("s", "/x", root)
            self.assertAlmostEqual(ts, expected, places=3)

    def test_returns_none_when_trailing_user_is_stale_but_no_prior_yield(self):
        # Stale trailing user, but the only prior assistant turn was a tool_use
        # (or there are none). Nothing to anchor on → None.
        import tempfile
        from datetime import datetime, timezone
        from unittest import mock
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "-x").mkdir()
            (root / "-x" / "s.jsonl").write_text(
                '{"type":"assistant","isSidechain":false,"timestamp":"2026-04-30T08:00:00.000Z",'
                '"message":{"model":"claude-opus-4-7","stop_reason":"tool_use","usage":{}}}\n'
                '{"type":"user","isSidechain":false,"timestamp":"2026-04-30T08:30:00.000Z",'
                '"message":{"role":"user","content":[{"type":"tool_result"}]}}\n'
            )
            now = datetime(2026, 4, 30, 8, 31, 0, tzinfo=timezone.utc).timestamp()
            with mock.patch.object(transcript.time, "time", return_value=now):
                self.assertIsNone(last_main_assistant_ts("s", "/x", root))

    def test_returns_ts_when_latest_is_end_turn(self):
        # Normal idle state: last main line is a yielding assistant turn.
        import tempfile
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "-x").mkdir()
            (root / "-x" / "s.jsonl").write_text(
                '{"type":"user","isSidechain":false,"timestamp":"2026-04-30T07:00:00.000Z",'
                '"message":{"role":"user","content":"hi"}}\n'
                '{"type":"assistant","isSidechain":false,"timestamp":"2026-04-30T08:00:00.000Z",'
                '"message":{"model":"claude-opus-4-7","stop_reason":"end_turn","usage":{}}}\n'
            )
            from datetime import datetime, timezone
            expected = datetime(2026, 4, 30, 8, 0, 0, tzinfo=timezone.utc).timestamp()
            ts = last_main_assistant_ts("s", "/x", root)
            self.assertAlmostEqual(ts, expected, places=3)


class TestDetectedCacheTtl(unittest.TestCase):
    def _setup(self, body: str) -> Path:
        import tempfile
        td = tempfile.mkdtemp()
        root = Path(td)
        (root / "-x").mkdir()
        (root / "-x" / "s.jsonl").write_text(body)
        return root

    def test_detects_1h_when_recent_creation_is_1h(self):
        root = self._setup(
            '{"type":"assistant","isSidechain":false,"message":{"usage":'
            '{"cache_creation":{"ephemeral_5m_input_tokens":50,"ephemeral_1h_input_tokens":0}}}}\n'
            '{"type":"assistant","isSidechain":false,"message":{"usage":'
            '{"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":133}}}}\n'
        )
        self.assertEqual(detected_cache_ttl("s", "/x", root), 3600)

    def test_detects_5m_when_recent_creation_is_5m(self):
        root = self._setup(
            '{"type":"assistant","isSidechain":false,"message":{"usage":'
            '{"cache_creation":{"ephemeral_5m_input_tokens":133,"ephemeral_1h_input_tokens":0}}}}\n'
        )
        self.assertEqual(detected_cache_ttl("s", "/x", root), 300)

    def test_returns_none_when_no_creation(self):
        root = self._setup(
            '{"type":"assistant","isSidechain":false,"message":{"usage":'
            '{"cache_read_input_tokens":1000}}}\n'
        )
        self.assertIsNone(detected_cache_ttl("s", "/x", root))

    def test_skips_sidechain_creations(self):
        # Sidechain (subagent) cache writes shouldn't dictate main TTL.
        root = self._setup(
            '{"type":"assistant","isSidechain":false,"message":{"usage":'
            '{"cache_creation":{"ephemeral_5m_input_tokens":133,"ephemeral_1h_input_tokens":0}}}}\n'
            '{"type":"assistant","isSidechain":true,"message":{"usage":'
            '{"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":500}}}}\n'
        )
        self.assertEqual(detected_cache_ttl("s", "/x", root), 300)


class TestSubagentCount(unittest.TestCase):
    def test_counts_agent_files(self):
        # Fixture has agent-aaa.jsonl and agent-bbb.jsonl
        self.assertEqual(subagent_count(SESSION, CWD, FIXTURES), 2)

    def test_missing_session_returns_zero(self):
        self.assertEqual(subagent_count("nope", CWD, FIXTURES), 0)

    def test_missing_subagents_dir_returns_zero(self):
        import tempfile
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "-x").mkdir()
            (root / "-x" / "only.jsonl").write_text("{}\n")
            self.assertEqual(subagent_count("only", "/x", root), 0)

    def test_ignores_non_agent_files(self):
        import tempfile
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            sub = root / "-x" / "s" / "subagents"
            sub.mkdir(parents=True)
            (sub / "agent-aaa.jsonl").write_text("")
            (sub / "agent-bbb.meta.json").write_text("{}")  # meta — skip
            (sub / "other.jsonl").write_text("")            # wrong prefix — skip
            self.assertEqual(subagent_count("s", "/x", root), 1)


if __name__ == "__main__":
    unittest.main()
