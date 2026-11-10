# Writing the Ling docs

The docs are five mdBooks with **identical page paths**:

| Language | Source | Served at |
|---|---|---|
| English | `docs/src/` | `/docs/` |
| 中文 | `docs/i18n/zh/src/` | `/docs/zh/` |
| 日本語 | `docs/i18n/ja/src/` | `/docs/ja/` |
| 한국어 | `docs/i18n/ko/src/` | `/docs/ko/` |
| ไทย | `docs/i18n/th/src/` | `/docs/th/` |

Every page exists in all five languages. `SUMMARY.md` in each book is fixed
(see `tools/titles.json` for every page's title in each language). **Use exactly
that title as the page's `# H1`.** Don't edit SUMMARY.md, `lang.js`,
`custom.css` or `book.toml`.

Build: `python docs/tools/build.py`. Verify examples: `python docs/tools/l10n.py check <page.md>`.

## Sources of truth, in order

1. The runtime: `src/runtime/mod.rs` (builtin dispatch: search `"name" |`),
   `src/semantic/builtins.rs`, `lexicons/{en,zh,ja,ko,th}.ling` (keyword and
   builtin aliases). If the source and a document disagree, the source wins.
2. The Ling Book reference, verified against 2030.1.53:
   `../LingApps/LingBook/book/reference/*.md` (relative to `Programs/`) and its tutorials
   `../LingApps/LingBook/book/tutorials/*.md`. `90-known-limits.md` lists real gotchas.
3. The previous docs: `docs/_previous_src/` (partly outdated; mine for content, don't trust blindly).

Never invent a builtin, parameter or behaviour. If you can't confirm it, leave it out.

## Code examples

Fences (checked by `tools/l10n.py`):

- ` ```ling ` runs headless and prints. If followed by ` ```output `, the output must match exactly.
  Prefer this form: a short program plus its real output.
- ` ```ling-gui ` needs a window, audio, network or server. Parse-checked only.
- ` ```ling-module ` is a fragment without a `start` block. Parse-checked only.
- `sh`, `powershell`, `toml`, `rust`, `text`: not checked.

A runnable program has `bind start = do { ... }` (or the localized form). Keep examples short.
Every example must pass `l10n.py check`. Don't put output you didn't observe in an `output` block.

### Localized code: translations are full translations

In `zh/ja/ko/th` pages, code is written **in that language**: keywords, builtins,
identifiers, comments and string contents. For example, the Thai page's hello world prints
`"สวัสดี"`, and its variables have Thai names.

1. Write and verify the English example.
2. `python docs/tools/l10n.py localize ex.ling --to th > ex_th.ling` translates keywords and builtins
   (it runs `lingfu normalize` on a private copy, so never run `lingfu normalize` in a real folder: it rewrites every
   `.ling` file in the directory).
3. Rename identifiers, translate comments and strings by hand.
4. Re-run and paste the **new** observed output (translated strings change it). `l10n.py check` the page.

If a localized builtin alias doesn't exist (normalize leaves the English name), keep the English name.
Don't make one up. Only these five languages are in scope.

Exceptions that stay in the original language: the "Writing in Five Languages" page (shows all five side by
side), CLI commands, file names, crate names, and Rust code.

## Prose

- Plain, direct, friendly. Second person. Short sentences. No marketing fluff, no emoji.
- Lead with what the reader can do, then the details. Tables for builtin lists:
  `| Builtin | Purpose |` with the signature in the first column.
- Reference pages: an intro paragraph, a summary table, then one `###` entry per builtin or group
  (parameters, return value, a note when there is a gotcha, an example when useful).
- Link related pages with relative links (`../reference/web.md`). Links work identically in every language
  because paths are mirrored.
- Translations are natural, native-quality writing in that language, not word-for-word. Keep the same
  headings structure, tables, examples and links as the English page, so pages stay in sync.
- Thai: no spaces between words inside a phrase; use spaces between phrases/sentences, as in normal Thai.
  Japanese: です/ます style. Korean: 합니다 style. Chinese: simplified (简体).

## Terminology (use these consistently)

| English | 中文 | 日本語 | 한국어 | ไทย |
|---|---|---|---|---|
| binding / bind | 绑定 | 束縛 | 바인딩 | การผูก |
| function | 函数 | 関数 | 함수 | ฟังก์ชัน |
| builtin (function) | 内置函数 | 組み込み関数 | 내장 함수 | ฟังก์ชันในตัว |
| keyword | 关键字 | キーワード | 키워드 | คำสงวน |
| list | 列表 | リスト | 리스트 | รายการ |
| text / string | 文本 / 字符串 | テキスト / 文字列 | 텍스트 / 문자열 | ข้อความ / สตริง |
| record (`form`) | 记录（`form`） | レコード（`form`） | 레코드 (`form`) | เรคคอร์ด (`form`) |
| result (`ok`/`bad`) | 结果 | 結果 | 결과 | ผลลัพธ์ |
| interpreter | 解释器 | インタプリタ | 인터프리터 | อินเทอร์พรีเตอร์ |
| JIT | JIT | JIT | JIT | JIT |
| window | 窗口 | ウィンドウ | 창 | หน้าต่าง |
| camera | 摄像机 | カメラ | 카메라 | กล้อง |
| mesh | 网格 | メッシュ | 메시 | เมช |
| route / handler | 路由 / 处理函数 | ルート / ハンドラ | 라우트 / 핸들러 | เส้นทาง / ตัวจัดการ |
| crate | crate | クレート | 크레이트 | crate |
| profile (install) | 安装配置 | プロファイル | 프로필 | โปรไฟล์ |
| the Ling Book | Ling 之书 | Ling ブック | Ling 북 | หนังสือ Ling |
