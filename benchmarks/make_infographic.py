#!/usr/bin/env python3
"""Render benchmark results.json into an SVG infographic (Ling palette).

Usage:
    python make_infographic.py [results.json] [out.svg]
"""
import json
import math
import sys

# ── Ling palette ───────────────────────────────────────────────────────────
BG = "#14233D"
PANEL = "#1C2F4F"
TEXT = "#E6ECF5"
MUTED = "#8D99AE"
GRID = "#2C4067"

COLORS = {
    "Ling": "#E8B84A",   # gold — the star of the show
    "Rust": "#E84A6F",   # rose
    "C": "#2A9D8F",      # teal
    "C++": "#3B6EA5",    # navy blue
    "Go": "#7FB069",     # vine green
    "Python": "#8D99AE", # grey
}
LANG_ORDER = ["C", "C++", "Rust", "Go", "Python", "Ling"]

BENCH_ORDER = ["fib", "loop_sum", "leibniz", "primes", "mandelbrot", "fm_synth"]
BENCH_DESC = {
    "fib": "recursion · function-call overhead",
    "loop_sum": "tight integer arithmetic loop",
    "leibniz": "floating-point division loop",
    "primes": "branchy integer (trial division)",
    "mandelbrot": "GRAPHICS — complex float math",
    "fm_synth": "AUDIO — FM synthesis (sin)",
}


def esc(s):
    return str(s).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def fmt_time(s):
    if s >= 1.0:
        return f"{s:.2f} s"
    if s >= 1e-3:
        return f"{s * 1e3:.1f} ms"
    return f"{s * 1e6:.0f} µs"


def fmt_ratio(r):
    if r < 1.05:
        return "1× (fastest)"
    if r < 10:
        return f"{r:.1f}×"
    if r < 1000:
        return f"{r:.0f}×"
    return f"{r / 1000:.1f}k×"


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else "results.json"
    out = sys.argv[2] if len(sys.argv) > 2 else "ling_benchmark.svg"
    with open(src, "r", encoding="utf-8") as fh:
        data = json.load(fh)
    svg = build_svg(data)
    with open(out, "w", encoding="utf-8") as fh:
        fh.write(svg)
    print(f"wrote {out}")


