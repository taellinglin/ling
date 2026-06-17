// Ling language extension: completion, hover, signature help, and document symbols.
// Plain CommonJS — the `vscode` module is provided by the host, so no build step
// or `npm install` is required.

const vscode = require("vscode");
const data = require("./lingdata");

const LANG = "ling";
const WORD_RE = /[\p{L}_][\p{L}\p{N}_]*/u;

/** Build the static completion list once. */
function buildCompletions() {
  const items = [];

  for (const [name, doc] of Object.entries(data.keywords)) {
    const it = new vscode.CompletionItem(name, vscode.CompletionItemKind.Keyword);
    it.detail = "keyword";
    it.documentation = new vscode.MarkdownString(doc);
    items.push(it);
  }

  for (const [name, doc] of Object.entries(data.types)) {
    const it = new vscode.CompletionItem(name, vscode.CompletionItemKind.TypeParameter);
    it.detail = "type";
    it.documentation = new vscode.MarkdownString(doc);
    items.push(it);
  }

  for (const [name, doc] of Object.entries(data.constants)) {
    const it = new vscode.CompletionItem(name, vscode.CompletionItemKind.Constant);
    it.detail = "constant";
    it.documentation = new vscode.MarkdownString(doc);
    items.push(it);
  }

  for (const name of data.builtins) {
    const it = new vscode.CompletionItem(name, vscode.CompletionItemKind.Function);
    const sig = data.signatures[name];
    it.detail = sig || data.builtinCategory[name];
    const md = new vscode.MarkdownString();
    if (sig) md.appendCodeblock(sig, "ling");
    md.appendMarkdown("\n_" + data.builtinCategory[name] + " builtin_");
    it.documentation = md;
    // Insert a call template for builtins that take arguments.
    if (sig && sig.includes("(") && !/\(\)/.test(sig)) {
      it.insertText = new vscode.SnippetString(name + "($0)");
    }
    items.push(it);
  }

  return items;
}

/** Markdown hover body for a known word, or null. */
function hoverFor(word) {
  if (data.keywords[word]) {
    const md = new vscode.MarkdownString();
    md.appendCodeblock(word, "ling");
    md.appendMarkdown("**keyword** — " + data.keywords[word]);
    return md;
  }
  if (data.types[word]) {
    const md = new vscode.MarkdownString();
    md.appendCodeblock(word, "ling");
    md.appendMarkdown("**type** — " + data.types[word]);
    return md;
  }
  if (data.constants[word]) {
    const md = new vscode.MarkdownString();
    md.appendCodeblock(word, "ling");
    md.appendMarkdown("**constant** — " + data.constants[word]);
    return md;
  }
  if (data.builtinCategory[word]) {
    const md = new vscode.MarkdownString();
    md.appendCodeblock(data.signatures[word] || word, "ling");
    md.appendMarkdown("_" + data.builtinCategory[word] + " builtin_");
    return md;
  }
  return null;
}

/**
 * Scan left from `position` on the current line to find an enclosing `name(`
 * call. Returns { name, activeParameter } or null.
 */
function enclosingCall(document, position) {
  const line = document.lineAt(position.line).text;
  let depth = 0;
  let commas = 0;
  for (let i = position.character - 1; i >= 0; i--) {
    const ch = line[i];
    if (ch === ")") depth++;
    else if (ch === "(") {
      if (depth === 0) {
        // Found the opening paren of the enclosing call. Read the name before it.
        const before = line.slice(0, i);
        const m = before.match(/([\p{L}_][\p{L}\p{N}_]*)\s*$/u);
        if (m) return { name: m[1], activeParameter: commas };
        return null;
      }
      depth--;
    } else if (ch === "," && depth === 0) {
      commas++;
    }
  }
  return null;
}

// ─── Shared scanning helpers (linting + inlay hints) ─────────────────────────

// Return a copy of `text` with the contents of strings ("...") and line comments
// (`//…` and `#…`) replaced by spaces, preserving every offset. Bracket and
// identifier scanning then operates on real code only and never trips on a
// brace inside a string or a `(` inside a comment.
function maskNonCode(text) {
  const out = text.split("");
  let i = 0;
  const n = text.length;
  let inStr = false;
  while (i < n) {
    const c = text[i];
    if (inStr) {
      if (c === "\\") { out[i] = " "; out[i + 1] = " "; i += 2; continue; }
      if (c === '"') { inStr = false; out[i] = " "; i++; continue; }
      out[i] = " "; i++; continue;
    }
    if (c === '"') { inStr = true; out[i] = " "; i++; continue; }
    // line comment: // or #  → blank to end of line
    if ((c === "/" && text[i + 1] === "/") || c === "#") {
      while (i < n && text[i] !== "\n") { out[i] = " "; i++; }
      continue;
    }
    i++;
  }
  return out.join("");
}

