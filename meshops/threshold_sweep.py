#!/usr/bin/env python3
"""Round 7c: the bulk threshold's effect. Links applets D and E, and the speed
bench's CRC16/parse kernels, with `chunker link-pic` at several thresholds and
writes meshops/out/threshold.md (included in RESULTS.md by report.py).

For each link: the gp window and bulk sizes, how many statics went to bulk
(and how many were spilled), the rewritten accesses, and a hash of the code
bytes. The kernels have no writable statics, so every threshold must give the
same bytes (the bench measures exactly that code).
"""
import json, os, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from build_applets import APPLETS, CHUNKER, fnv1a64  # noqa: E402

THRESHOLDS = [0, 64, 128, 256, 1024, 1 << 20]


def inputs(pkg):
    env = dict(os.environ, PKG=pkg, BUILD_STD="1", CODE_MODEL="medium")
    return subprocess.run([os.path.join(ROOT, "guest", "build.sh")], env=env,
                          capture_output=True, text=True, check=True).stdout.split()


def link(ins, entries, threshold, tmp):
    out = os.path.join(tmp, "x")
    args = [CHUNKER, "link-pic", "--bulk-threshold", str(threshold), "-o", out]
    for e in entries:
        args += ["--entry", e]
    r = subprocess.run(args + ins, capture_output=True, text=True)
    if r.returncode:
        return None, r.stderr.strip()
    j = json.load(open(out + ".json"))
    j["code_fnv"] = fnv1a64(open(out + ".bin", "rb").read())
    return j, ""


def main():
    os.makedirs(os.path.join(HERE, "out"), exist_ok=True)
    apps = {a[0]: a for a in APPLETS}
    cases = []
    ins = inputs("applets")
    for name in ("d", "e"):
        _, funcs, _, _ = apps[name]
        cases.append((f"applet {name.upper()}", ins, [name.upper() + "_IMPORTS"] + funcs))
    cases.append(("kernels (crc16, parse)", inputs("kernels"), ["crc16", "parse"]))
    L = ["| Link | Threshold | gp window (B) | Bulk (B) | Slots | Statics in bulk | Spilled | gp / bulk accesses | Code bytes | Code FNV |",
         "|---|---|---|---|---|---|---|---|---|---|"]
    same_kernels, sizes = set(), {}
    with tempfile.TemporaryDirectory() as tmp:
        for label, i, entries in cases:
            for t in THRESHOLDS:
                j, err = link(i, entries, t, tmp)
                tl = "none" if t >= 1 << 20 else str(t)
                if j is None:
                    L.append(f"| {label} | {tl} | refused: {err} | | | | | | | |")
                    continue
                placed = j["placed"]
                nb = sum(1 for p in placed if p[2] == "bulk")
                sp = sum(1 for p in placed if p[4] == "spill")
                L.append(f"| {label} | {tl} | {j['gp_size']} | {j['bulk_size']} | {j['bulk_slots']} | {nb} of {len(placed)} | {sp} "
                         f"| {j['gp_rewrites']} / {j['bulk_rewrites']} | {j['flash_len']} | `{j['code_fnv']:016x}` |")
                sizes.setdefault(label, set()).add(j["flash_len"])
                if label.startswith("kernels"):
                    same_kernels.add(j["code_fnv"])
    L += ["", f"Kernels: {len(same_kernels)} distinct code hash(es) across {len(THRESHOLDS)} thresholds "
          "(they have no writable statics, so placement can't touch them).",
          "Code size per link across thresholds: " + ", ".join(
              f"{k}: {'/'.join(str(v) for v in sorted(vs))} B" for k, vs in sizes.items())
          + " (one size each: placement rewrites instructions in place, never adds them)."]
    open(os.path.join(HERE, "out", "threshold.md"), "w").write("\n".join(L) + "\n")
    print("\n".join(L))
    return 0 if len(same_kernels) == 1 and all(len(v) == 1 for v in sizes.values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
