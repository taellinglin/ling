# gen_glossary.py — build the language-switchable Ling glossary from the lexicons.
#
# Source of truth: lexicons/{en,zh,ja,ko,th}.ling  (key = canonical English name,
# value = surface form in that language). We read all five, merge by key, and emit
# Markdown tables with one column per language. docs/lang.js then shows only the
# selected language's column (filterCols) and localizes the sidebar.
#
# Output: docs/src/glossary/*.md  (keywords, types, literals, one page per crate)
# plus a SUMMARY fragment and the lang.js DICT additions printed to stdout.
#
# Run from the repo root:  python docs/gen_glossary.py

import os, re

ROOT  = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LEXD  = os.path.join(ROOT, "lexicons")
OUTD  = os.path.join(ROOT, "docs", "src", "glossary")
LANGS = ["en", "zh", "ja", "ko", "th"]
HEADERS = {"en": "English", "zh": "中文", "ja": "日本語", "ko": "한국어", "th": "ไทย"}

KV = re.compile(r'^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*"(.*)"\s*$')

def parse_flat(code):
    """Flat {canonical_key: surface_form} for one lexicon file."""
    out = {}
    with open(os.path.join(LEXD, f"{code}.ling"), encoding="utf-8") as f:
        for line in f:
            m = KV.match(line)
            if m:
                out[m.group(1)] = m.group(2)
    return out

def parse_en_structured():
    """Walk en.ling tracking [section] and `# group` comment so we keep order."""
    section, group = None, None
    rows = []  # (key, section, group)
    with open(os.path.join(LEXD, "en.ling"), encoding="utf-8") as f:
        for line in f:
            s = line.strip()
            sm = re.match(r'^\[(\w+)\]$', s)
            if sm:
                section, group = sm.group(1), None
                continue
            if s.startswith("#"):
                g = s.lstrip("#").strip(" -─—")
                if g:
                    group = g
                continue
            m = KV.match(line)
            if m and section:
                rows.append((m.group(1), section, group))
    return rows

LEX = {c: parse_flat(c) for c in LANGS}

def surfaces(key):
    en = LEX["en"].get(key, key)
    return [LEX[c].get(key, en) for c in LANGS]

# ── builtin → crate assignment (prefix rules + explicit overrides) ───────────
def crate_of(name):
    pre = [
        ("ui_", "ling-ui"), ("font_", "ling-ui"),
        ("music_", "ling-music"),
        ("audio_", "ling-audio"),
        ("vtex_", "ling-graphics"),
        ("nn_", "ling-ai"), ("bt_", "ling-ai"), ("ai_", "ling-ai"),
        ("knot_", "ling-crypto"), ("crypto_", "ling-crypto"), ("holo_", "ling-crypto"),
        ("physics_", "ling-physics"), ("soft_", "ling-physics"),
        ("rb_", "ling-physics"), ("liquid_", "ling-physics"),
        ("pad_", "ling-input"),
        ("http_", "ling-net"), ("ws_", "ling-net"),
        ("svg_", "ling-graphics"),
        ("orb_", "ling-graphics"),
        ("mic_", "ling-audio"),
    ]
    for p, c in pre:
        if name.startswith(p):
            return c
    explicit = {
        "tone": "ling-audio", "play_wav": "ling-audio", "stop_audio": "ling-audio",
        "set_volume": "ling-audio", "sparkle": "ling-graphics",
        "hash": "ling-crypto", "encrypt": "ling-crypto", "decrypt": "ling-crypto",
        "sign": "ling-crypto", "verify": "ling-crypto",
        "dialog_show": "ling-dialog", "dialog_step": "ling-dialog", "dialog_advance": "ling-dialog",
        "dialog_active": "ling-dialog", "dialog_typing": "ling-dialog", "dialog_close": "ling-dialog",
        "dialog_color": "ling-dialog", "dialog_draw": "ling-dialog",
        "dialog_new": "ling-ai", "dialog_learn": "ling-ai", "dialog_load": "ling-ai",
        "dialog_train": "ling-ai", "dialog_say": "ling-ai", "dialog_save": "ling-ai",
        "dialog_load_model": "ling-ai",
        "tween": "ling-animation", "tween_ease": "ling-animation", "breathe": "ling-animation",
        "wobble": "ling-animation", "gait_phase": "ling-animation", "gait_swing": "ling-animation",
        "gait_lift": "ling-animation", "spring_to": "ling-animation", "ik2": "ling-animation",
        "gear_couple": "ling-animation", "gear_train": "ling-animation", "cam_lift": "ling-animation",
        "piston": "ling-animation", "rack": "ling-animation",
    }
    if name in explicit:
        return explicit[name]
    shapes = {"cube","box","sphere","icosphere","dome","cylinder","cone","capsule","torus",
              "pyramid","prism","frustum","tetrahedron","octahedron","dodecahedron","icosahedron",
              "gear","gyro","helix","spring","arch","stairs","star_prism","capsule_chain","mobius"}
    if name in shapes:
        return "ling-graphics"
    # core language runtime: io, math, noise, time, collections, color
    core = {"print","debug","panic","len","push","pop","new_list","new_map","keys","values",
            "contains","time","sleep","time_now","frame_count","trunc","int","sin","cos","tan",
            "sqrt","abs","floor","ceil","round","pow","log","min","max","clamp","pi","tau","lerp",
            "smoothstep","rand","sign","vnoise","fbm","perlin","hsv_to_rgb","lerp_color","hsl_color"}
    if name in core:
        return "ling (core runtime)"
    # default: window/2-D/3-D drawing, camera, lighting, input → graphics
    return "ling-graphics"

