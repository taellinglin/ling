// Offline unit test for the Ling extension's lint + inlay-hint logic.
// Stubs the `vscode` module (no editor needed) and a minimal TextDocument, then
// asserts diagnostics and inlay hints on small snippets. Run: `node test/lint.test.js`.

const assert = require("assert");
const Module = require("module");

// ── minimal `vscode` stub ────────────────────────────────────────────────────
const Severity = { Error: 0, Warning: 1, Information: 2, Hint: 3 };
const InlayKind = { Type: 1, Parameter: 2 };
class Position {
  constructor(off) { this.off = off; }
  isBefore(o) { return this.off < o.off; }
  isAfter(o) { return this.off > o.off; }
}
class Range { constructor(s, e) { this.start = s; this.end = e; } }
class Diagnostic { constructor(range, message, severity) { this.range = range; this.message = message; this.severity = severity; } }
class InlayHint { constructor(position, label, kind) { this.position = position; this.label = label; this.kind = kind; } }
const vscodeStub = {
  DiagnosticSeverity: Severity,
  InlayHintKind: InlayKind,
  Position, Range, Diagnostic, InlayHint,
  workspace: { getConfiguration: () => ({ get: (_k, d) => d }) },
};
const origLoad = Module._load;
Module._load = function (req, parent, isMain) {
  if (req === "vscode") return vscodeStub;
  return origLoad.call(this, req, parent, isMain);
};

const { _internal } = require("../src/extension.js");
const { maskNonCode, splitArgs, paramNames, lint, inlayHintsFor } = _internal;

// ── fake TextDocument (offsets == positions for simplicity) ──────────────────
function doc(text) {
  return {
    languageId: "ling",
    getText: () => text,
    positionAt: (off) => new Position(off),
    offsetAt: (pos) => pos.off,
  };
}
const CFG = { lint: true, unknownCalls: true, inlayHints: true };
let passed = 0;
function ok(name, cond) { assert.ok(cond, name); passed++; console.log("  ok -", name); }

// 1) maskNonCode blanks strings + comments but keeps code offsets
{
  const src = 'print("a (b) {c}") // }}}\nbind x = 1';
  const m = maskNonCode(src);
  ok("mask keeps length", m.length === src.length);
  ok("mask blanks brace in string", !m.slice(0, 18).includes("{"));
  ok("mask keeps real code", m.includes("bind x = 1"));
}

// 2) clean program → no diagnostics
{
  const d = lint(doc('bind start = do {\n  set_color(255, 0, 0)\n  present()\n}'), CFG);
  ok("clean program lints clean", d.length === 0);
}

// 3) unbalanced bracket → error
{
  const d = lint(doc("bind start = do {\n  present(\n}"), CFG);
  ok("unbalanced paren flagged", d.some((x) => x.severity === Severity.Error && /Unmatched|Unclosed/.test(x.message)));
}

// 4) `let` → error suggesting bind
{
  const d = lint(doc("let x = 1"), CFG);
  ok("let flagged", d.some((x) => /bind/.test(x.message)));
}

// 5) unknown ascii call → warning; known builtin + local fn → clean
{
  const d = lint(doc('bind start = do {\n  set_collor(1,2,3)\n}'), CFG);
  ok("typo'd builtin warned", d.some((x) => x.severity === Severity.Warning && /set_collor/.test(x.message)));

  const d2 = lint(doc('bind helper = fn(x) { x }\nbind start = do { helper(3) cast_shadow(1,2,3) }'), CFG);
  ok("local fn + new builtin not flagged", !d2.some((x) => x.severity === Severity.Warning));
}

// 6) non-ASCII (Thai) call never flagged
{
  const d = lint(doc("bind start = do {\n  เติม(0,0,0)\n}"), CFG);
  ok("thai builtin not flagged", !d.some((x) => x.severity === Severity.Warning));
}

// 7) signature param parsing + inlay hints
{
  ok("paramNames parses types", JSON.stringify(paramNames("set_color(r: number, g: number, b: number)")) === '["r","g","b"]');
  ok("splitArgs respects nesting", splitArgs("a, f(b, c), d").length === 3);

  const text = "bind start = do { set_color(255, 0, 0) }";
  const full = doc(text);
  const hints = inlayHintsFor(full, new Range(new Position(0), new Position(text.length)));
  ok("3 inlay hints for set_color", hints.length === 3);
  ok("first hint is r:", hints[0].label === "r:");
  ok("third hint is b:", hints[2].label === "b:");
}

// 8) new gfx builtin has a signature → hints work
{
  const text = "bind start = do { shadow_blob(50, 50, 16, 8, 0.4) }";
  const hints = inlayHintsFor(doc(text), new Range(new Position(0), new Position(text.length)));
  ok("shadow_blob hints", hints.length === 5 && hints[0].label === "cx:" && hints[4].label === "alpha:");
}

console.log(`\nAll ${passed} checks passed.`);
