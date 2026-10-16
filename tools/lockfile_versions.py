#!/usr/bin/env python3
"""Fail when the Python lockfile resolves a shared crate the workspace does not.

`crates/daedalus-py` is excluded from the cargo workspace (maturin builds it from
its own manifest) and carries crates/daedalus-py/Cargo.lock. Every crate version
in that lockfile that also appears in the workspace lockfile must be a version
the workspace already resolves, and a shared git dependency must use the same
source. The workspace may carry extra versions (it builds more crates).
"""

import sys
from pathlib import Path


def packages(path):
    """name -> set of (version, source) from a Cargo.lock."""
    found = {}
    name = version = source = None
    def flush():
        nonlocal name, version, source
        if name and version:
            found.setdefault(name, set()).add((version, source or ""))
        name = version = source = None
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if line.startswith("[[package]]"):
            flush()
        elif line.startswith("name = "):
            name = line.split("=", 1)[1].strip().strip('"')
        elif line.startswith("version = ") and name and version is None:
            version = line.split("=", 1)[1].strip().strip('"')
        elif line.startswith("source = ") and name:
            source = line.split("=", 1)[1].strip().strip('"')
    flush()
    return found


def main():
    root = Path(__file__).resolve().parents[1]
    ws = packages(root / "Cargo.lock")
    py = packages(root / "crates" / "daedalus-py" / "Cargo.lock")
    bad = []
    shared = 0
    for name in sorted(set(ws) & set(py)):
        shared += 1
        ws_versions = {v for v, _ in ws[name]}
        py_versions = {v for v, _ in py[name]}
        if not py_versions <= ws_versions:
            bad.append(f"  {name}: python {sorted(py_versions)} is not in the workspace {sorted(ws_versions)}")
        ws_git = {s for _, s in ws[name] if s.startswith("git+")}
        py_git = {s for _, s in py[name] if s.startswith("git+")}
        if py_git and ws_git and py_git != ws_git:
            bad.append(f"  {name}: git source differs\n    workspace {sorted(ws_git)}\n    python {sorted(py_git)}")
    if bad:
        print("crates/daedalus-py/Cargo.lock does not match the workspace lockfile:")
        print("\n".join(bad))
        sys.exit(1)
    print(f"{shared} shared crates match")


if __name__ == "__main__":
    main()