# Sub-section label within a crate page, derived from the en.ling group comment.
def subsection(group):
    if not group:
        return "Misc"
    g = group
    # tidy a couple of long labels
    g = g.replace("(ling-net)", "").replace("(ling-ai)", "").replace("(ling-crypto)", "")
    g = g.replace("(ling-physics)", "").replace("(ling-input)", "").replace("(ling-animation)", "")
    g = re.sub(r"\(ling-graphics[^)]*\)", "", g)
    return g.strip(" -—:") or "Misc"

CRATE_DESC = {
    "ling (core runtime)": "I/O, math, noise, time, colour helpers and collections — always available.",
    "ling-graphics":  "Window, 2-D/3-D drawing, camera, lighting, cel shading, vector textures, surfaces, shadows and SVG export.",
    "ling-audio":     "Tone synthesis, WAV playback, microphone capture, spatial SFX, samples and FX.",
    "ling-music":     "Music decode/analysis, GM synth, rhythm grading, karaoke and MIDI.",
    "ling-ui":        "HUD, meters, controls, game UI widgets and vector fonts.",
    "ling-physics":   "Vector maths, forces, soft bodies, rigid + angular bodies and a liquid sim.",
    "ling-crypto":    "Hashing, AEAD, signatures and the geometric / post-quantum suite.",
    "ling-ai":        "Neural nets, behaviour trees and the dialog LLM.",
    "ling-animation": "The Anima procedural-animation drivers: tweens, gaits, springs, gears and IK.",
    "ling-input":     "Gamepad / joystick polling, sticks, triggers and rumble.",
    "ling-net":       "HTTP and WebSocket clients.",
    "ling-dialog":    "On-screen dialog boxes with typewriter text.",
}
CRATE_ORDER = ["ling (core runtime)","ling-graphics","ling-ui","ling-audio","ling-music",
               "ling-physics","ling-animation","ling-ai","ling-crypto","ling-input",
               "ling-net","ling-dialog"]

def slug(crate):
    return crate.replace("ling (core runtime)", "core").replace("ling-", "").replace(" ", "-")

# ── descriptions for the small glossary sets ────────────────────────────────
KEYWORD_DESC = {
 "bind":"Bind a value or function to a name.","do":"Block expression `do { … }`.",
 "fn":"Function type / lambda.","mod":"Declare a module.","type":"Declare a type.",
 "if":"Conditional branch.","else":"Alternative branch.","while":"Loop while a condition holds.",
 "for":"Iterate over a range or collection.","in":"Iteration source: `for x in 0..10`.",
 "match":"Pattern match.","return":"Return a value from a function.","own":"Ownership: take ownership.",
 "lend":"Ownership: borrow immutably.","share":"Ownership: shared reference.","move":"Ownership: move value.",
 "copy":"Ownership: copy value.","async":"Mark a function/block asynchronous.","wait":"Await an async value.",
 "as":"Type cast / alias.","where":"Constraint clause.","post":"Return a value (idiomatic `return`).",
 "give":"Yield / return a value.","fit":"Pattern / refinement.","form":"Construct a form / struct.",
 "choose":"Enum / selection construct.","can":"Capability / permission.","change":"Mutate a binding.",
 "stop":"Break out of the loop.","again":"Continue the loop.","try":"Fallible expression.",
 "sure":"Assert success.","maybe":"Optional value.","pure":"Mark as pure (no side effects).",
 "spawn":"Spawn a concurrent task.","ok":"Success result.","bad":"Error result.","none":"Absent value.",
 "start":"Program entry point: `bind start = do { … }`.","result":"Result value.",
}
TYPE_DESC = {
 "number":"Numeric type (integer or float).","text":"Text / string type.","bool":"Boolean (true / false).",
 "list":"Ordered list type.","map":"Key / value map type.","tuple":"Fixed-size tuple type.",
}

