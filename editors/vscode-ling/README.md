# Ling Language — VS Code extension

Editor support for the Ling (灵言) polyglot programming language (`.ling` files).

## Features

- **Syntax highlighting** — keywords, types, builtins, strings (with escapes), numbers, operators, and `#` / `//` comments. Recognizes both the canonical English keywords and the multilingual forms (中文 / 日本語 / 한국어 / ภาษาไทย / Русский / Español / …).
- **Autocomplete** — every keyword, type, constant, and the full builtin library (325 builtins) with category labels and call signatures. Builtins that take arguments expand to a call template.
- **Hover docs** — hover any keyword/type/builtin for a one-line description and signature.
- **Signature help** — parameter hints with the active argument highlighted while typing inside a builtin call.
- **Outline / breadcrumbs** — `bind` declarations appear in the Outline view and breadcrumbs, marked as functions or values.
- **Snippets** — `fn`, `start`, `if`, `ifelse`, `while`, `for`, `match`, `print`, `window`, and more.
- **Editor smarts** — bracket matching, auto-closing pairs, and comment toggling (`//`).
- **"Ling Dark" color theme** — a navy / teal / rose / grey / vine-green palette matching the compiler's colored terminal diagnostics. Select it via `Preferences: Color Theme → Ling Dark`.

## Install (local)

This extension ships as source — no build step is required (the `vscode` API is
provided by the host).

**Quick install:** copy or symlink this folder into your VS Code extensions dir:

```powershell
# Windows (PowerShell)
Set-Location C:\Users\User\Programs\ling\editors\vscode-ling
$dst = "$env:USERPROFILE\.vscode\extensions\ling-lang-0.1.0"
New-Item -ItemType Directory -Force -Path (Split-Path $dst)
New-Item -ItemType SymbolicLink -Path $dst -Target (Resolve-Path .)
```

```bash
# macOS / Linux
ln -s "$(pwd)" ~/.vscode/extensions/ling-lang-0.1.0
```

Then reload VS Code (`Developer: Reload Window`).

**Package as `.vsix`** (optional, for sharing):

```bash
npx @vscode/vsce package
code --install-extension ling-lang-0.1.0.vsix
```

## Develop

Open this folder in VS Code and press `F5` to launch an Extension Development
Host with the extension loaded. Open any `.ling` file to try it.

Highlighting lives in `syntaxes/ling.tmLanguage.json`; completion / hover /
signature help / outline live in `src/extension.js`, with the language data
(keywords, types, builtins, signatures) in `src/lingdata.js`.
