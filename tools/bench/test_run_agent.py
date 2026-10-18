"""Dry-run tests of run_agent.sh: no client is launched."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "run_agent.sh")


def read(path):
    with open(path, encoding="utf-8") as handle:
        return handle.read()


@unittest.skipUnless(shutil.which("bash") and os.name == "posix", "needs bash")
class RunAgentDryRun(unittest.TestCase):
    def run_script(self, client, mode, tmp, **extra_env):
        image = os.path.join(tmp, "case.png")
        with open(image, "wb") as handle:
            handle.write(b"\x89PNG synthetic")
        env = dict(
            os.environ,
            BENCH_OUT=os.path.join(tmp, "bench-out"),
            CASE_IMAGE=image,
            CASE_LAYOUT="ABC|DEF",
            FORMA_BIN=sys.executable,
            RUN_AGENT_DRY_RUN="1",
            MAX_TURNS="7",
            **extra_env,
        )
        done = subprocess.run(
            ["bash", SCRIPT, client, mode, "specimen", "1"],
            env=env,
            capture_output=True,
            text=True,
        )
        return done, os.path.join(tmp, "bench-out", "%s-%s-specimen-r1" % (client, mode))

    def test_claude_mode_a(self):
        with tempfile.TemporaryDirectory() as tmp:
            done, run_dir = self.run_script("claude-code", "a", tmp)
            self.assertEqual(done.returncode, 0, done.stderr)
            self.assertIn("transcript: " + os.path.join(run_dir, "transcript.jsonl"), done.stdout)
            self.assertIn("--output-format stream-json", done.stdout.replace("\\", ""))
            self.assertIn("(turn cap 7", done.stdout)
            mcp = json.loads(read(os.path.join(run_dir, "mcp.json")))
            args = mcp["mcpServers"]["forma"]["args"]
            self.assertEqual(args[0], "mcp")
            self.assertEqual(args[args.index("--policy") + 1], "auto")
            prompt = read(os.path.join(run_dir, "prompt.txt"))
            self.assertIn('"ABC" / "DEF"', prompt)
            self.assertNotIn("{{", prompt)
            meta = json.loads(read(os.path.join(run_dir, "meta.json")))
            self.assertTrue(meta["dry_run"])
            self.assertEqual(meta["mode"], "a")

    def test_codex_mode_b_uses_prompted_and_extra_args(self):
        with tempfile.TemporaryDirectory() as tmp:
            done, run_dir = self.run_script(
                "codex", "b", tmp, FORMA_MCP_EXTRA="--profile image-font"
            )
            self.assertEqual(done.returncode, 0, done.stderr)
            mcp = json.loads(read(os.path.join(run_dir, "mcp.json")))
            args = mcp["mcpServers"]["forma"]["args"]
            self.assertEqual(args[args.index("--policy") + 1], "prompted")
            self.assertEqual(args[-2:], ["--profile", "image-font"])
            self.assertIn("codex exec", done.stdout.replace("\\", ""))
            prompt = read(os.path.join(run_dir, "prompt.txt"))
            self.assertIn("font_from_image", prompt)

    def test_bad_arguments_print_usage(self):
        done = subprocess.run(["bash", SCRIPT, "claude-code"], capture_output=True, text=True)
        self.assertEqual(done.returncode, 2)
        self.assertIn("run_agent.sh", done.stderr)


if __name__ == "__main__":
    unittest.main()
