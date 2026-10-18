#!/usr/bin/env python3
"""Metrics of an agent run, from its transcript (standard library only).

Usage:
  agent_metrics.py TRANSCRIPT --client claude-code|codex|auto
                   [--engine-report DIR] [--prices FILE.json] [--turns-only]

TRANSCRIPT is one of:
  * Claude Code: `claude -p --output-format stream-json --verbose` (JSONL);
  * Codex: `codex exec --json` (JSONL events), or a Codex session rollout
    (`~/.codex/sessions/.../rollout-*.jsonl`).

Prints one JSON object:
  client, format, turns, duration_s, engine_s, agent_s,
  calls {tool: n}, calls_total, arg_bytes, result_bytes, images,
  image_bytes_b64, tokens {input, cache_read, cache_write, output, reasoning},
  cost_usd, cost_source, notes [...]

`tokens.input` never includes cached tokens (Codex reports cached tokens as a
part of its input; they are subtracted so both clients mean the same).
A field that the client's format does not carry is null and says why in
`notes`: nothing is invented. `--turns-only` prints just the number of turns
so far (used by run_agent.sh to enforce the turn cap).

Engine time (`engine_s`): the sum of the stage times found in the JSON files
of `--engine-report DIR`: a mode C `results.json` (`stages[].ms`) or any
report with `timings_ms` {stage: ms} or `elapsed_ms`. The agent's time is the
run's duration minus the engine's.

Cost: the client's own figure when it reports one (`cost_source`
"reported"); otherwise tokens times PRICES (USD per million tokens, per model
name), `cost_source` "computed"; otherwise null. PRICES is versioned here:
add the models you run, or pass `--prices FILE.json` with the same shape.
"""

import argparse
import glob
import json
import os
import sys

# USD per million tokens, per model. Fill in from the provider's price list
# and bump `date`; a model that is not here has no computed cost.
PRICES = {
    "date": "2026-10-18",
    "unit": "USD per million tokens",
    "models": {
        # "model-name": {"input": 0.0, "cache_read": 0.0, "cache_write": 0.0,
        #                "output": 0.0},
    },
}

CLIENTS = ("claude-code", "codex")


# --------------------------------------------------------------------- utils


def read_events(path):
    events = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                value = json.loads(line)
            except ValueError:
                continue
            if isinstance(value, dict):
                events.append(value)
    return events


def compact_bytes(value):
    return len(json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode("utf-8"))


def tool_name(raw):
    """`mcp__forma__glyph_get` -> `glyph_get`; other names are kept."""
    if isinstance(raw, str) and raw.startswith("mcp__"):
        parts = raw.split("__", 2)
        if len(parts) == 3:
            return parts[2]
    return raw or "?"


def num(value):
    return value if isinstance(value, (int, float)) and not isinstance(value, bool) else None


def detect_client(events):
    for event in events[:20]:
        kind = event.get("type")
        if kind in ("system", "assistant", "user", "result"):
            return "claude-code"
        if kind in ("session_meta", "response_item", "event_msg", "turn_context") or str(
            kind
        ).startswith(("thread.", "turn.", "item.")):
            return "codex"
    return None


class Totals:
    def __init__(self):
        self.calls = {}
        self.arg_bytes = 0
        self.result_bytes = 0
        self.images = 0
        self.image_bytes = 0
        self.notes = []

    def call(self, name, arguments):
        self.calls[name] = self.calls.get(name, 0) + 1
        self.arg_bytes += compact_bytes(arguments if arguments is not None else {})

    def result_parts(self, parts):
        """Counts the text bytes and the images of a tool result."""
        if isinstance(parts, str):
            self.result_bytes += len(parts.encode("utf-8"))
            return
        if not isinstance(parts, list):
            return
        for part in parts:
            if isinstance(part, str):
                self.result_bytes += len(part.encode("utf-8"))
            elif isinstance(part, dict):
                kind = part.get("type")
                if kind in ("text", "input_text", "output_text"):
                    self.result_bytes += len(str(part.get("text", "")).encode("utf-8"))
                elif kind in ("image", "input_image"):
                    self.images += 1
                    data = (part.get("source") or {}).get("data")
                    if data is None:
                        data = part.get("image_url") or part.get("data") or ""
                    self.image_bytes += len(str(data))

    def finish(self):
        return {
            "calls": dict(sorted(self.calls.items())),
            "calls_total": sum(self.calls.values()),
            "arg_bytes": self.arg_bytes,
            "result_bytes": self.result_bytes,
            "images": self.images,
            "image_bytes_b64": self.image_bytes,
        }