// Names defined locally in the document (so the unknown-call lint doesn't flag a
// user's own functions, parameters, loop variables or fields). Collected
// generously — over-including only means we miss a typo, never a false alarm.
function localNames(code) {
  const names = new Set();
  let m;
  const add = (re, g) => { re.lastIndex = 0; while ((m = re.exec(code))) names.add(m[g]); };
  add(/\bbind\s+([\p{L}_][\p{L}\p{N}_]*)/gu, 1);   // bind NAME = …
  add(/\bfn\s+([\p{L}_][\p{L}\p{N}_]*)/gu, 1);     // fn NAME(…)
  add(/\bfor\s+([\p{L}_][\p{L}\p{N}_]*)\s+in\b/gu, 1); // for NAME in …
  add(/([\p{L}_][\p{L}\p{N}_]*)\s*:/gu, 1);        // param/field NAME:
  add(/([\p{L}_][\p{L}\p{N}_]*)\s*=/gu, 1);        // NAME = … (assignment)
  return names;
}

// Split a comma-separated argument list at top level (respecting nested
// (), [], {}). Returns [{ text, start }] with `start` an offset into `inner`.
function splitArgs(inner) {
  const parts = [];
  let depth = 0, start = 0;
  for (let i = 0; i <= inner.length; i++) {
    const ch = inner[i];
    if (i === inner.length || (ch === "," && depth === 0)) {
      parts.push({ text: inner.slice(start, i), start });
      start = i + 1;
    } else if (ch === "(" || ch === "[" || ch === "{") depth++;
    else if (ch === ")" || ch === "]" || ch === "}") depth--;
  }
  return parts;
}

// Parameter names parsed out of a curated signature string, e.g.
// "set_color(r: number, g: number, b: number)" → ["r", "g", "b"].
function paramNames(sig) {
  const lp = sig.indexOf("(");
  const rp = sig.lastIndexOf(")");
  if (lp < 0 || rp <= lp) return [];
  return splitArgs(sig.slice(lp + 1, rp))
    .map((p) => p.text.trim().replace(/^\.\.\./, "").split(":")[0].trim())
    .filter(Boolean);
}

const KNOWN = new Set([
  ...Object.keys(data.keywords),
  ...Object.keys(data.types),
  ...Object.keys(data.constants),
  ...data.builtins,
]);

