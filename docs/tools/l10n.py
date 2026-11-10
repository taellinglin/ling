#!/usr/bin/env python3
"""Docs helpers: localize Ling code and verify the examples in a docs page.

    python docs/tools/l10n.py localize prog.ling --to zh     # print prog.ling in zh keywords/builtins
    python docs/tools/l10n.py check docs/src/tutorials/values.md
    python docs/tools/l10n.py check docs/i18n/th/src/tutorials/values.md
    python docs/tools/l10n.py check-all                      # every page of every language

Fences in the docs:
    ```ling         runnable, prints to stdout; RUN and, if the next fence is
                    ```output, its stdout must match that block exactly
    ```ling-gui     needs a window/audio/network/server; PARSE-checked only
    ```ling-module  a fragment (no start block); PARSE-checked only
    ```ling-interp  like ```ling but run with `ling run --interp` (the LingBook uses it)
    ```output       expected stdout of the ```ling block just above
    anything else   (sh, rust, toml, text, ...) is not checked

`localize` runs `lingfu normalize --to <lang>` on a private copy (normalize
rewrites every .ling file in its working directory, so never run it in place).
It translates keywords and builtins; translate identifiers, comments and
strings yourself, then `check` the page again.

Binaries: $LING / $LINGFU, else ../target/release/ling(.exe) next to docs/.
"""
import os, re, shutil, subprocess, sys, tempfile, glob

HERE = os.path.dirname(os.path.abspath(__file__))
DOCS = os.path.dirname(HERE)
REPO = os.path.dirname(DOCS)
EXE = ".exe" if os.name == "nt" else ""
LING = os.environ.get("LING") or os.path.join(REPO, "target", "release", "ling" + EXE)
LINGFU = os.environ.get("LINGFU") or os.path.join(REPO, "target", "release", "lingfu" + EXE)
TIMEOUT = 30

# ── Unknown-call scan ──────────────────────────────────────────────────────
# A parse check can't see a call to a builtin that doesn't exist (e.g. a
# made-up localized alias): the JIT silently returns () and only the
# interpreter errors at run time. So every `name(` in a Ling block must be a
# runtime builtin (a string literal in the runtime/lexer sources), a keyword,
# or something the block itself defines (fn / form / choose variant / bind).
_KNOWN = None


def known_names():
    global _KNOWN
    if _KNOWN is None:
        names = set()
        srcs = glob.glob(os.path.join(REPO, "src", "**", "*.rs"), recursive=True)
        # crate-dispatched builtins (daw_*, body_*, video_*, shapes, ...)
        # (not the localization tables of lingfu normalize / polyglot / lexer crates:
        # they list aliases the runtime doesn't implement, which is what we hunt)
        skip = ("ling-fu",)
        srcs += [f for f in glob.glob(os.path.join(REPO, "crates", "*", "src", "**", "*.rs"), recursive=True)
                 if not any(os.sep + c + os.sep in f for c in skip)]
        for f in srcs:
            try:
                txt = open(f, encoding="utf-8", errors="replace").read()
            except OSError:
                continue
            names.update(re.findall(r'"([^"\\\s(){}\[\],.;:]{1,48})"', txt))
        _KNOWN = names
    return _KNOWN


# Identifier characters: \w misses Thai vowel/tone marks (category Mn), so add
# the Thai block and combining marks explicitly.
_W = r"[\w฀-๿̀-ͯ]"
_ID = r"(?:(?!\d)" + _W + r")" + _W + r"*"
CALL = re.compile(r"(?<![\w฀-๿.:])(" + _ID + r")\s*\(")
# Keyword spellings in all five languages (lexer-verified, docs/src/language/keywords.md).
_KW_FN = ["fn", "函", "関数", "関", "함수", "ฟังก์ชัน"]
_KW_TYPE = ["form", "形", "形式", "構造", "구조", "โครงสร้าง",
            "choose", "选", "选择", "選択", "선택", "เลือกแบบ", "mod", "核", "モジュール", "模", "모듈", "โมดูล"]
_KW_BIND = ["bind", "令", "灵符", "束縛", "バ", "바인드", "묶", "ผูก"]
# builtin families dispatched by prefix (src/semantic/builtins.rs): the part after the
# prefix is what appears as a string literal in the runtime/crate sources
_PREFIXES = ("daw_", "fs_", "dlg_", "st_", "px_", "body_", "video_")


def _kw(words):
    return "(?:" + "|".join(re.escape(w) for w in sorted(words, key=len, reverse=True)) + ")"


_NB = r"(?<![\w฀-๿])"
DEFS = [re.compile(p) for p in (
    _NB + _kw(_KW_FN) + r"\s+(" + _ID + r")\s*\(",            # fn NAME(
    _NB + _kw(_KW_TYPE) + r"\s+(" + _ID + r")\s*\{",          # form/choose/mod NAME {
    _NB + _kw(_KW_BIND) + r"\s+(" + _ID + r")\s*=",           # bind NAME = (closures)
)]


_KW_USE = ["use", "载", "引", "사용", "使う", "ใช้", "นำเข้า"]