# ------------------------------------------------------------------ Claude Code


def parse_claude(events):
    totals = Totals()
    notes = totals.notes
    result = None
    messages = {}  # message id -> (model, usage), the last one wins
    order = []
    for event in events:
        kind = event.get("type")
        message = event.get("message")
        if kind == "result":
            result = event
        elif kind == "assistant" and isinstance(message, dict):
            mid = message.get("id") or "n%d" % len(order)
            if mid not in messages:
                order.append(mid)
            usage = message.get("usage")
            messages[mid] = (message.get("model"), usage if isinstance(usage, dict) else {})
            for block in message.get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    totals.call(tool_name(block.get("name")), block.get("input"))
        elif kind == "user" and isinstance(message, dict):
            content = message.get("content")
            if isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "tool_result":
                        totals.result_parts(block.get("content"))

    model = next((m for m, _ in messages.values() if m), None)
    tokens = {"input": None, "cache_read": None, "cache_write": None, "output": None, "reasoning": None}
    usage = result.get("usage") if isinstance(result, dict) else None
    if isinstance(usage, dict):
        tokens["input"] = num(usage.get("input_tokens"))
        tokens["cache_read"] = num(usage.get("cache_read_input_tokens"))
        tokens["cache_write"] = num(usage.get("cache_creation_input_tokens"))
        tokens["output"] = num(usage.get("output_tokens"))
        details = usage.get("output_tokens_details")
        tokens["reasoning"] = num(details.get("thinking_tokens")) if isinstance(details, dict) else None
        if tokens["reasoning"] is None:
            notes.append("tokens.reasoning: the result event has no thinking_tokens")
    elif messages:
        notes.append("no result event: tokens summed from the usage of each assistant message")
        for key, source in (
            ("input", "input_tokens"),
            ("cache_read", "cache_read_input_tokens"),
            ("cache_write", "cache_creation_input_tokens"),
            ("output", "output_tokens"),
        ):
            tokens[key] = sum(num(u.get(source)) or 0 for _, u in messages.values())
        notes.append("tokens.reasoning: not reported per message")
    else:
        notes.append("no assistant messages: tokens unknown")

    turns = num(result.get("num_turns")) if isinstance(result, dict) else None
    if turns is None:
        turns = len(order) if order else None
        notes.append("turns: no result event; counted assistant messages")
    duration = None
    if isinstance(result, dict) and num(result.get("duration_ms")) is not None:
        duration = result["duration_ms"] / 1000.0
    else:
        notes.append("duration_s: no result event")

    reported = num(result.get("total_cost_usd")) if isinstance(result, dict) else None
    return {
        "client": "claude-code",
        "format": "stream-json",
        "model": model,
        "turns": turns,
        "duration_s": duration,
        "tokens": tokens,
        "reported_cost_usd": reported,
        **totals.finish(),
        "notes": notes,
    }


def count_turns_claude(events):
    result = [e for e in events if e.get("type") == "result"]
    if result and num(result[-1].get("num_turns")) is not None:
        return int(result[-1]["num_turns"])
    ids = set()
    for event in events:
        message = event.get("message")
        if event.get("type") == "assistant" and isinstance(message, dict):
            ids.add(message.get("id") or id(message))
    return len(ids)


# ------------------------------------------------------------------------ Codex


def norm_kind(kind):
    return str(kind or "").replace("_", ".").lower()


def codex_usage_to_tokens(usage, tokens, notes):
    total_in = num(usage.get("input_tokens"))
    cached = num(usage.get("cached_input_tokens"))
    tokens["input"] = None if total_in is None else total_in - (cached or 0)
    tokens["cache_read"] = cached
    tokens["cache_write"] = num(usage.get("cache_write_input_tokens"))
    tokens["output"] = num(usage.get("output_tokens"))
    tokens["reasoning"] = num(usage.get("reasoning_output_tokens"))
    for key in ("cache_write", "reasoning"):
        if tokens[key] is None:
            notes.append("tokens.%s: the Codex usage has no such field" % key)


