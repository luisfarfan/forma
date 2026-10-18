"""Tests of agent_metrics.py with small hand-written synthetic transcripts
(tools/bench/fixtures; none of them is a real transcript)."""

import json
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import agent_metrics  # noqa: E402

FIXTURES = os.path.join(HERE, "fixtures")


def fixture(name):
    return os.path.join(FIXTURES, name)


class ClaudeTests(unittest.TestCase):
    def test_claude_stream_json_counts(self):
        s = agent_metrics.analyze(fixture("claude_stream.jsonl"), "claude-code")
        self.assertEqual(s["client"], "claude-code")
        self.assertEqual(s["turns"], 3)
        self.assertEqual(s["duration_s"], 12.5)
        self.assertEqual(
            s["tokens"],
            {"input": 30, "cache_read": 300, "cache_write": 150, "output": 60, "reasoning": 15},
        )
        self.assertEqual(s["calls"], {"ToolSearch": 1, "image_inspect": 1, "image_trace": 1})
        self.assertEqual(s["calls_total"], 3)
        self.assertEqual(s["images"], 1)
        self.assertEqual(s["image_bytes_b64"], 8)
        # Text of the results: {"rows":1} (10 bytes) + {"rev":1} (9 bytes).
        self.assertEqual(s["result_bytes"], 19)
        self.assertEqual(
            s["arg_bytes"],
            len(b'{"path":"/x.png"}') + len(b'{"doc":"d","base_rev":0}') + len(b'{"query":"select:a"}'),
        )
        self.assertEqual(s["cost_usd"], 0.25)
        self.assertEqual(s["cost_source"], "reported")

    def test_missing_fields_are_null_with_note(self):
        s = agent_metrics.analyze(fixture("claude_no_result.jsonl"), "claude-code")
        self.assertIsNone(s["duration_s"])
        self.assertIsNone(s["tokens"]["reasoning"])
        # A message id repeated by the stream counts once, with its last usage.
        self.assertEqual(s["turns"], 2)
        self.assertEqual(s["tokens"]["input"], 1 + 4)
        self.assertEqual(s["tokens"]["output"], 5 + 7)
        self.assertIsNone(s["cost_usd"])
        self.assertEqual(s["cost_source"], "unavailable")
        joined = " ".join(s["notes"])
        self.assertIn("no result event", joined)
        self.assertIn("duration_s", joined)
        self.assertIn("no price for model", joined)

    def test_cost_is_computed_with_a_price_table(self):
        prices = {
            "date": "test",
            "models": {
                "claude-test-1": {"input": 3.0, "cache_read": 0.3, "cache_write": 6.0, "output": 15.0}
            },
        }
        s = agent_metrics.analyze(fixture("claude_no_result.jsonl"), "claude-code", prices=prices)
        t = s["tokens"]
        expected = (t["input"] * 3.0 + t["cache_read"] * 0.3 + t["cache_write"] * 6.0 + t["output"] * 15.0) / 1e6
        self.assertAlmostEqual(s["cost_usd"], expected)
        self.assertEqual(s["cost_source"], "computed")

    def test_turns_only(self):
        events = agent_metrics.read_events(fixture("claude_stream.jsonl"))
        self.assertEqual(agent_metrics.count_turns_claude(events), 3)


class CodexTests(unittest.TestCase):
    def test_codex_json_counts(self):
        s = agent_metrics.analyze(fixture("codex_exec.jsonl"), "codex")
        self.assertEqual(s["format"], "exec-json")
        # Cached tokens are not part of `input`.
        self.assertEqual(s["tokens"]["input"], 600)
        self.assertEqual(s["tokens"]["cache_read"], 400)
        self.assertEqual(s["tokens"]["output"], 50)
        self.assertEqual(s["calls"], {"image_inspect": 1, "shell": 1})
        self.assertEqual(s["result_bytes"], len('{"rows":1}') + len("a\nb\n"))
        self.assertIsNone(s["tokens"]["reasoning"])
        self.assertIsNone(s["tokens"]["cache_write"])
        self.assertIsNone(s["cost_usd"])
        self.assertTrue(any("reasoning" in n for n in s["notes"]))

    def test_codex_rollout_counts(self):
        s = agent_metrics.analyze(fixture("codex_rollout.jsonl"), "codex")
        self.assertEqual(s["format"], "rollout")
        self.assertEqual(s["model"], "test-model")
        # The last cumulative total wins.
        self.assertEqual(s["tokens"]["input"], 100)
        self.assertEqual(s["tokens"]["cache_read"], 200)
        self.assertEqual(s["tokens"]["output"], 30)
        self.assertEqual(s["tokens"]["reasoning"], 9)
        self.assertEqual(s["turns"], 2)
        self.assertEqual(s["duration_s"], 8.0)
        self.assertEqual(s["calls"], {"image_trace": 1})
        self.assertEqual(s["images"], 1)
        self.assertEqual(s["result_bytes"], 4)

    def test_client_is_detected(self):
        for name, client in (
            ("claude_stream.jsonl", "claude-code"),
            ("codex_exec.jsonl", "codex"),
            ("codex_rollout.jsonl", "codex"),
        ):
            events = agent_metrics.read_events(fixture(name))
            self.assertEqual(agent_metrics.detect_client(events), client, name)


class EngineTests(unittest.TestCase):
    def test_engine_time_is_summed_and_subtracted(self):
        s = agent_metrics.analyze(
            fixture("claude_stream.jsonl"), "claude-code", engine_report=fixture("engine_report")
        )
        self.assertAlmostEqual(s["engine_s"], 2.25)
        self.assertAlmostEqual(s["agent_s"], 12.5 - 2.25)

    def test_no_engine_report_gives_null(self):
        s = agent_metrics.analyze(fixture("claude_stream.jsonl"), "claude-code")
        self.assertIsNone(s["engine_s"])
        self.assertIsNone(s["agent_s"])
        self.assertTrue(any("engine_s" in n for n in s["notes"]))


class CommandLineTests(unittest.TestCase):
    def run_cli(self, *args):
        return subprocess.run(
            [sys.executable, "-I", os.path.join(HERE, "agent_metrics.py"), *args],
            check=True,
            capture_output=True,
            text=True,
        ).stdout

    def test_prints_json(self):
        out = json.loads(self.run_cli(fixture("claude_stream.jsonl"), "--client", "claude-code"))
        self.assertEqual(out["turns"], 3)
        self.assertEqual(list(out)[:3], ["client", "format", "model"])

    def test_turns_only(self):
        self.assertEqual(self.run_cli(fixture("codex_rollout.jsonl"), "--turns-only").strip(), "2")


if __name__ == "__main__":
    unittest.main()
