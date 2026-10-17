#!/usr/bin/env bash
# Milestone 1 acceptance with external oracles (docs/04-roadmap.md, Hito 1,
# criteria 2 and 8). An AI agent builds "Forma Demo" through `forma mcp` (the
# stdio scenario test), then independent tools check the font it exported:
# OTS, HarfBuzz, fontTools, fontmake parity and, if it is installed,
# fontspector.
#
# Usage (from any folder):  scripts/acceptance.sh
#
# Python: $FORMA_ORACLE_PYTHON if set, else tools/oracle/.venv, else python3
# on the PATH. It needs the packages of tools/oracle/requirements.txt:
#   python3 -m venv tools/oracle/.venv
#   tools/oracle/.venv/bin/pip install -r tools/oracle/requirements.txt
# fontspector is optional: cargo install fontspector --locked
#
# Artefacts go to out/acceptance/, which is recreated on every run.
set -euo pipefail
# Whatever stops the script (a failed step or die), the log ends with a verdict.
trap 'status=$?; ((status == 0)) || printf "\nACCEPTANCE FAILED (exit code %s)\n" "$status" >&2' EXIT

repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
core="$repo/core"
out="$repo/out/acceptance"

step() { printf '\n==> %s\n' "$*"; }
die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

if [[ -n "${FORMA_ORACLE_PYTHON:-}" ]]; then
  python="$FORMA_ORACLE_PYTHON"
elif [[ -x "$repo/tools/oracle/.venv/bin/python" ]]; then
  python="$repo/tools/oracle/.venv/bin/python"
elif command -v python3 >/dev/null 2>&1; then
  python="$(command -v python3)"
else
  die "no Python found: set FORMA_ORACLE_PYTHON or create tools/oracle/.venv"
fi

# The same head timestamps from both compilers (fontc and fontmake honour it).
export SOURCE_DATE_EPOCH=0

step "1/5 Build the release forma binary"
(cd "$core" && cargo build --release -p forma-cli --locked)
target_dir="$(cd "$core" && cargo metadata --format-version 1 --no-deps |
  "$python" -I -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
forma="$target_dir/release/forma"
[[ -x "$forma" ]] || die "the release binary is not at $forma"

step "2/5 Agent scenario over MCP stdio (artefacts in $out)"
rm -rf -- "$out"
mkdir -p -- "$out"
(cd "$core" && FORMA_ACCEPTANCE_OUT="$out" cargo test -p forma-cli --locked --test mcp_scenario)
for file in forma-demo.ttf forma-demo.woff2 forma-demo.forma expected.json; do
  [[ -s "$out/$file" ]] || die "the scenario did not write $out/$file"
done

step "3/5 Export the scenario document as a UFO"
"$forma" export "$out/forma-demo.forma" --format ufo -o "$out/forma-demo.ufo"

step "4/5 External oracles: OTS, HarfBuzz, fontTools, fontmake parity"
# Run from the artefact folder, never from inside the Python environment.
(cd "$out" && "$python" -I "$repo/tools/oracle/verify_font.py" \
  --ttf "$out/forma-demo.ttf" --expected "$out/expected.json" \
  --woff2 "$out/forma-demo.woff2" \
  --ufo "$out/forma-demo.ufo" --fontmake-parity --work-dir "$out/fontmake") ||
  die "the external oracles rejected the font (see above)"

step "5/5 fontspector (opentype profile)"
if command -v fontspector >/dev/null 2>&1; then
  version="$(fontspector --version)"
  echo "${version%%$'\n'*}"
  # Exit code 1 if any check ends in FAIL or worse (--error-code-on fail).
  (cd "$out" && fontspector --profile opentype --error-code-on fail \
    --json "$out/fontspector.json" "$out/forma-demo.ttf") ||
    die "fontspector reported a FAIL (report: $out/fontspector.json)"
else
  echo "fontspector: SKIPPED (not on the PATH; install it with: cargo install fontspector --locked)"
fi

printf '\nACCEPTANCE OK\n'
