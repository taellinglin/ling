#!/usr/bin/env python3
"""Render AGC benchmark results into an SVG infographic.

Usage:
    python make_agc_infographic.py [agc_results.json] [agc_benchmark.svg]
"""

import json
import math
import sys

BG = "#14233D"
PANEL = "#1C2F4F"
TEXT = "#E6ECF5"
MUTED = "#8D99AE"
GRID = "#2C4067"

COLORS = {
    "Ling": "#E8B84A",
    "Rust": "#E84A6F",
    "C": "#2A9D8F",
    "C++": "#3B6EA5",
    "Go": "#7FB069",
    "Python": "#8D99AE",
}
LANG_ORDER = ["C", "C++", "Rust", "Go", "Python", "Ling"]

BENCH_ORDER = [
    "audio_fm_poly",
    "audio_iir_bank",
    "audio_delay_net",
    "gfx_mandelbrot",
    "gfx_particles",
    "gfx_triangle_math",
    "crypto_modexp",
    "crypto_feistel",
    "crypto_lcg_stream",
]

BENCH_DESC = {
    "audio_fm_poly": "AUDIO — polyphonic FM synthesis",
    "audio_iir_bank": "AUDIO — IIR filter-bank recurrence",
    "audio_delay_net": "AUDIO — delay-network recurrence",
    "gfx_mandelbrot": "GRAPHICS — complex float iterations",
    "gfx_particles": "GRAPHICS — particle integrator",
    "gfx_triangle_math": "GRAPHICS — edge-function triangle math",
    "crypto_modexp": "CRYPTO — modular exponentiation",
    "crypto_feistel": "CRYPTO — Feistel-round arithmetic",
    "crypto_lcg_stream": "CRYPTO — stream-state mixer",
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
    src = sys.argv[1] if len(sys.argv) > 1 else "agc_results.json"
    out = sys.argv[2] if len(sys.argv) > 2 else "agc_benchmark.svg"
    with open(src, "r", encoding="utf-8") as fh:
        data = json.load(fh)
    svg = build_svg(data)
    with open(out, "w", encoding="utf-8") as fh:
        fh.write(svg)
    print(f"wrote {out}")


def build_svg(data):
    meta = data.get("meta", {})
    benches = data.get("benchmarks", {})

    present = set()
    for b in benches.values():
        present.update(b.keys())
    langs = [l for l in LANG_ORDER if l in present]

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

    W = 1200
    pad = 40
    bar_x = 400
    bar_w = 620
    row_h = 26
    parts = []

    def rect(x, y, w, h, fill, rx=0, opacity=1.0):
        parts.append(
            f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h:.1f}" '
            f'rx="{rx}" fill="{fill}" opacity="{opacity}"/>'
        )

    def text(x, y, s, size=15, fill=TEXT, anchor="start", weight="normal", mono=False):
        fam = "Consolas, 'DejaVu Sans Mono', monospace" if mono else "'Segoe UI', 'DejaVu Sans', sans-serif"
        parts.append(
            f'<text x="{x:.1f}" y="{y:.1f}" font-family="{fam}" font-size="{size}" '
            f'fill="{fill}" text-anchor="{anchor}" font-weight="{weight}">{esc(s)}</text>'
        )

    all_ratios = [r for v in geo.values() for r in v] + [1.0]
    max_log = max(math.log10(max(r, 1.0)) for r in all_ratios) or 1.0

    def logbar_w(ratio):
        return 4 + (bar_w - 4) * (math.log10(max(ratio, 1.0)) / max_log)

    y = pad
    text(pad, y + 24, "Ling 2030 — AGC Benchmark", size=30, weight="bold")
    text(pad, y + 50, "Audio · Graphics · Cryptography compute suite", size=15, fill=MUTED)
    y += 78

    rect(pad, y, W - 2 * pad, 70, PANEL, rx=10)
    text(pad + 20, y + 28, "Cross-language benchmark with identical AGC algorithms", size=17, weight="bold", fill="#E8B84A")
    text(pad + 20, y + 52, "Lower bars are faster (log-scaled). Ratios are vs fastest per test.", size=14, fill=TEXT)
    y += 92

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

    text(pad, y, "Per-benchmark", size=18, weight="bold")
    y += 10

    for name in BENCH_ORDER:
        b = benches.get(name)
        if not b:
            continue
        tmin = min(v["time"] for v in b.values() if v["time"] > 0)
        y += 30

        group_fill = TEXT
        if name.startswith("audio_"):
            group_fill = "#E8B84A"
        elif name.startswith("gfx_"):
            group_fill = "#2A9D8F"
        elif name.startswith("crypto_"):
            group_fill = "#E84A6F"

        text(pad, y, name, size=16, weight="bold", fill=group_fill)
        text(pad + 210, y, BENCH_DESC.get(name, ""), size=13, fill=MUTED)

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
            text(bar_x + wpix + 10, y - 6, f"{fmt_time(t)}   ·   {fmt_ratio(ratio)}", size=13, fill=TEXT, mono=True)
        y += 6

    y += 30
    rect(pad, y - 18, W - 2 * pad, 1, GRID)
    text(pad, y + 6, f"machine: {meta.get('cpu', 'unknown')}  ·  {meta.get('os', '')}", size=12, fill=MUTED)
    text(
        pad,
        y + 24,
        f"min of {meta.get('reps', '?')} runs · identical algorithms/inputs across languages · "
        f"C/C++ -O2 -ffp-contract=off · Rust -O · Go build · CPython {meta.get('python', '')}",
        size=12,
        fill=MUTED,
    )
    text(pad, y + 42, f"generated {meta.get('date', '')} · benchmarks/agc_bench.*", size=12, fill=MUTED)
    H = y + 60

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
