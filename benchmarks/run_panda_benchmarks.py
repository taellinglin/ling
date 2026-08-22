#!/usr/bin/env python3
"""Run and time the Ling vs Python+Panda3D engine-feature benchmark suite.

Runs panda_bench.ling and panda_bench.py, takes the best (min) of N runs per
benchmark, verifies checksums where the two sides compute the same algorithm,
writes panda_results.json, and renders an SVG infographic.

    python run_panda_benchmarks.py [reps]      # default reps = 3
"""
import json
import os
import platform
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
EXE = ".exe" if os.name == "nt" else ""

BENCH_ORDER = [
    "vec_math",
    "noise_field",
    "vertex_pipeline",
    "fm_synth",
    "hash_chain",
    "boids",
    "spring_rope",
]

# noise_field: Panda3D Perlin tables vs Ling's fbm implementation.
# hash_chain:  Panda3D HashVal is MD5, Ling hashes with SHA-256.
# Same workload shape, different algorithm -> per-language checksums only.
UNVERIFIED = {"noise_field", "hash_chain"}


def ling_bin():
    for p in (
        os.path.join(REPO, "target", "release", "ling" + EXE),
        os.path.join(REPO, "target", "debug", "ling" + EXE),
    ):
        if os.path.exists(p):
            return p
    return None


def lang_table():
    t = {}
    ling = ling_bin()
    if ling:
        t["Ling"] = {"run": [ling, "run", os.path.join(HERE, "panda_bench.ling")]}
    py = sys.executable or "python"
    probe = subprocess.run([py, "-c", "import panda3d.core"], capture_output=True)
    if probe.returncode == 0:
        t["Panda3D"] = {"run": [py, os.path.join(HERE, "panda_bench.py")]}
    else:
        print("panda3d not importable from this Python — skipping the Panda3D side")
    return t


def parse(output):
    """Parse 'BENCH <name> RESULT <r> TIME <t>' lines."""
    res = {}
    for line in output.splitlines():
        p = line.split()
        if len(p) == 6 and p[0] == "BENCH" and p[2] == "RESULT" and p[4] == "TIME":
            try:
                res[p[1]] = {"result": float(p[3]), "time": float(p[5])}
            except ValueError:
                pass
    return res


def main():
    reps = int(sys.argv[1]) if len(sys.argv) > 1 else 3
    langs = lang_table()
    print(f"languages found: {', '.join(langs) or 'NONE'}\n")

    best = {b: {} for b in BENCH_ORDER}
    for name, cfg in langs.items():
        print(f"running {name} ({reps}x) ...", end=" ", flush=True)
        agg = {}
        ok = True
        for _ in range(reps):
            try:
                r = subprocess.run(cfg["run"], cwd=HERE, capture_output=True, text=True,
                                   timeout=600)
            except subprocess.TimeoutExpired:
                ok = False
                break
            parsed = parse(r.stdout)
            if not parsed:
                ok = False
                print("no output\n" + r.stdout[:300] + r.stderr[:300])
                break
            for b, v in parsed.items():
                if b not in agg or v["time"] < agg[b]["time"]:
                    agg[b] = v
        if ok:
            for b, v in agg.items():
                if b in best:
                    best[b][name] = v
            print("ok")
        else:
            print("skipped")
    print()

    # ── verify checksums (tolerance 1e-3 relative) ─────────────────────────
    print("checksum verification (tolerance 1e-3 relative):")
    for b in BENCH_ORDER:
        row = best.get(b, {})
        if not row:
            continue
        if b in UNVERIFIED:
            vals = ", ".join(f"{l}={v['result']:g}" for l, v in row.items())
            print(f"  {b:<16} SKIPPED (different algorithms by design)  {vals}")
            continue
        ref_lang = "Ling" if "Ling" in row else next(iter(row))
        ref = row[ref_lang]["result"]
        bad = []
        for l, v in row.items():
            denom = abs(ref) if abs(ref) > 1e-9 else 1.0
            if abs(v["result"] - ref) / denom > 1e-3:
                bad.append(f"{l}={v['result']:g}")
        tag = "OK" if not bad else "MISMATCH: " + ", ".join(bad)
        print(f"  {b:<16} ref({ref_lang})={ref:<22g} {tag}")
    print()

    # ── results table ──────────────────────────────────────────────────────
    langs_present = [l for l in ["Panda3D", "Ling"]
                     if any(l in best[b] for b in BENCH_ORDER)]
    hdr = f"{'benchmark':<16}" + "".join(f"{l:>14}" for l in langs_present)
    print(hdr)
    print("-" * len(hdr))
    for b in BENCH_ORDER:
        line = f"{b:<16}"
        for l in langs_present:
            v = best[b].get(l)
            line += f"{(v['time'] * 1000):>12.2f}ms" if v else f"{'-':>14}"
        print(line)
    print("\n(times in milliseconds, best of N runs)\n")

    # ── write json + svg ───────────────────────────────────────────────────
    meta = {
        "date": time.strftime("%Y-%m-%d %H:%M"),
        "cpu": platform.processor() or platform.machine(),
        "os": platform.platform(),
        "python": platform.python_version(),
        "reps": reps,
    }
    try:
        from panda3d.core import PandaSystem
        meta["panda3d"] = PandaSystem.get_version_string()
    except Exception:  # noqa: BLE001
        pass
    data = {"meta": meta, "langs": langs_present, "benchmarks": best}
    with open(os.path.join(HERE, "panda_results.json"), "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2)
    print("wrote panda_results.json")

    try:
        sys.path.insert(0, HERE)
        import make_panda_infographic
        svg = make_panda_infographic.build_svg(data)
        out = os.path.join(HERE, "panda_benchmark.svg")
        with open(out, "w", encoding="utf-8") as fh:
            fh.write(svg)
        print(f"wrote {out}")
    except Exception as e:  # noqa: BLE001
        print(f"infographic step failed: {e}")


if __name__ == "__main__":
    main()