def unknown_calls(code):
    # a block that imports another file may call functions defined there
    if re.search(r"(?m)^\s*" + _kw(_KW_USE) + r"\s+\"", code):
        return []
    body = re.sub(r'"(\\.|[^"\\])*"', '""', code)             # drop string contents
    body = re.sub(r"(?m)(//|#).*$", "", body)                      # drop comments
    defined = set()
    for rx in DEFS:
        defined.update(rx.findall(body))
    # parameters of fn definitions and closures may be called as functions
    for m in re.finditer(r"\(([^()]*)\)\s*\{|\|([^|]*)\|", body):
        defined.update(re.findall(_ID, m.group(1) or m.group(2) or ""))
    # choose { A(x), B, C(y, z) } — a brace group that is only a comma list of
    # identifiers (optionally with a parameter list) declares variants.
    # (linear check — no nested quantifiers, which backtrack badly on big blocks)
    ident = re.compile(_ID)
    for m in re.finditer(r"\{([^{}]*)\}", body):
        items = re.sub(r"\([^()]*\)", "", m.group(1)).split(",")
        names = [it.strip() for it in items if it.strip()]
        if names and all(ident.fullmatch(nm) for nm in names):
            defined.update(names)
    k = known_names()
    bad = []
    for name in CALL.findall(body):
        if name in defined or name in k:
            continue
        if name.startswith(_PREFIXES) and name.split("_", 1)[1] in k:
            continue
        bad.append(name)
    return sorted(set(bad))


FENCE = re.compile(r"^```([A-Za-z0-9_-]*)[^\n]*\n(.*?)^```[ \t]*$", re.M | re.S)


def localize(code, lang):
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "p.ling")
        with open(p, "w", encoding="utf-8", newline="\n") as f:
            f.write(code)
        r = subprocess.run([LINGFU, "normalize", "--to", lang], cwd=d,
                           capture_output=True, text=True, encoding="utf-8", errors="replace")
        if r.returncode != 0:
            raise SystemExit(f"lingfu normalize failed:\n{r.stdout}\n{r.stderr}")
        with open(p, encoding="utf-8") as f:
            return f.read()


def run(code, parse_only, interp=False):
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "ex.ling")
        with open(p, "w", encoding="utf-8", newline="\n") as f:
            f.write(code)
        cmd = [LING, "visualize", p] if parse_only else ([LING, "run", "--interp", p] if interp else [LING, "run", p])
        try:
            r = subprocess.run(cmd, cwd=d, capture_output=True, timeout=TIMEOUT,
                               text=True, encoding="utf-8", errors="replace")
        except subprocess.TimeoutExpired:
            return False, "", f"timed out after {TIMEOUT}s"
        return r.returncode == 0, ("" if parse_only else r.stdout), r.stderr


def norm(s):
    return "\n".join(l.rstrip() for l in s.strip().replace("\r\n", "\n").split("\n"))


def check(path):
    text = open(path, encoding="utf-8").read()
    blocks = [(m.group(1), m.group(2), text.count("\n", 0, m.start()) + 1) for m in FENCE.finditer(text)]
    fails = 0
    n = 0
    for i, (kind, code, line) in enumerate(blocks):
        if kind not in ("ling", "ling-gui", "ling-module", "ling-interp"):
            continue
        n += 1
        runnable = kind in ("ling", "ling-interp")
        ok, out, err = run(code, parse_only=not runnable, interp=(kind == "ling-interp"))
        msg = None
        unk = unknown_calls(code)
        if not ok:
            msg = (err or out).strip().splitlines()[:6]
            msg = "error: " + " | ".join(msg)
        elif runnable and i + 1 < len(blocks) and blocks[i + 1][0] == "output":
            want = norm(blocks[i + 1][1])
            got = norm(out)
            if want != got:
                msg = f"output differs\n      want: {want[:300]!r}\n      got:  {got[:300]!r}"
        if not msg and unk:
            msg = "unknown function(s): " + ", ".join(unk)
        if msg:
            fails += 1
            print(f"  FAIL {os.path.relpath(path, REPO)}:{line} ```{kind}  {msg}")
    print(f"{'ok  ' if not fails else 'FAIL'} {os.path.relpath(path, REPO)}: {n} example(s), {fails} failing")
    return fails


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    cmd = sys.argv[1]
    if cmd == "localize":
        src = sys.argv[2]
        lang = sys.argv[sys.argv.index("--to") + 1]
        code = sys.stdin.read() if src == "-" else open(src, encoding="utf-8").read()
        sys.stdout.reconfigure(encoding="utf-8")
        print(localize(code, lang), end="")
        return 0
    if cmd == "check":
        return 1 if sum(check(p) for p in sys.argv[2:]) else 0
    if cmd == "scan":
        # fast: only the unknown-function scan, no running. Paths or globs.
        hits = 0
        for pat in sys.argv[2:]:
            for p in sorted(glob.glob(pat, recursive=True)):
                text = open(p, encoding="utf-8").read()
                for m in FENCE.finditer(text):
                    if m.group(1) in ("ling", "ling-gui", "ling-module", "ling-interp"):
                        unk = unknown_calls(m.group(2))
                        if unk:
                            hits += 1
                            line = text.count("\n", 0, m.start()) + 1
                            print(f"{os.path.relpath(p, REPO)}:{line}: {', '.join(unk)}")
        return 1 if hits else 0
    if cmd == "check-all":
        pages = sorted(glob.glob(os.path.join(DOCS, "src", "**", "*.md"), recursive=True))
        pages += sorted(glob.glob(os.path.join(DOCS, "i18n", "*", "src", "**", "*.md"), recursive=True))
        return 1 if sum(check(p) for p in pages) else 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main())
