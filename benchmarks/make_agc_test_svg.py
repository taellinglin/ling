#!/usr/bin/env python3
"""Render agc_test_suite.json into an SVG infographic.

Usage:
    python make_agc_test_svg.py [agc_test_suite.json] [agc_test_suite.svg]
"""

import json
import sys

BG = "#14233D"
PANEL = "#1C2F4F"
TEXT = "#E6ECF5"
MUTED = "#8D99AE"
GRID = "#2C4067"


def esc(s):
    return str(s).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else "agc_test_suite.json"
    out = sys.argv[2] if len(sys.argv) > 2 else "agc_test_suite.svg"

    with open(src, "r", encoding="utf-8") as fh:
        data = json.load(fh)

    svg = build_svg(data)
    with open(out, "w", encoding="utf-8") as fh:
        fh.write(svg)

    print(f"wrote {out}")


def build_svg(data):
    meta = data.get("meta", {})
    categories = data.get("categories", [])

    W = 1200
    pad = 40
    y = pad

    parts = []

    def rect(x, yy, w, h, fill, rx=0, opacity=1.0, stroke="none"):
        parts.append(
            f'<rect x="{x:.1f}" y="{yy:.1f}" width="{w:.1f}" height="{h:.1f}" '
            f'rx="{rx}" fill="{fill}" opacity="{opacity}" stroke="{stroke}"/>'
        )

    def text(x, yy, s, size=15, fill=TEXT, anchor="start", weight="normal", mono=False):
        fam = "Consolas, 'DejaVu Sans Mono', monospace" if mono else "'Segoe UI', 'DejaVu Sans', sans-serif"
        parts.append(
            f'<text x="{x:.1f}" y="{yy:.1f}" font-family="{fam}" font-size="{size}" '
            f'fill="{fill}" text-anchor="{anchor}" font-weight="{weight}">{esc(s)}</text>'
        )

    def wrap_lines(s, max_chars=86):
        words = str(s).split()
        if not words:
            return [""]
        lines = []
        cur = words[0]
        for w in words[1:]:
            if len(cur) + 1 + len(w) <= max_chars:
                cur += " " + w
            else:
                lines.append(cur)
                cur = w
        lines.append(cur)
        return lines

    title = meta.get("title", "Audio/Graphics/Crypto Test Suite")
    subtitle = meta.get("subtitle", "Workload definitions and metrics")
    version = meta.get("version", "")
    date = meta.get("date", "")

    text(pad, y + 24, title, size=30, weight="bold")
    text(pad, y + 50, subtitle, size=15, fill=MUTED)
    text(W - pad, y + 24, f"v{version}", size=13, fill=MUTED, anchor="end")
    text(W - pad, y + 44, date, size=13, fill=MUTED, anchor="end")
    y += 76

    rect(pad, y, W - 2 * pad, 66, PANEL, rx=10)
    text(pad + 20, y + 28, "How to use this suite", size=17, weight="bold", fill="#E8B84A")
    text(
        pad + 20,
        y + 52,
        "Run each test with fixed input sizes, report median of N runs, and compare by the listed metric.",
        size=14,
        fill=TEXT,
    )
    y += 90

    row_h = 84
    bar_x = 950
    bar_w = 180

    for cat in categories:
        cname = cat.get("name", "Category")
        ccolor = cat.get("color", "#E8B84A")
        tests = cat.get("tests", [])

        text(pad, y, cname, size=22, weight="bold", fill=ccolor)
        y += 16

        for t in tests:
            rect(pad, y, W - 2 * pad, row_h, PANEL, rx=8, opacity=0.98)

            tid = t.get("id", "TST")
            tname = t.get("name", "Unnamed test")
            tdesc = t.get("description", "")
            tin = t.get("input", "")
            tmetric = t.get("metric", "")
            workload = float(t.get("workload", 5))
            workload = max(1.0, min(workload, 10.0))

            text(pad + 16, y + 24, f"{tid}  {tname}", size=16, weight="bold", fill=TEXT)

            dlines = wrap_lines(tdesc, max_chars=82)
            base_y = y + 44
            for i, ln in enumerate(dlines[:2]):
                text(pad + 16, base_y + i * 16, ln, size=13, fill=MUTED)

            text(pad + 16, y + 74, f"Input: {tin}", size=12, fill="#B9C5D8", mono=True)
            text(pad + 510, y + 74, f"Metric: {tmetric}", size=12, fill="#B9C5D8", mono=True)

            text(bar_x, y + 22, "Workload", size=12, fill=MUTED)
            rect(bar_x, y + 30, bar_w, 16, GRID, rx=6, opacity=0.45)
            rect(bar_x, y + 30, (bar_w * workload) / 10.0, 16, ccolor, rx=6)
            text(bar_x + bar_w, y + 44, f"{workload:.0f}/10", size=12, fill=TEXT, anchor="end", mono=True)

            y += row_h + 10

        y += 12

    rect(pad, y + 8, W - 2 * pad, 1, GRID)
    text(
        pad,
        y + 30,
        "Generated from agc_test_suite.json. Style aligned with the existing benchmark infographic.",
        size=12,
        fill=MUTED,
    )
    H = y + 52

    body = "\n".join(parts)
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H:.0f}" '
        f'viewBox="0 0 {W} {H:.0f}">\n'
        f'<rect width="{W}" height="{H:.0f}" fill="{BG}"/>\n{body}\n</svg>\n'
    )


if __name__ == "__main__":
    main()
