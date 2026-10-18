#!/usr/bin/env bash
# Runs one image-to-font task with an agent client and keeps its transcript.
#
#   run_agent.sh <client> <mode> <case> <rep>
#
#   client  claude-code | codex
#   mode    a (granular tools, policy auto, as in Milestone 2)
#           b (optimized pipeline, policy prompted)
#   case    a case id of the corpus (`forma-bench list --level s|m|m-hand|l|o`)
#   rep     repetition number (names the run folder)
#
# Everything goes to bench-out/<client>-<mode>-<case>-r<rep>/ (git-ignored):
#   prompt.txt  mcp.json  work/ (image, document, exports)  audit.jsonl
#   transcript.jsonl (the client's JSON event stream)  stderr.log
#   rollout.jsonl (Codex only: its session file, richer than the stream)
#   meta.json  metrics.json (from agent_metrics.py)
# and prints where the transcript went.
#
# Environment:
#   MAX_TURNS     turn cap (default 60). Claude Code also gets --max-turns when
#                 its CLI has the flag; the cap is enforced here for both by
#                 counting turns in the transcript and stopping the client.
#   MAX_SECONDS   wall-clock cap (default 1800)
#   MAX_USD       Claude Code --max-budget-usd (default 3)
#   MODEL         model name passed to the client (optional)
#   FORMA_BIN     the `forma` binary (default: core/target/{release,debug}, then PATH)
#   FORMA_BENCH_BIN  the `forma-bench` binary (same search)
#   FORMA_MCP_EXTRA  extra arguments for `forma mcp` (e.g. "--profile image-font")
#   POLICY        the server's approval policy instead of the mode's (auto,
#                 prompted, strict): auto stands for an operator who approved
#                 in advance (one human intervention)
#   CLAUDE_EXTRA_ARGS / CODEX_EXTRA_ARGS  extra client arguments
#   BENCH_OUT     output base (default <repo>/bench-out)
#   CASE_IMAGE, CASE_LAYOUT  use this image and layout ("ABC|DEF") instead of
#                 exporting the case from forma-bench
#   RUN_AGENT_DRY_RUN=1  prepare everything and print the commands, run nothing
#
# Never put images of third parties under version control: the output folder is
# git-ignored on purpose.

set -euo pipefail

usage() {
  sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

[ $# -eq 4 ] || usage
client=$1
mode=$2
case_id=$3
rep=$4

case "$client" in
  claude-code | codex) ;;
  *) usage ;;
esac
case "$mode" in
  a | A) mode=a ;;
  b | B) mode=b ;;
  *) usage ;;
esac

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out_base=${BENCH_OUT:-$root/bench-out}
max_turns=${MAX_TURNS:-60}
max_seconds=${MAX_SECONDS:-1800}
max_usd=${MAX_USD:-3}
dry_run=${RUN_AGENT_DRY_RUN:-0}

find_bin() {
  # find_bin <override> <name>
  if [ -n "$1" ]; then
    printf '%s\n' "$1"
    return
  fi
  for profile in release debug; do
    if [ -x "$root/core/target/$profile/$2" ]; then
      printf '%s\n' "$root/core/target/$profile/$2"
      return
    fi
  done
  command -v "$2" || true
}

forma_bin=$(find_bin "${FORMA_BIN:-}" forma)
bench_bin=$(find_bin "${FORMA_BENCH_BIN:-}" forma-bench)

mkdir -p "$out_base"
out_base=$(cd "$out_base" && pwd)
run_dir="$out_base/$client-$mode-$case_id-r$rep"
work="$run_dir/work"
mkdir -p "$work"
transcript="$run_dir/transcript.jsonl"

# --- the case: image and layout -------------------------------------------
if [ -n "${CASE_IMAGE:-}" ]; then
  image="$work/$(basename "$CASE_IMAGE")"
  cp "$CASE_IMAGE" "$image"
  layout=${CASE_LAYOUT:-}
else
  [ -n "$bench_bin" ] || { echo "run_agent.sh: forma-bench not found (build it or set FORMA_BENCH_BIN)" >&2; exit 1; }
  image=$("$bench_bin" export-case --case "$case_id" --out "$work")
  layout=$(tr '\n' '|' <"$work/$case_id.layout.txt")
  layout=${layout%|}
fi
[ -n "$layout" ] || layout="(unknown: read it from image_inspect)"

# --- the prompt ------------------------------------------------------------
python3 - "$here/prompts/mode_$mode.txt" "$run_dir/prompt.txt" "$image" "$layout" "$work" "$case_id" <<'PY'
import sys
template, out, image, layout, work, case = sys.argv[1:7]
text = open(template, encoding="utf-8").read()
rows = " / ".join('"%s"' % r for r in layout.split("|"))
for key, value in (("IMAGE", image), ("LAYOUT", rows), ("WORK_DIR", work), ("CASE", case)):
    text = text.replace("{{%s}}" % key, value)
open(out, "w", encoding="utf-8").write(text)
PY

# --- the MCP server ----------------------------------------------------------
if [ "$mode" = a ]; then policy=auto; else policy=prompted; fi
# A headless client cannot answer an approval itself (Claude Code refuses to
# let the agent commit plans meant for a person), and a second stdio client
# cannot commit this server's plans: POLICY=auto stands for the operator
# approving in advance, counted as one human intervention.
policy=${POLICY:-$policy}
# shellcheck disable=SC2206
mcp_extra=(${FORMA_MCP_EXTRA:-})
mcp_args=(mcp --allow-dir "$work" --policy "$policy" --audit-log "$run_dir/audit.jsonl" ${mcp_extra[@]+"${mcp_extra[@]}"})
[ -n "$forma_bin" ] || { echo "run_agent.sh: forma not found (build it or set FORMA_BIN)" >&2; exit 1; }
python3 - "$run_dir/mcp.json" "$forma_bin" "${mcp_args[@]}" <<'PY'
import json, sys
out, command, *args = sys.argv[1:]
json.dump({"mcpServers": {"forma": {"command": command, "args": args}}}, open(out, "w"), indent=2)
PY

