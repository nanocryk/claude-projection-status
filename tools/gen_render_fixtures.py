"""Generate render parity fixtures from the Python implementation.

Writes ``tests/fixtures/render_cases.json``: one case per layout situation,
each holding the view the Rust renderer is given and the line the Python
renderer produced for it.

The idle indicator is deliberately not covered here: it moved from line 1 to
line 3, so the two implementations no longer agree about it on purpose. Its
layout is asserted directly in the Rust renderer's own tests. Run from the
repository root:

    python tools/gen_render_fixtures.py

The clock glyph depends on the local hour, so the hour is frozen here and
carried in each case's ``ctx``.
"""

from __future__ import annotations

import json
import os
import sys
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FROZEN_HOUR = 14

# Keep the config file and the env overrides out of the fixtures.
os.environ["CLAUDE_STATUS_CONFIG"] = str(ROOT / "tools" / "no-such-config.json")
for var in ("CLAUDE_STATUS_WARNING", "CLAUDE_STATUS_CRITICAL"):
    os.environ.pop(var, None)

sys.path.insert(0, str(ROOT))

from claude_status import config, render  # noqa: E402


class _FrozenDatetime:
    @staticmethod
    def now():
        return datetime(2026, 9, 18, FROZEN_HOUR, 0, 0)


render.datetime = _FrozenDatetime


def samples(start: float, count: int, step: float, first_pct: float, per_step: float):
    return [
        {"at": start + index * step, "pct": first_pct + index * per_step}
        for index in range(count)
    ]


NOW = 1_758_000_000.0
FIVE_HOUR_SAMPLES = samples(NOW - 4 * 3600, 10, 1600.0, 2.0, 1.4)
SEVEN_DAY_SAMPLES = samples(NOW - 20 * 3600, 8, 9000.0, 1.0, 0.25)
LATE_SAMPLES = samples(NOW - 1200, 4, 400.0, 9.0, 1.1)

CASES: list[dict] = [
    {
        "name": "nominal",
        "view": {
            "five_hour": {
                "pct": 15.0, "projected": 23.0, "cooldown": "4h32m",
                "samples": FIVE_HOUR_SAMPLES, "confidence": "medium", "rate": 8.0,
            },
            "seven_day": {
                "pct": 2.0, "projected": 8.0, "cooldown": "6d02h",
                "samples": SEVEN_DAY_SAMPLES, "confidence": "low", "rate": 4.0,
            },
            "model": "Opus 5 (1M context)",
            "ctx_pct": 42.0,
            "ctx_size": 1_000_000,
        },
    },
    {
        "name": "no-data",
        "view": {
            "five_hour": {"cooldown": "   --"},
            "seven_day": {"cooldown": "   --"},
            "model": "Unknown",
        },
    },
    {
        "name": "projection-eta",
        "view": {
            "five_hour": {"pct": 3.0, "cooldown": "4h58m", "proj_eta": "7m"},
            "seven_day": {"pct": 1.0, "cooldown": "6d23h"},
            "model": "Sonnet 5",
            "ctx_pct": 12.0,
            "ctx_size": 200_000,
        },
    },
    {
        "name": "deadline",
        "view": {
            "five_hour": {
                "pct": 62.0, "projected": 96.0, "cooldown": "2h04m",
                "time_to_100": "2h30m", "confidence": "high", "rate": 22.0,
                "samples": FIVE_HOUR_SAMPLES,
            },
            "seven_day": {
                "pct": 74.0, "projected": 91.0, "cooldown": "1d06h",
                "time_to_100": "1d04h", "confidence": "high", "rate": 12.0,
                "samples": SEVEN_DAY_SAMPLES,
            },
            "model": "Opus 5",
            "ctx_pct": 66.0,
            "ctx_size": 1_000_000,
        },
    },
    {
        "name": "bypass",
        "view": {
            "five_hour": {"pct": 15.0, "projected": 23.0, "cooldown": "4h32m"},
            "seven_day": {"pct": 2.0, "projected": 8.0, "cooldown": "6d02h"},
            "model": "Opus 5",
            "bypass": True,
        },
    },
    {
        "name": "model-mix",
        "view": {
            "five_hour": {"pct": 15.0, "projected": 23.0, "cooldown": "4h32m"},
            "seven_day": {"pct": 2.0, "projected": 8.0, "cooldown": "6d02h"},
            "model": "Opus 5",
            "ctx_pct": 42.0,
            "ctx_size": 1_000_000,
            "model_shares": {"o": 0.62, "s": 0.31, "h": 0.07},
            "subagent_count": 3,
            "subagent_share": 0.18,
        },
    },
    {
        "name": "unknown-family",
        "view": {
            "five_hour": {"pct": 15.0, "cooldown": "4h32m"},
            "seven_day": {"pct": 2.0, "cooldown": "6d02h"},
            "model": "Opus 5",
            "model_shares": {"o": 0.52, "?": 0.48},
        },
    },
    {
        "name": "single-family-hidden",
        "view": {
            "five_hour": {"pct": 15.0, "cooldown": "4h32m"},
            "seven_day": {"pct": 2.0, "cooldown": "6d02h"},
            "model": "Opus 5",
            "model_shares": {"o": 1.0},
            "subagent_count": 2,
        },
    },
    {
        "name": "over-one-hundred",
        "view": {
            "five_hour": {
                "pct": 88.0, "projected": 137.0, "cooldown": "0h42m",
                "time_to_100": "18m", "confidence": "high", "rate": 34.0,
            },
            "seven_day": {"pct": 61.0, "projected": 104.0, "cooldown": "2d11h",
                          "time_to_100": "1d02h", "confidence": "medium", "rate": 21.0},
            "model": "Opus 5",
            "ctx_pct": 93.0,
            "ctx_size": 1_000_000,
        },
    },
    {
        "name": "long-model-name",
        "view": {
            "five_hour": {"pct": 15.0, "projected": 23.0, "cooldown": "4h32m"},
            "seven_day": {"pct": 2.0, "projected": 8.0, "cooldown": "6d02h"},
            "model": "Some Very Long Model Name v3 (1M context)",
            "ctx_pct": 91.0,
            "ctx_size": 1_000_000,
        },
    },
    {
        "name": "sparkline-partial-history",
        "view": {
            "five_hour": {"pct": 12.0, "projected": 19.0, "cooldown": "3h10m",
                          "samples": LATE_SAMPLES, "confidence": "low", "rate": 6.0},
            "seven_day": {"pct": 3.0, "cooldown": "5d20h", "samples": LATE_SAMPLES},
            "model": "Opus 5",
        },
    },
    {
        "name": "quiet-rates",
        "view": {
            "five_hour": {"pct": 4.0, "projected": 5.0, "cooldown": "4h02m", "rate": 0.2},
            "seven_day": {"pct": 1.0, "projected": 2.0, "cooldown": "6d12h", "rate": 0.4},
            "model": "Haiku 4.5",
            "ctx_pct": 8.0,
            "ctx_size": 200_000,
        },
    },
    {
        "name": "zero-usage",
        "view": {
            "five_hour": {"pct": 0.0, "projected": 0.0, "cooldown": "5h00m"},
            "seven_day": {"pct": 0.0, "projected": 0.0, "cooldown": "7d00h"},
            "model": "Opus 5",
            "ctx_pct": 0.0,
            "ctx_size": 1_000_000,
        },
    },
]