def build_svg(data):
    meta = data.get("meta", {})
    benches = data.get("benchmarks", {})
    langs = [l for l in LANG_ORDER if l in data.get("langs", LANG_ORDER)]
    # langs actually present in the data
    present = set()
    for b in benches.values():
        present.update(b.keys())
    langs = [l for l in LANG_ORDER if l in present]

    # ── geometric-mean slowdown vs the fastest, per language ────────────────
    geo = {l: [] for l in langs}
    for name in BENCH_ORDER:
        b = benches.get(name)
        if not b:
            continue
        tmin = min(v["time"] for v in b.values() if v["time"] > 0)
        for l in langs:
            if l in b and b[l]["time"] > 0:
                geo[l].append(b[l]["time"] / tmin)
    geomean = {l: math.exp(sum(math.log(x) for x in v) / len(v)) for l, v in geo.items() if v}
    ranked = sorted(geomean, key=lambda l: geomean[l])

    # headline ratios
    def gm_ratio(a, b):
        rs = []
        for name in BENCH_ORDER:
            bb = benches.get(name)
            if bb and a in bb and b in bb and bb[b]["time"] > 0:
                rs.append(bb[a]["time"] / bb[b]["time"])
        return math.exp(sum(math.log(x) for x in rs) / len(rs)) if rs else None

    ling_vs_c = gm_ratio("Ling", "C")
    ling_vs_py = gm_ratio("Ling", "Python")

    # ── layout ──────────────────────────────────────────────────────────────
    W = 1200
    pad = 40
    bar_x = 360
    bar_w = 660
    row_h = 26
    parts = []

    def rect(x, y, w, h, fill, rx=0, opacity=1.0):
        parts.append(
            f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h:.1f}" '
            f'rx="{rx}" fill="{fill}" opacity="{opacity}"/>'
        )

    def text(x, y, s, size=15, fill=TEXT, anchor="start", weight="normal", mono=False):
        fam = "Consolas, 'DejaVu Sans Mono', monospace" if mono else \
            "'Segoe UI', 'DejaVu Sans', sans-serif"
        parts.append(
            f'<text x="{x:.1f}" y="{y:.1f}" font-family="{fam}" font-size="{size}" '
            f'fill="{fill}" text-anchor="{anchor}" font-weight="{weight}">{esc(s)}</text>'
        )

    # global max log-ratio (for consistent log bar scaling across the figure)
    all_ratios = [r for v in geo.values() for r in v] + [1.0]
    max_log = max(math.log10(max(r, 1.0)) for r in all_ratios) or 1.0

    def logbar_w(ratio):
        # fastest -> small stub; slowest -> full width
        return 4 + (bar_w - 4) * (math.log10(max(ratio, 1.0)) / max_log)

    y = pad

    # ── header ───────────────────────────────────────────────────────────────
    text(pad, y + 24, "Ling 2030 — Language Benchmark", size=30, weight="bold")
    text(pad, y + 50, "base language · graphics · audio compute, vs Python · Rust · C · C++ · Go",
         size=15, fill=MUTED)
    y += 78

    # ── headline callout ──────────────────────────────────────────────────────
    rect(pad, y, W - 2 * pad, 70, PANEL, rx=10)
    hl = "Ling's tree-walking interpreter trades raw speed for reach."
    text(pad + 20, y + 28, hl, size=17, weight="bold", fill="#E8B84A")
    sub = []
    if ling_vs_c:
        sub.append(f"~{fmt_ratio(ling_vs_c)} slower than C")
    if ling_vs_py:
        sub.append(f"~{fmt_ratio(ling_vs_py)} slower than CPython")
    sub.append("(geometric mean) — its graphics/audio/GPU run as native Rust crates at ~C speed.")
    text(pad + 20, y + 52, "   ·   ".join(sub), size=14, fill=TEXT)
    y += 92

    # ── overall ranking ────────────────────────────────────────────────────────
    text(pad, y, "Overall — geometric-mean slowdown vs fastest", size=18, weight="bold")
    y += 14
    gmax_log = max(math.log10(max(geomean[l], 1.0)) for l in ranked) or 1.0
    for l in ranked:
        y += row_h
        text(pad, y - 6, l, size=15, weight="bold", fill=COLORS.get(l, TEXT))
        rect(bar_x, y - row_h + 6, bar_w, row_h - 10, GRID, rx=4, opacity=0.35)
        wpix = 4 + (bar_w - 4) * (math.log10(max(geomean[l], 1.0)) / gmax_log)
        rect(bar_x, y - row_h + 6, wpix, row_h - 10, COLORS.get(l, TEXT), rx=4)
        text(bar_x + wpix + 10, y - 6, fmt_ratio(geomean[l]), size=14, fill=TEXT)
    y += 24

    # ── per-benchmark sections ──────────────────────────────────────────────────
    text(pad, y, "Per-benchmark (log-scaled · shorter bar = faster)", size=18, weight="bold")
    y += 10
    for name in BENCH_ORDER:
        b = benches.get(name)
        if not b:
            continue
        tmin = min(v["time"] for v in b.values() if v["time"] > 0)
        y += 30
        is_subsys = name in ("mandelbrot", "fm_synth")
        title_fill = "#E8B84A" if is_subsys else TEXT
        text(pad, y, f"{name}", size=16, weight="bold", fill=title_fill)
        text(pad + 130, y, BENCH_DESC.get(name, ""), size=13, fill=MUTED)
        for l in langs:
            if l not in b:
                continue
            y += row_h
            t = b[l]["time"]
            ratio = t / tmin if tmin > 0 else 1.0
            text(pad + 12, y - 6, l, size=14, fill=COLORS.get(l, TEXT))
            rect(bar_x, y - row_h + 6, bar_w, row_h - 10, GRID, rx=4, opacity=0.3)
            wpix = logbar_w(ratio)
            rect(bar_x, y - row_h + 6, wpix, row_h - 10, COLORS.get(l, TEXT), rx=4)
            label = f"{fmt_time(t)}   ·   {fmt_ratio(ratio)}"
            text(bar_x + wpix + 10, y - 6, label, size=13, fill=TEXT, mono=True)
        y += 6

    # ── footer ─────────────────────────────────────────────────────────────────
    y += 30
    rect(pad, y - 18, W - 2 * pad, 1, GRID)
    cpu = meta.get("cpu", "unknown CPU")
    osv = meta.get("os", "")
    date = meta.get("date", "")
    reps = meta.get("reps", "?")
    text(pad, y + 6, f"machine: {cpu}  ·  {osv}", size=12, fill=MUTED)
    text(pad, y + 24, f"min of {reps} runs · same algorithm/inputs in every language · "
                      f"C/C++ built -O2 -ffp-contract=off · Rust -O · Go build · CPython {meta.get('python','')}",
         size=12, fill=MUTED)
    text(pad, y + 42, f"generated {date}  ·  benchmarks/ in the ling repo  ·  "
                      "mandelbrot=graphics-style float math, fm_synth=audio-style synthesis "
                      "(these are CPU compute, not the native ling-graphics/ling-audio crates)",
         size=12, fill=MUTED)
    H = y + 60

    # ── legend (top-right) ──────────────────────────────────────────────────────
    lx = W - pad - 150
    ly = pad + 4
    rect(lx - 12, ly - 4, 162, 18 * len(langs) + 16, PANEL, rx=8, opacity=0.9)
    text(lx, ly + 12, "legend", size=12, fill=MUTED)
    for i, l in enumerate(langs):
        yy = ly + 30 + i * 18
        rect(lx, yy - 10, 12, 12, COLORS.get(l, TEXT), rx=2)
        text(lx + 20, yy, l, size=13, fill=TEXT)

    body = "\n".join(parts)
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H:.0f}" '
        f'viewBox="0 0 {W} {H:.0f}">\n'
        f'<rect width="{W}" height="{H:.0f}" fill="{BG}"/>\n{body}\n</svg>\n'
    )


if __name__ == "__main__":
    main()
