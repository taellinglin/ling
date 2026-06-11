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

function activate(context) {
  const sel = { language: LANG, scheme: "file" };
  const completions = buildCompletions();

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

module.exports = { activate, deactivate };