def codex_item_call(totals, item):
    kind = norm_kind(item.get("type"))
    if kind in ("mcp.tool.call", "mcptoolcall"):
        totals.call(tool_name(item.get("tool")), item.get("arguments"))
        result = item.get("result")
        if isinstance(result, dict):
            totals.result_parts(result.get("content"))
        return True
    if kind in ("command.execution", "commandexecution"):
        totals.call("shell", {"command": item.get("command")})
        output = item.get("aggregated_output")
        if output is None:
            output = item.get("output")
        totals.result_parts(output if isinstance(output, str) else "")
        return True
    return False


def parse_codex(events):
    totals = Totals()
    notes = totals.notes
    tokens = {"input": None, "cache_read": None, "cache_write": None, "output": None, "reasoning": None}
    is_rollout = any(e.get("type") in ("session_meta", "response_item", "event_msg") for e in events)
    duration = None
    turns = None
    model = None
    if is_rollout:
        fmt = "rollout"
        last_total = None
        token_events = 0
        used_items = False
        for event in events:
            payload = event.get("payload") if isinstance(event.get("payload"), dict) else {}
            kind = payload.get("type")
            if event.get("type") == "turn_context" and payload.get("model"):
                model = payload.get("model")
            if event.get("type") == "event_msg":
                if kind == "token_count":
                    info = payload.get("info")
                    if isinstance(info, dict) and isinstance(info.get("total_token_usage"), dict):
                        last_total = info["total_token_usage"]
                        token_events += 1
                elif kind == "task_complete" and num(payload.get("duration_ms")) is not None:
                    duration = (duration or 0.0) + payload["duration_ms"] / 1000.0
                elif kind == "item_completed" and isinstance(payload.get("item"), dict):
                    used_items = codex_item_call(totals, payload["item"]) or used_items
        if not used_items:
            # Older rollouts: only the response items (function calls and outputs).
            for event in events:
                payload = event.get("payload") if isinstance(event.get("payload"), dict) else {}
                if event.get("type") != "response_item":
                    continue
                kind = payload.get("type")
                if kind == "function_call":
                    args = payload.get("arguments")
                    try:
                        args = json.loads(args) if isinstance(args, str) else args
                    except ValueError:
                        pass
                    namespace = payload.get("namespace") or ""
                    name = payload.get("name")
                    totals.call(tool_name(namespace + "__" + name if namespace.startswith("mcp__") else name), args)
                elif kind == "custom_tool_call":
                    totals.call(payload.get("name") or "custom_tool", payload.get("input"))
                elif kind in ("function_call_output", "custom_tool_call_output"):
                    totals.result_parts(payload.get("output"))
        if last_total is not None:
            codex_usage_to_tokens(last_total, tokens, notes)
            turns = token_events
        else:
            notes.append("no token_count event: tokens unknown")
        if duration is None:
            notes.append("duration_s: no task_complete event")
    else:
        fmt = "exec-json"
        usage_sum = {}
        completed = 0
        for event in events:
            kind = norm_kind(event.get("type"))
            if kind == "turn.completed" and isinstance(event.get("usage"), dict):
                completed += 1
                for key, value in event["usage"].items():
                    if num(value) is not None:
                        usage_sum[key] = usage_sum.get(key, 0) + value
                if num(event.get("duration_ms")) is not None:
                    duration = (duration or 0.0) + event["duration_ms"] / 1000.0
            elif kind == "item.completed" and isinstance(event.get("item"), dict):
                codex_item_call(totals, event["item"])
        if usage_sum:
            codex_usage_to_tokens(usage_sum, tokens, notes)
        else:
            notes.append("no turn.completed usage: tokens unknown")
        turns = totals.finish()["calls_total"] + 1 if completed else None
        notes.append("turns: `codex exec --json` has no per-request events; counted tool calls plus the final message")
        if duration is None:
            notes.append("duration_s: `codex exec --json` carries no duration; use the wall clock of the run")
    notes.append("cost: Codex does not report a cost")
    return {
        "client": "codex",
        "format": fmt,
        "model": model,
        "turns": turns,
        "duration_s": duration,
        "tokens": tokens,
        "reported_cost_usd": None,
        **totals.finish(),
        "notes": notes,
    }


def count_turns_codex(events):
    rollout = [
        e
        for e in events
        if e.get("type") == "event_msg" and (e.get("payload") or {}).get("type") == "token_count"
    ]
    if rollout:
        return len(rollout)
    calls = 0
    for event in events:
        if norm_kind(event.get("type")) == "item.completed" and isinstance(event.get("item"), dict):
            kind = norm_kind(event["item"].get("type"))
            calls += kind in ("mcp.tool.call", "mcptoolcall", "command.execution", "commandexecution")
    return calls


