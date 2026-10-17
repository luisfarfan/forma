"""Line-coverage gate for CI: every named package must have at least MIN % of
its source lines covered by the whole workspace test run.

    cargo llvm-cov --workspace --locked --json --summary-only --output-path S.json
    python -I scripts/coverage_gate.py S.json --min 80 forma-model forma-edit ...

S.json is `llvm-cov export` (what `cargo llvm-cov --json --summary-only`
writes): a list of files with their line counts. The lines of a package are
those of the files under `crates/<package>/src/`; its percentage is the covered
lines over all lines of those files, so a big file weighs more than a small one,
as in `--fail-under-lines`. `cargo llvm-cov --package` is not used for this: its
report also includes the workspace crates the package depends on, which would
mix their lines into the number of the package that is being checked.

Prints one line per package. Exits 0 if all pass, 1 if one is below MIN or has
no measured lines (a package missing from the report must not pass silently),
2 on a usage error or an unreadable report.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

CRATE_SOURCE = re.compile(r"/crates/([^/]+)/src/")


def package_lines(report: dict) -> dict[str, list[int]]:
    """{package: [covered lines, lines]} summed over the files of the report."""
    totals: dict[str, list[int]] = {}
    for export in report["data"]:
        for entry in export["files"]:
            found = CRATE_SOURCE.search(entry["filename"].replace("\\", "/"))
            if found is None:
                continue
            lines = entry["summary"]["lines"]
            total = totals.setdefault(found.group(1), [0, 0])
            total[0] += lines["covered"]
            total[1] += lines["count"]
    return totals


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("summary", type=Path, help="the JSON written by cargo llvm-cov")
    parser.add_argument("--min", type=float, required=True, help="minimum line coverage, in percent")
    parser.add_argument("packages", nargs="+", help="package names, such as forma-model")
    args = parser.parse_args(argv)

    try:
        totals = package_lines(json.loads(args.summary.read_text(encoding="utf-8")))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"coverage_gate: cannot read {args.summary}: {error!r}", file=sys.stderr)
        return 2

    failed = False
    for package in args.packages:
        covered, count = totals.get(package, [0, 0])
        if count == 0:
            print(f"{package:<16} NO LINES MEASURED (is it in the report?)")
            failed = True
            continue
        percent = 100.0 * covered / count
        ok = percent >= args.min
        failed = failed or not ok
        print(
            f"{package:<16} {percent:6.2f} % of lines ({covered}/{count})  "
            f"{'ok' if ok else f'BELOW {args.min:g} %'}"
        )
    print("coverage gate:", "FAILED" if failed else "ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
