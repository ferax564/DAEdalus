# Draws the per-package MSL chart for docs/img/msl/by_package.webp from docs/conformance/msl-4.1.0.json.
# Run from the repository root; it writes msl_by_package.png, which is then saved as WebP (Pillow).
import json, collections, matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib import font_manager
# Run from the repository root: python3 tools/msl_by_package.py
rows = json.load(open("docs/conformance/msl-4.1.0.json"))["results"]
pkg = collections.defaultdict(lambda: collections.Counter())
for r in rows:
    parts = r["model"].split(".")
    key = ".".join(parts[:2]) if len(parts) > 2 else parts[0]
    pkg[key][r["status"]] += 1
items = sorted(pkg.items(), key=lambda kv: -sum(kv[1].values()))[:16]
names = [k.replace("Modelica.", "") for k, _ in items]
tot = [sum(c.values()) for _, c in items]
passed = [c["pass"] for _, c in items]
differ = [c["differ"] for _, c in items]
fail = [c["error"] + c["timeout"] for _, c in items]
fig, ax = plt.subplots(figsize=(9, 6.2), dpi=150)
y = range(len(names))[::-1]
ax.barh(y, passed, color="#2b8a3e", label="match the reference")
ax.barh(y, differ, left=passed, color="#f59f00", label="run, differ")
ax.barh(y, fail, left=[p + d for p, d in zip(passed, differ)], color="#e03131", label="error or timeout")
for yi, t, p in zip(y, tot, passed):
    ax.text(t + 0.8, yi, f"{p}/{t}", va="center", fontsize=8.5, color="#444")
ax.set_yticks(list(y)); ax.set_yticklabels(names, fontsize=9)
ax.set_xlabel("MSL 4.1.0 examples")
ax.set_title("Where the MSL examples stand, by package (top-level packages)", fontsize=11, loc="left")
ax.spines[["top", "right"]].set_visible(False)
ax.legend(loc="lower right", fontsize=8.5, frameon=False)
ax.set_xlim(0, max(tot) + 8)
fig.tight_layout()
fig.savefig("msl_by_package.png", dpi=150)
print(names, passed, sum(passed))