# ------------------------------------------------------------- engine and cost


def engine_seconds(directory):
    """(seconds or None, notes) from the engine reports in `directory`."""
    notes = []
    if not directory:
        return None, ["engine_s: no --engine-report given"]
    files = sorted(glob.glob(os.path.join(directory, "**", "*.json"), recursive=True))
    total = 0.0
    found = 0
    for path in files:
        try:
            with open(path, encoding="utf-8") as handle:
                data = json.load(handle)
        except (OSError, ValueError):
            continue
        if not isinstance(data, dict):
            continue
        if isinstance(data.get("stages"), list):
            values = [s.get("ms") for s in data["stages"] if isinstance(s, dict)]
            values = [v for v in values if num(v) is not None]
            if values:
                total += sum(values) / 1000.0
                found += 1
                continue
        if isinstance(data.get("timings_ms"), dict):
            values = [v for v in data["timings_ms"].values() if num(v) is not None]
            if values:
                total += sum(values) / 1000.0
                found += 1
                continue
        if num(data.get("elapsed_ms")) is not None:
            total += data["elapsed_ms"] / 1000.0
            found += 1
    if not found:
        return None, ["engine_s: no stage times in %s" % directory]
    notes.append("engine_s: summed from %d report file(s)" % found)
    return total, notes


def compute_cost(summary, prices):
    """(cost, source, note) from the tokens and the price table."""
    reported = summary.get("reported_cost_usd")
    if reported is not None:
        return reported, "reported", None
    model = summary.get("model")
    table = (prices.get("models") or {}).get(model) if model else None
    if not table:
        return None, "unavailable", "cost_usd: no price for model %r in PRICES (%s)" % (model, prices.get("date"))
    tokens = summary["tokens"]
    parts = (("input", "input"), ("cache_read", "cache_read"), ("cache_write", "cache_write"), ("output", "output"))
    cost = 0.0
    for key, price_key in parts:
        amount = tokens.get(key)
        price = table.get(price_key)
        if amount is None or price is None:
            if amount:
                return None, "unavailable", "cost_usd: missing %s tokens or price" % key
            continue
        cost += amount * price / 1_000_000.0
    return cost, "computed", "cost_usd: computed with PRICES dated %s" % prices.get("date")


def analyze(path, client="auto", engine_report=None, prices=None):
    events = read_events(path)
    notes_head = []
    if client == "auto":
        client = detect_client(events)
        if client is None:
            raise SystemExit("cannot tell the client of %s; pass --client" % path)
    if client not in CLIENTS:
        raise SystemExit("unknown client %r" % client)
    summary = parse_claude(events) if client == "claude-code" else parse_codex(events)
    engine_s, engine_notes = engine_seconds(engine_report)
    duration = summary["duration_s"]
    summary["engine_s"] = engine_s
    summary["agent_s"] = None if engine_s is None or duration is None else max(duration - engine_s, 0.0)
    cost, source, note = compute_cost(summary, prices or PRICES)
    summary["cost_usd"] = cost
    summary["cost_source"] = source
    del summary["reported_cost_usd"]
    summary["notes"] = notes_head + summary["notes"] + engine_notes + ([note] if note else [])
    ordered = [
        "client", "format", "model", "turns", "duration_s", "engine_s", "agent_s", "calls",
        "calls_total", "arg_bytes", "result_bytes", "images", "image_bytes_b64", "tokens",
        "cost_usd", "cost_source", "notes",
    ]
    return {key: summary[key] for key in ordered}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("transcript")
    parser.add_argument("--client", default="auto", choices=("auto",) + CLIENTS)
    parser.add_argument("--engine-report", default=None)
    parser.add_argument("--prices", default=None, help="JSON file shaped like PRICES")
    parser.add_argument("--turns-only", action="store_true")
    args = parser.parse_args(argv)
    if args.turns_only:
        events = read_events(args.transcript)
        client = args.client if args.client != "auto" else detect_client(events)
        count = count_turns_claude(events) if client == "claude-code" else count_turns_codex(events)
        print(count)
        return 0
    prices = None
    if args.prices:
        with open(args.prices, encoding="utf-8") as handle:
            prices = json.load(handle)
    summary = analyze(args.transcript, args.client, args.engine_report, prices)
    json.dump(summary, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
