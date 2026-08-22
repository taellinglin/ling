#!/usr/bin/env python3
"""Render panda_results.json into an SVG infographic (Ling palette).

Usage:
    python make_panda_infographic.py [panda_results.json] [out.svg]
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
    "Ling": "#E8B84A",     # gold — the star of the show
    "Panda3D": "#E84A6F",  # rose — Panda3D's brand red, from the Ling palette
}
LANG_ORDER = ["Panda3D", "Ling"]

BENCH_ORDER = [
    "vec_math",
    "noise_field",
    "vertex_pipeline",
    "fm_synth",
    "hash_chain",
    "boids",
    "spring_rope",
]
BENCH_CAT = {
    "vec_math": "MATH",
    "noise_field": "COMPUTING",
    "vertex_pipeline": "GRAPHICS",
    "fm_synth": "AUDIO",
    "hash_chain": "CRYPTO",
    "boids": "AI",
    "spring_rope": "PHYSICS",
}
BENCH_DESC = {
    "vec_math": "LVecBase3d dot · cross · length vs Ling scalar math",
    "noise_field": "StackedPerlinNoise2, 4 octaves vs Ling fbm builtin (different noise tables — timing only)",
    "vertex_pipeline": "LMatrix4d rotate·translate compose + xform + perspective project",
    "fm_synth": "2-op FM synthesis + envelope (Panda3D has no DSP API — pure Python loop)",
    "hash_chain": "HashVal MD5 chain vs Ling sha256_hex (different hash algorithms — timing only)",
    "boids": "flock steering: seek + all-pairs separation with LVecBase3d",
    "spring_rope": "pinned spring-mass rope, symplectic Euler, LVecBase3d",
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
    src = sys.argv[1] if len(sys.argv) > 1 else "panda_results.json"
    out = sys.argv[2] if len(sys.argv) > 2 else "panda_benchmark.svg"
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

    # ── per-benchmark ratios & the headline geomean ─────────────────────────
    ling_wins = []
    panda_wins = []
    logs = []
    for name in BENCH_ORDER:
        b = benches.get(name)
        if not b or "Ling" not in b or "Panda3D" not in b:
            continue
        r = b["Panda3D"]["time"] / b["Ling"]["time"]  # >1 → Ling faster
        logs.append(math.log(r))
        (ling_wins if r >= 1.0 else panda_wins).append((name, r))
    geomean = math.exp(sum(logs) / len(logs)) if logs else None

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

    def chip(x, y, label):
        cw = 12 + len(label) * 7.4
        rect(x, y - 13, cw, 18, GRID, rx=9)
        text(x + cw / 2, y, label, size=11, fill=TEXT, anchor="middle", weight="bold")
        return cw

    # global max log-ratio for consistent log bar scaling
    all_ratios = [1.0]
    for name in BENCH_ORDER:
        b = benches.get(name)
        if not b:
            continue
        tmin = min(v["time"] for v in b.values() if v["time"] > 0)
        for v in b.values():
            if v["time"] > 0:
                all_ratios.append(v["time"] / tmin)
    max_log = max(math.log10(max(r, 1.0)) for r in all_ratios) or 1.0

    def logbar_w(ratio):
        return 4 + (bar_w - 4) * (math.log10(max(ratio, 1.0)) / max_log)

    y = pad

    # ── header ───────────────────────────────────────────────────────────────
    text(pad, y + 24, "Ling vs Python + Panda3D — Engine Feature Benchmark", size=30, weight="bold")
    text(pad, y + 50,
         "math · computing · graphics · audio · crypto · AI · physics — "
         "Panda3D's C++ APIs from CPython vs Ling's builtin surface",
         size=15, fill=MUTED)
    y += 78

    # ── headline callout ──────────────────────────────────────────────────────
    rect(pad, y, W - 2 * pad, 70, PANEL, rx=10)
    if geomean and geomean >= 1.0:
        hl = f"Ling runs this feature suite {geomean:.1f}× faster than Python + Panda3D (geometric mean)."
    elif geomean:
        hl = f"Python + Panda3D runs this feature suite {1 / geomean:.1f}× faster than Ling (geometric mean)."
    else:
        hl = "Ling vs Python + Panda3D."
    text(pad + 20, y + 28, hl, size=17, weight="bold", fill="#E8B84A")
    sub = []
    if ling_wins:
        best = max(ling_wins, key=lambda kv: kv[1])
        sub.append(f"Ling leads {len(ling_wins)} of {len(ling_wins) + len(panda_wins)} features "
                   f"(up to {fmt_ratio(best[1])} on {best[0]})")
    if panda_wins:
        best = min(panda_wins, key=lambda kv: kv[1])
        sub.append(f"Panda3D's C++ leads on {', '.join(n for n, _ in panda_wins)} "
                   f"(up to {fmt_ratio(1 / best[1])})")
    text(pad + 20, y + 52, "   ·   ".join(sub), size=14, fill=TEXT)
    y += 92

    # ── per-benchmark sections ──────────────────────────────────────────────────
    text(pad, y, "Per-feature (log-scaled · shorter bar = faster)", size=18, weight="bold")
    y += 10
    for name in BENCH_ORDER:
        b = benches.get(name)
        if not b:
            continue
        tmin = min(v["time"] for v in b.values() if v["time"] > 0)
        y += 32
        cw = chip(pad, y, BENCH_CAT.get(name, ""))
        text(pad + cw + 10, y, name, size=16, weight="bold")
        y += 18
        text(pad + 12, y, BENCH_DESC.get(name, ""), size=12.5, fill=MUTED)
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
    text(pad, y + 24,
         f"min of {reps} runs · same workload & op order both sides · CPython {meta.get('python', '')} + "
         f"Panda3D {meta.get('panda3d', '?')} (C++ math/noise/hash via Python bindings) · Ling AOT/JIT",
         size=12, fill=MUTED)
    text(pad, y + 42,
         "checksums verified identical for vec_math, vertex_pipeline, fm_synth, boids, spring_rope · "
         "noise_field & hash_chain compare engine-native algorithms (Perlin tables / MD5 vs SHA-256), timing only",
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
