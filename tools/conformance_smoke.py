#!/usr/bin/env python3
"""Write the pull-request conformance smoke list.

The list is every model that does not pass in docs/conformance/msl-4.1.0.json,
plus any baseline model whose full name appears in the diff of HEAD against
the base tip. One name per line.

    python3 tools/conformance_smoke.py origin/main smoke-models.txt

The diff is two commits (`git diff BASE HEAD`), not three-dot. A pull-request
checkout is one commit deep and the base is fetched with --depth=1, so those
histories do not meet and `git diff BASE...HEAD` exits 128 (no merge base).
Comparing the two trees does not need one.
"""

import json
import re
import subprocess
import sys


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else "origin/main"
    out = sys.argv[2] if len(sys.argv) > 2 else "smoke-models.txt"
    data = json.load(open("docs/conformance/msl-4.1.0.json", encoding="utf-8"))
    known = [r["model"] for r in data["results"]]
    failing = [r["model"] for r in data["results"] if r.get("status") != "pass"]
    diff = subprocess.run(
        ["git", "diff", "--unified=0", base, "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    mentioned = [m for m in known if re.search(re.escape(m) + r"(?![A-Za-z0-9_])", diff)]
    models = sorted(set(failing) | set(mentioned))
    with open(out, "w", encoding="utf-8") as f:
        f.write("\n".join(models) + "\n")
    print(
        f"{len(models)} smoke models ({len(failing)} not passing, {len(set(mentioned) - set(failing))} mentioned in the diff)",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
