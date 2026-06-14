#!/usr/bin/env python3
"""Compile, run, and time the Ling cross-language benchmark suite.

Runs bench.{ling,py,c,cpp,rs,go}, takes the best (min) of N runs per benchmark,
verifies every language computes the same checksum, writes results.json, and
renders an SVG infographic.

    python run_benchmarks.py [reps]      # default reps = 3
"""
import json
import os
import platform
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
BIN = os.path.join(HERE, "bin")
EXE = ".exe" if os.name == "nt" else ""

BENCH_ORDER = ["fib", "loop_sum", "leibniz", "primes", "mandelbrot", "fm_synth"]


def find(*names, fallbacks=()):
    for n in names:
        p = shutil.which(n)
        if p:
            return p
    for p in fallbacks:
        if os.path.exists(p):
            return p
    return None


def ling_bin():
    for p in (
        os.path.join(REPO, "target", "release", "ling" + EXE),
        os.path.join(REPO, "target", "debug", "ling" + EXE),
    ):
        if os.path.exists(p):
            return p
    return None


def lang_table():
    py = sys.executable or find("python", "py")
    go = find("go", fallbacks=[r"C:\Program Files\Go\bin\go.exe", "/usr/local/go/bin/go"])
    gcc = find("gcc", "cc")
    gpp = find("g++", "clang++")
    rustc = find("rustc")
    ling = ling_bin()

    t = {}
    if ling:
        t["Ling"] = {"run": [ling, "run", os.path.join(HERE, "bench.ling")]}
    if py:
        t["Python"] = {"run": [py, os.path.join(HERE, "bench.py")]}
    if rustc:
        outp = os.path.join(BIN, "bench_rs" + EXE)
        t["Rust"] = {
            "compile": [rustc, "-O", os.path.join(HERE, "bench.rs"), "-o", outp],
            "run": [outp],
        }
    if gcc:
        outp = os.path.join(BIN, "bench_c" + EXE)
        t["C"] = {
            "compile": [gcc, "-O2", "-ffp-contract=off", os.path.join(HERE, "bench.c"),
                        "-o", outp, "-lm"],
            "run": [outp],
        }
    if gpp:
        outp = os.path.join(BIN, "bench_cpp" + EXE)
        t["C++"] = {
            "compile": [gpp, "-O2", "-ffp-contract=off", os.path.join(HERE, "bench.cpp"),
                        "-o", outp],
            "run": [outp],
        }
    if go:
        outp = os.path.join(BIN, "bench_go" + EXE)
        t["Go"] = {
            "compile": [go, "build", "-o", outp, os.path.join(HERE, "bench.go")],
            "run": [outp],
        }
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
    os.makedirs(BIN, exist_ok=True)
    langs = lang_table()
    print(f"languages found: {', '.join(langs) or 'NONE'}\n")

    # ── compile ───────────────────────────────────────────────────────────
    for name, cfg in list(langs.items()):
        if "compile" in cfg:
            print(f"compiling {name} ...", end=" ", flush=True)
            r = subprocess.run(cfg["compile"], cwd=HERE, capture_output=True, text=True)
            if r.returncode != 0:
                print("FAILED")
                print(r.stderr.strip()[:600])
                langs.pop(name)
            else:
                print("ok")
    print()

    # ── run ───────────────────────────────────────────────────────────────
    best = {b: {} for b in BENCH_ORDER}     # bench -> lang -> {time, result}
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

    # ── verify checksums against Ling (or first available) ──────────────────
    print("checksum verification (tolerance 1e-3 relative):")
    for b in BENCH_ORDER:
        row = best.get(b, {})
        if not row:
            continue
        ref_lang = "Ling" if "Ling" in row else next(iter(row))
        ref = row[ref_lang]["result"]
        bad = []
        for l, v in row.items():
            denom = abs(ref) if abs(ref) > 1e-9 else 1.0
            if abs(v["result"] - ref) / denom > 1e-3:
                bad.append(f"{l}={v['result']:g}")
        tag = "OK" if not bad else "MISMATCH: " + ", ".join(bad)
        print(f"  {b:<12} ref({ref_lang})={ref:<22g} {tag}")
    print()

    # ── results table ───────────────────────────────────────────────────────
    langs_present = [l for l in ["C", "C++", "Rust", "Go", "Python", "Ling"]
                     if any(l in best[b] for b in BENCH_ORDER)]
    hdr = f"{'benchmark':<12}" + "".join(f"{l:>12}" for l in langs_present)
    print(hdr)
    print("-" * len(hdr))
    for b in BENCH_ORDER:
        line = f"{b:<12}"
        for l in langs_present:
            v = best[b].get(l)
            line += f"{(v['time']*1000):>10.2f}m" if v else f"{'-':>12}"
        print(line)
    print("\n(times in milliseconds, best of N runs)\n")

    # ── write json + svg ────────────────────────────────────────────────────
    meta = {
        "date": time.strftime("%Y-%m-%d %H:%M"),
        "cpu": platform.processor() or platform.machine(),
        "os": platform.platform(),
        "python": platform.python_version(),
        "reps": reps,
    }
    data = {"meta": meta, "langs": langs_present, "benchmarks": best}
    with open(os.path.join(HERE, "results.json"), "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2)
    print("wrote results.json")

    try:
        sys.path.insert(0, HERE)
        import make_infographic
        svg = make_infographic.build_svg(data)
        out = os.path.join(HERE, "ling_benchmark.svg")
        with open(out, "w", encoding="utf-8") as fh:
            fh.write(svg)
        print(f"wrote {out}")
    except Exception as e:  # noqa: BLE001
        print(f"infographic step failed: {e}")


if __name__ == "__main__":
    main()