prompt=$(cat "$run_dir/prompt.txt")

# --- the client ----------------------------------------------------------------
if [ "$client" = claude-code ]; then
  # shellcheck disable=SC2206
  extra=(${CLAUDE_EXTRA_ARGS:-})
  cmd=(claude -p "$prompt" --output-format stream-json --verbose
    --mcp-config "$run_dir/mcp.json" --strict-mcp-config
    --allowedTools mcp__forma --add-dir "$work" --max-budget-usd "$max_usd")
  if claude --help 2>&1 | grep -q -- '--max-turns'; then
    cmd+=(--max-turns "$max_turns")
  fi
  [ -z "${MODEL:-}" ] || cmd+=(--model "$MODEL")
  cmd+=(${extra[@]+"${extra[@]}"})
else
  # shellcheck disable=SC2206
  extra=(${CODEX_EXTRA_ARGS:-})
  toml_args=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${mcp_args[@]}")
  toml_cmd=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$forma_bin")
  cmd=(codex exec --json --skip-git-repo-check -C "$work" -s workspace-write
    -c 'approval_policy="never"'
    -c "mcp_servers.forma.command=$toml_cmd"
    -c "mcp_servers.forma.args=$toml_args"
    # Codex asks before running a tool marked destructive, and in `exec`
    # nobody answers: the FORMA server's own approvals (its policy, plans
    # and tokens) govern its tools instead. Only this server; the sandbox
    # stays as it is.
    -c 'mcp_servers.forma.default_tools_approval_mode="approve"')
  [ -z "${MODEL:-}" ] || cmd+=(-m "$MODEL")
  cmd+=(${extra[@]+"${extra[@]}"} "$prompt")
fi

started=$(date -u +%Y-%m-%dT%H:%M:%SZ)
marker="$run_dir/.started"
: >"$marker"

if [ "$dry_run" = 1 ]; then
  printf 'DRY RUN: not launching the client. Command:\n'
  printf ' %q' "${cmd[@]}"
  printf '\n(turn cap %s, wall cap %ss)\n' "$max_turns" "$max_seconds"
  : >"$transcript"
  exit_code=0
  capped=false
  timed_out=false
else
  : >"$transcript"
  # stdin closed: `codex exec` otherwise waits for more input on it.
  (cd "$work" && "${cmd[@]}" </dev/null >"$transcript" 2>"$run_dir/stderr.log") &
  pid=$!
  capped=false
  timed_out=false
  start_s=$(date +%s)
  while kill -0 "$pid" 2>/dev/null; do
    sleep 2
    turns=$(python3 "$here/agent_metrics.py" "$transcript" --client "$client" --turns-only 2>/dev/null || echo 0)
    if [ "${turns:-0}" -ge "$max_turns" ]; then
      capped=true
    fi
    if [ $(($(date +%s) - start_s)) -ge "$max_seconds" ]; then
      timed_out=true
    fi
    if [ "$capped" = true ] || [ "$timed_out" = true ]; then
      echo "run_agent.sh: stopping the client (turns=${turns:-?}, cap=$max_turns, timeout=$timed_out)" >&2
      kill "$pid" 2>/dev/null || true
      sleep 5
      kill -9 "$pid" 2>/dev/null || true
      break
    fi
  done
  exit_code=0
  wait "$pid" || exit_code=$?
fi

# --- Codex keeps a richer session file: take the newest one since the start ----
metrics_source="$transcript"
if [ "$client" = codex ] && [ "$dry_run" != 1 ] && [ -d "$HOME/.codex/sessions" ]; then
  rollout=$(find "$HOME/.codex/sessions" -name 'rollout-*.jsonl' -newer "$marker" 2>/dev/null | sort | tail -n 1 || true)
  if [ -n "$rollout" ]; then
    cp "$rollout" "$run_dir/rollout.jsonl"
    metrics_source="$run_dir/rollout.jsonl"
  fi
fi
rm -f "$marker"

ended=$(date -u +%Y-%m-%dT%H:%M:%SZ)
python3 - "$run_dir/meta.json" "$client" "$mode" "$case_id" "$rep" "$started" "$ended" "$exit_code" "$capped" "$timed_out" "$max_turns" "$max_seconds" "$policy" "$dry_run" <<'PY'
import json, sys
(out, client, mode, case, rep, started, ended, code, capped, timed_out, max_turns, max_seconds, policy, dry) = sys.argv[1:]
json.dump({
    "client": client, "mode": mode, "case": case, "rep": int(rep),
    "started": started, "ended": ended, "exit_code": int(code),
    "turn_cap_hit": capped == "true", "timed_out": timed_out == "true",
    "max_turns": int(max_turns), "max_seconds": int(max_seconds),
    "policy": policy, "dry_run": dry == "1",
}, open(out, "w"), indent=2)
PY

if [ "$dry_run" != 1 ]; then
  report_args=()
  [ -d "$work/.forma-reports" ] && report_args=(--engine-report "$work/.forma-reports")
  python3 "$here/agent_metrics.py" "$metrics_source" --client "$client" ${report_args[@]+"${report_args[@]}"} >"$run_dir/metrics.json" || true
fi

echo "transcript: $transcript"
[ "$metrics_source" = "$transcript" ] || echo "rollout: $metrics_source"
echo "run folder: $run_dir"