def to_python_kwargs(view: dict) -> dict:
    five = view.get("five_hour", {})
    seven = view.get("seven_day", {})

    def as_tuples(window: dict):
        return [(sample["at"], sample["pct"]) for sample in window.get("samples", [])]

    return {
        "pct_5h": five.get("pct"),
        "pct_7d": seven.get("pct"),
        "cooldown_5h": five.get("cooldown", ""),
        "cooldown_7d": seven.get("cooldown", ""),
        "proj_5h": five.get("projected"),
        "proj_7d": seven.get("projected"),
        "time_to_100_5h": five.get("time_to_100"),
        "time_to_100_7d": seven.get("time_to_100"),
        "model": view.get("model", ""),
        "ctx_pct": view.get("ctx_pct"),
        "ctx_size": view.get("ctx_size", 0),
        "bypass": view.get("bypass", False),
        "samples_5h": as_tuples(five),
        "samples_7d": as_tuples(seven),
        "conf_5h": five.get("confidence"),
        "conf_7d": seven.get("confidence"),
        "rate_per_h": five.get("rate"),
        "rate_per_d": seven.get("rate"),
        "proj_eta": five.get("proj_eta"),
        "model_shares": view.get("model_shares", {}),
        "subagent_count": view.get("subagent_count", 0),
        "subagent_share": view.get("subagent_share", 0.0),
        "idle_sec": view.get("idle_sec"),
        "cache_ttl": view.get("cache_ttl"),
    }


def main() -> int:
    if (config.WARNING_PCT, config.CRITICAL_PCT) != (40.0, 70.0):
        print(
            f"refusing to bake non-default thresholds "
            f"({config.WARNING_PCT}, {config.CRITICAL_PCT}) into fixtures",
            file=sys.stderr,
        )
        return 1

    ctx = {
        "local_hour": FROZEN_HOUR,
        "warning_pct": config.WARNING_PCT,
        "critical_pct": config.CRITICAL_PCT,
    }
    out = []
    for case in CASES:
        expected = render.render_status_line(**to_python_kwargs(case["view"]))
        out.append({"name": case["name"], "ctx": ctx, "view": case["view"],
                    "expected": expected})

    target = ROOT / "tests" / "fixtures" / "render_cases.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n",
                      encoding="utf-8")
    print(f"wrote {len(out)} cases to {target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