def table_head():
    cols = " | ".join(HEADERS[c] for c in LANGS)
    sep  = "|".join(["---"] * (len(LANGS) + 1))
    return f"| {cols} | Description |\n|{sep}|\n"

def esc(s):
    return s.replace("|", "\\|")

def emit_named_table(rows, descmap):
    out = table_head()
    for key in rows:
        cells = " | ".join(f"`{esc(v)}`" for v in surfaces(key))
        out += f"| {cells} | {descmap.get(key, '')} |\n"
    return out

# ── build ───────────────────────────────────────────────────────────────────
os.makedirs(OUTD, exist_ok=True)
structured = parse_en_structured()
kw  = [k for (k, s, g) in structured if s == "keywords"]
ty  = [k for (k, s, g) in structured if s == "types"]
bi  = [(k, g) for (k, s, g) in structured if s == "builtins"]

# crate -> subsection -> [keys]   (dedupe, keep first occurrence order)
crates = {}
seen = set()
for key, group in bi:
    if key in seen:
        continue
    seen.add(key)
    c = crate_of(key)
    sub = subsection(group)
    crates.setdefault(c, {}).setdefault(sub, []).append(key)

dict_entries = {}  # english title -> [zh, ja, ko, th]  for lang.js

def write(path, text):
    with open(os.path.join(OUTD, path), "w", encoding="utf-8") as f:
        f.write(text)

# index
total = sum(len(v) for subs in crates.values() for v in subs.values())
idx = ["# Glossary\n",
 "A language-switchable reference for every Ling **keyword**, **type**, **literal** "
 "and **builtin**. Pick a language from the selector in the top bar — the tables and "
 "the sidebar switch to show only that language's spelling.\n",
 f"- **{len(kw)}** keywords · **{len(ty)}** types · **{total}** builtins across "
 f"**{len(crates)}** crates.\n",
 "\n## Builtins by crate\n"]
for c in CRATE_ORDER:
    if c in crates:
        idx.append(f"- [{c}](crate-{slug(c)}.md) — {CRATE_DESC.get(c,'')}\n")
write("index.md", "".join(idx))

# keywords / types
write("keywords.md", "# Keywords\n\nReserved words and their spelling in each language.\n\n" + emit_named_table(kw, KEYWORD_DESC))
write("types.md",    "# Types\n\nBuilt-in types.\n\n" + emit_named_table(ty, TYPE_DESC))

# literals & operators (static — not in the lexicon)
write("literals.md", """# Literals & operators

Literals and operators are spelled the same in every language.

## Literals

| Literal | Meaning |
|---------|---------|
| `true` / `false` | Boolean values |
| `ok` / `bad` | Result success / error |
| `none` | Absent value |
| `123`, `1.5`, `-7` | Numbers |
| `"hello"` | Text |
| `[1, 2, 3]` | List |
| `0..10` | Range (used by `for`) |

## Operators

| Operator | Meaning |
|----------|---------|
| `+` `-` `*` `/` `%` | Arithmetic |
| `==` `!=` `<` `<=` `>` `>=` | Comparison |
| `&&` `\\|\\|` `!` | Logical and / or / not |
| `=` | Bind / assign |
| `->` | Function return type |
| `.` | Field / method access |
| `\\|>` | Pipe |
""")

# per-crate pages
for c in CRATE_ORDER:
    if c not in crates:
        continue
    body = [f"# {c}\n\n{CRATE_DESC.get(c,'')}\n"]
    for sub, keys in crates[c].items():
        if sub and sub != "Misc":
            body.append(f"\n## {sub}\n\n")
        else:
            body.append("\n")
        head = " | ".join(HEADERS[x] for x in LANGS)
        sep  = "|".join(["---"] * len(LANGS))
        body.append(f"| {head} |\n|{sep}|\n")
        for key in keys:
            body.append("| " + " | ".join(f"`{esc(v)}`" for v in surfaces(key)) + " |\n")
    write(f"crate-{slug(c)}.md", "".join(body))

# SUMMARY fragment + lang.js DICT additions
print("=== SUMMARY fragment ===")
print("# Glossary\n")
print("- [Overview](glossary/index.md)")
print("- [Keywords](glossary/keywords.md)")
print("- [Types](glossary/types.md)")
print("- [Literals & operators](glossary/literals.md)")
print("- [Builtins by crate]()")
for c in CRATE_ORDER:
    if c in crates:
        print(f"  - [{c}](glossary/crate-{slug(c)}.md)")
print(f"\n=== generated {len(crates)} crate pages, {total} builtins ===")