// Produce diagnostics for one document: unbalanced brackets, `let` (Ling uses
// `bind`), and calls to unknown ASCII-named functions (typo catcher). Non-ASCII
// (Thai/CJK) names are never flagged, since our tables are English-canonical.
function lint(document, cfg) {
  if (document.languageId !== LANG) return [];
  const text = document.getText();
  const code = maskNonCode(text);
  const diags = [];
  const at = (off, len, msg, sev) => {
    const start = document.positionAt(off);
    const end = document.positionAt(off + len);
    const d = new vscode.Diagnostic(new vscode.Range(start, end), msg, sev);
    d.source = "ling";
    diags.push(d);
  };

  // 1) bracket balance
  const pairs = { ")": "(", "]": "[", "}": "{" };
  const opens = { "(": ")", "[": "]", "{": "}" };
  const stack = [];
  for (let i = 0; i < code.length; i++) {
    const c = code[i];
    if (opens[c]) stack.push({ c, i });
    else if (pairs[c]) {
      if (!stack.length || stack[stack.length - 1].c !== pairs[c]) {
        at(i, 1, `Unmatched '${c}'.`, vscode.DiagnosticSeverity.Error);
      } else stack.pop();
    }
  }
  for (const s of stack) at(s.i, 1, `Unclosed '${s.c}'.`, vscode.DiagnosticSeverity.Error);

  // 2) `let` is not a Ling keyword
  let m;
  const letRe = /\blet\b/g;
  while ((m = letRe.exec(code))) {
    at(m.index, 3, "Ling has no `let` — use `bind` to introduce a binding.", vscode.DiagnosticSeverity.Error);
  }

  // 3) unknown function calls (ASCII names only)
  if (cfg.unknownCalls) {
    const locals = localNames(code);
    const callRe = /(^|[^.\p{L}\p{N}_])([a-z][a-z0-9_]*)\s*\(/gu;
    while ((m = callRe.exec(code))) {
      const name = m[2];
      if (KNOWN.has(name) || locals.has(name)) continue;
      const off = m.index + m[1].length;
      at(off, name.length, `Unknown function \`${name}\` — not a builtin or a defined binding.`,
        vscode.DiagnosticSeverity.Warning);
    }
  }
  return diags;
}

// Inlay parameter-name hints at call sites with a known signature.
function inlayHintsFor(document, range) {
  const hints = [];
  const full = document.getText();
  const code = maskNonCode(full);
  const startOff = document.offsetAt(range.start);
  const endOff = document.offsetAt(range.end);
  const callRe = /(^|[^.\p{L}\p{N}_])([a-z_][a-z0-9_]*)\s*\(/gu;
  let m;
  while ((m = callRe.exec(code))) {
    const name = m[2];
    const sig = data.signatures[name];
    if (!sig) continue;
    const params = paramNames(sig);
    if (!params.length) continue;
    const open = m.index + m[0].length - 1; // index of '('
    if (open > endOff || open < startOff - 200) continue;
    // find matching ')' (same masked code, any line)
    let depth = 0, close = -1;
    for (let i = open; i < code.length; i++) {
      const ch = code[i];
      if (ch === "(") depth++;
      else if (ch === ")") { depth--; if (depth === 0) { close = i; break; } }
    }
    if (close < 0) continue;
    const inner = code.slice(open + 1, close);
    if (!inner.trim()) continue;
    const args = splitArgs(inner);
    args.forEach((arg, idx) => {
      if (idx >= params.length) return;            // varargs tail — stop naming
      if (!arg.text.trim()) return;
      const lead = arg.text.length - arg.text.replace(/^\s+/, "").length;
      const pos = document.positionAt(open + 1 + arg.start + lead);
      if (pos.isBefore(range.start) || pos.isAfter(range.end)) return;
      const hint = new vscode.InlayHint(pos, params[idx] + ":", vscode.InlayHintKind.Parameter);
      hint.paddingRight = true;
      hints.push(hint);
    });
  }
  return hints;
}

function readConfig() {
  const c = vscode.workspace.getConfiguration("ling");
  return {
    lint: c.get("lint.enabled", true),
    unknownCalls: c.get("lint.unknownCalls", true),
    inlayHints: c.get("inlayHints.enabled", true),
  };
}

function activate(context) {
  const sel = { language: LANG, scheme: "file" };
  const completions = buildCompletions();

  // ── Linting (diagnostics) ──
  const diagnostics = vscode.languages.createDiagnosticCollection("ling");
  context.subscriptions.push(diagnostics);
  const refresh = (doc) => {
    if (!doc || doc.languageId !== LANG) return;
    const cfg = readConfig();
    diagnostics.set(doc.uri, cfg.lint ? lint(doc, cfg) : []);
  };
  vscode.workspace.textDocuments.forEach(refresh);
  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(refresh),
    vscode.workspace.onDidChangeTextDocument((e) => refresh(e.document)),
    vscode.workspace.onDidCloseTextDocument((doc) => diagnostics.delete(doc.uri)),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("ling")) vscode.workspace.textDocuments.forEach(refresh);
    })
  );

  // ── Inlay hints (parameter names at call sites) ──
  context.subscriptions.push(
    vscode.languages.registerInlayHintsProvider(sel, {
      provideInlayHints(document, range) {
        return readConfig().inlayHints ? inlayHintsFor(document, range) : [];
      }
    })
  );

  context.subscriptions.push(
    vscode.languages.registerCompletionItemProvider(sel, {
      provideCompletionItems() {
        return completions;
      }
    })
  );

  context.subscriptions.push(
    vscode.languages.registerHoverProvider(sel, {
      provideHover(document, position) {
        const range = document.getWordRangeAtPosition(position, WORD_RE);
        if (!range) return null;
        const body = hoverFor(document.getText(range));
        return body ? new vscode.Hover(body, range) : null;
      }
    })
  );

  context.subscriptions.push(
    vscode.languages.registerSignatureHelpProvider(
      sel,
      {
        provideSignatureHelp(document, position) {
          const call = enclosingCall(document, position);
          if (!call) return null;
          const sig = data.signatures[call.name];
          if (!sig) return null;
          const help = new vscode.SignatureHelp();
          const info = new vscode.SignatureInformation(sig);
          // Parse parameters from the "(...)" portion of the signature.
          const inner = sig.slice(sig.indexOf("(") + 1, sig.lastIndexOf(")"));
          const params = inner.split(",").map((p) => p.trim()).filter(Boolean);
          info.parameters = params.map((p) => new vscode.ParameterInformation(p));
          help.signatures = [info];
          help.activeSignature = 0;
          help.activeParameter = Math.min(call.activeParameter, Math.max(params.length - 1, 0));
          return help;
        }
      },
      "(",
      ","
    )
  );

  context.subscriptions.push(
    vscode.languages.registerDocumentSymbolProvider(sel, {
      provideDocumentSymbols(document) {
        const symbols = [];
        // `bind <name> = ...` — function if it binds a lambda/do-block, else variable.
        const re = /^\s*(?:bind|令|灵符|束縛|바인드|enlazar|lier|binden|ligar)\s+([\p{L}_][\p{L}\p{N}_]*)\s*=(.*)$/u;
        for (let i = 0; i < document.lineCount; i++) {
          const text = document.lineAt(i).text;
          const m = re.exec(text);
          if (!m) continue;
          const name = m[1];
          const rhs = m[2];
          const isFn = /(\(|->|\bdo\b|\bfn\b|\b执\b|\b函\b)/u.test(rhs);
          const kind = isFn ? vscode.SymbolKind.Function : vscode.SymbolKind.Variable;
          const range = document.lineAt(i).range;
          symbols.push(new vscode.DocumentSymbol(name, isFn ? "fn" : "bind", kind, range, range));
        }
        return symbols;
      }
    })
  );
}

function deactivate() {}

module.exports = {
  activate, deactivate,
  // Exposed for the offline unit test (test/lint.test.js).
  _internal: { maskNonCode, localNames, splitArgs, paramNames, lint, inlayHintsFor, KNOWN }
};
