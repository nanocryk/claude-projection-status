"""Tests for per-model token-share computation."""

import unittest
from pathlib import Path

from claude_status import transcript
from claude_status.transcript import (
    format_mix,
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
