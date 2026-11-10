# gen_glossary.py — build the Ling glossary for all five docs books from the lexicons.
#
# Source of truth: lexicons/{en,zh,ja,ko,th}.ling  (key = canonical English name,
# value = surface form in that language). We read all five, merge by key, and emit
# Markdown tables with one column per language (header cells exactly
# `English | 中文 | 日本語 | 한국어 | ไทย`). docs/lang.js picks which column to show.
#
# A builtin is listed only if the runtime really dispatches it (its canonical name
# appears as a quoted string somewhere under src/). Lexicon entries with no runtime
# implementation are left out.
#
# Output, for each book (docs/src and docs/i18n/<lang>/src):
#   glossary/index.md  keywords.md  types.md  literals.md  builtins.md
# Prose and headings are in that book's language; titles come from docs/tools/titles.json.
# Old per-crate pages (glossary/crate-*.md) are removed.
#
# Run from the repo root:  python docs/gen_glossary.py

import glob, json, os, re, sys

ROOT  = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DOCS  = os.path.join(ROOT, "docs")
LEXD  = os.path.join(ROOT, "lexicons")
LANGS = ["en", "zh", "ja", "ko", "th"]
HEADERS = {"en": "English", "zh": "中文", "ja": "日本語", "ko": "한국어", "th": "ไทย"}

with open(os.path.join(DOCS, "tools", "titles.json"), encoding="utf-8") as f:
    TITLES = json.load(f)

def outdir(book):
    if book == "en":
        return os.path.join(DOCS, "src", "glossary")
    return os.path.join(DOCS, "i18n", book, "src", "glossary")

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
    """Walk en.ling tracking [section] so we keep the lexicon's order."""
    section = None
    rows = []  # (key, section)
    with open(os.path.join(LEXD, "en.ling"), encoding="utf-8") as f:
        for line in f:
            s = line.strip()
            sm = re.match(r'^\[(\w+)\]$', s)
            if sm:
                section = sm.group(1)
                continue
            m = KV.match(line)
            if m and section:
                rows.append((m.group(1), section))
    return rows

LEX = {c: parse_flat(c) for c in LANGS}

def surfaces(key):
    en = LEX["en"].get(key, key)
    return [LEX[c].get(key, en) for c in LANGS]

def runtime_source():
    text = []
    for p in glob.glob(os.path.join(ROOT, "src", "**", "*.rs"), recursive=True):
        with open(p, encoding="utf-8", errors="ignore") as f:
            text.append(f.read())
    return "\n".join(text)

# ── builtin → crate assignment (prefix rules + explicit overrides) ───────────
CORE = "ling"

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
    core = {"print","debug","panic","len","push","pop","new_list","new_map","keys","values",
            "contains","time","sleep","time_now","frame_count","trunc","int","sin","cos","tan",
            "sqrt","abs","floor","ceil","round","pow","log","min","max","clamp","pi","tau","lerp",
            "smoothstep","rand","sign","vnoise","fbm","perlin","hsv_to_rgb","lerp_color","hsl_color"}
    if name in core:
        return CORE
    # default: window, 2D/3D drawing, shapes, camera, lighting, input → graphics
    return "ling-graphics"

CRATE_ORDER = [CORE, "ling-graphics", "ling-ui", "ling-audio", "ling-music",
               "ling-physics", "ling-animation", "ling-ai", "ling-crypto", "ling-input",
               "ling-net", "ling-dialog"]

# ── localized text ───────────────────────────────────────────────────────────
# Each entry: {lang: text}. Keep the five in step when editing.

DESC_HEAD = {"en": "Description", "zh": "说明", "ja": "説明", "ko": "설명", "th": "คำอธิบาย"}

CORE_LABEL = {"en": "ling (core runtime)", "zh": "ling（核心运行时）", "ja": "ling（コアランタイム）",
              "ko": "ling (코어 런타임)", "th": "ling (รันไทม์หลัก)"}

CRATE_DESC = {
 CORE: {
  "en": "Output, math, noise, time, colour helpers and lists. Always available.",
  "zh": "输出、数学、噪声、时间、颜色辅助函数和列表。始终可用。",
  "ja": "出力、数学、ノイズ、時間、色の補助関数、リスト。常に使えます。",
  "ko": "출력, 수학, 노이즈, 시간, 색상 도우미, 리스트. 항상 사용할 수 있습니다.",
  "th": "การแสดงผล คณิตศาสตร์ นอยส์ เวลา ตัวช่วยเรื่องสี และรายการ ใช้ได้เสมอ"},
 "ling-graphics": {
  "en": "Window, 2D and 3D drawing, shapes, camera, lighting, cel shading, vector textures (`vtex_*`) and SVG export.",
  "zh": "窗口、2D 与 3D 绘图、形状、摄像机、光照、卡通着色、矢量纹理（`vtex_*`）和 SVG 导出。",
  "ja": "ウィンドウ、2D と 3D の描画、図形、カメラ、ライティング、セルシェーディング、ベクターテクスチャ（`vtex_*`）、SVG 書き出し。",
  "ko": "창, 2D와 3D 그리기, 도형, 카메라, 조명, 셀 셰이딩, 벡터 텍스처(`vtex_*`), SVG 내보내기.",
  "th": "หน้าต่าง การวาด 2 มิติและ 3 มิติ รูปทรง กล้อง แสง การแรเงาแบบเซล เท็กซ์เจอร์เวกเตอร์ (`vtex_*`) และการส่งออก SVG"},
 "ling-ui": {
  "en": "HUD, meters, controls, game UI widgets and vector fonts.",
  "zh": "HUD、仪表、控件、游戏 UI 组件和矢量字体。",
  "ja": "HUD、メーター、コントロール、ゲーム UI ウィジェット、ベクターフォント。",
  "ko": "HUD, 미터, 컨트롤, 게임 UI 위젯, 벡터 폰트.",
  "th": "HUD มาตรวัด ตัวควบคุม วิดเจ็ต UI สำหรับเกม และฟอนต์เวกเตอร์"},
 "ling-audio": {
  "en": "Tone synthesis, spatial sound, background music and microphone input.",
  "zh": "音调合成、空间音效、背景音乐和麦克风输入。",
  "ja": "トーン合成、空間オーディオ、BGM、マイク入力。",
  "ko": "톤 합성, 공간 사운드, 배경 음악, 마이크 입력.",
  "th": "การสังเคราะห์เสียงโทน เสียงเชิงพื้นที่ เพลงพื้นหลัง และอินพุตจากไมโครโฟน"},
 "ling-music": {
  "en": "Music decoding and analysis, GM-style synth, rhythm grading, karaoke and MIDI.",
  "zh": "音乐解码与分析、GM 风格合成器、节奏判定、卡拉 OK 和 MIDI。",
  "ja": "音楽のデコードと解析、GM 風シンセ、リズム判定、カラオケ、MIDI。",
  "ko": "음악 디코딩과 분석, GM 스타일 신시사이저, 리듬 판정, 노래방, MIDI.",
  "th": "การถอดรหัสและวิเคราะห์เพลง ซินธ์แบบ GM การให้คะแนนจังหวะ คาราโอเกะ และ MIDI"},
 "ling-physics": {
  "en": "Vector math, forces, soft bodies, rigid bodies and a liquid simulation.",
  "zh": "向量数学、力、软体、刚体和液体模拟。",
  "ja": "ベクトル演算、力、ソフトボディ、剛体、液体シミュレーション。",
  "ko": "벡터 수학, 힘, 소프트 바디, 강체, 액체 시뮬레이션.",
  "th": "คณิตศาสตร์เวกเตอร์ แรง วัตถุนิ่ม วัตถุแข็งเกร็ง และการจำลองของเหลว"},
 "ling-animation": {
  "en": "Procedural animation: tweens, gaits, springs, gears and IK.",
  "zh": "程序化动画：补间、步态、弹簧、齿轮和 IK。",
  "ja": "プロシージャルアニメーション：トゥイーン、歩行、ばね、歯車、IK。",
  "ko": "절차적 애니메이션: 트윈, 걸음걸이, 스프링, 기어, IK.",
  "th": "แอนิเมชันเชิงกระบวนการ: ทวีน ท่าเดิน สปริง เฟือง และ IK"},
 "ling-ai": {
  "en": "Neural networks, behaviour trees and a small learned dialog model.",
  "zh": "神经网络、行为树和一个可训练的小型对话模型。",
  "ja": "ニューラルネットワーク、ビヘイビアツリー、学習する小さな対話モデル。",
  "ko": "신경망, 행동 트리, 학습하는 작은 대화 모델.",
  "th": "โครงข่ายประสาทเทียม ต้นไม้พฤติกรรม และโมเดลบทสนทนาขนาดเล็กที่เรียนรู้ได้"},
 "ling-crypto": {
  "en": "Hashing, encryption, signatures and the geometric and post-quantum suite.",
  "zh": "哈希、加密、签名，以及几何与后量子密码套件。",
  "ja": "ハッシュ、暗号化、署名、幾何学的・耐量子暗号スイート。",
  "ko": "해시, 암호화, 서명, 기하학 및 양자 내성 암호 모음.",
  "th": "แฮช การเข้ารหัส ลายเซ็น และชุดวิทยาการเข้ารหัสเชิงเรขาคณิตและหลังควอนตัม"},
 "ling-input": {
  "en": "Gamepad polling, sticks, triggers and rumble.",
  "zh": "手柄轮询、摇杆、扳机键和震动。",
  "ja": "ゲームパッドの読み取り、スティック、トリガー、振動。",
  "ko": "게임패드 폴링, 스틱, 트리거, 진동.",
  "th": "การอ่านเกมแพด ก้านอนาล็อก ปุ่มทริกเกอร์ และการสั่น"},
 "ling-net": {
  "en": "HTTP client.",
  "zh": "HTTP 客户端。",
  "ja": "HTTP クライアント。",
  "ko": "HTTP 클라이언트.",
  "th": "ไคลเอนต์ HTTP"},
 "ling-dialog": {
  "en": "On-screen dialog boxes with typewriter text.",
  "zh": "带打字机效果的屏幕对话框。",
  "ja": "タイプライター風に文字が出る画面上のダイアログボックス。",
  "ko": "타자기처럼 글자가 나오는 화면 대화 상자.",
  "th": "กล่องบทสนทนาบนหน้าจอที่แสดงข้อความแบบพิมพ์ดีด"},
}

def crate_label(c, book):
    return CORE_LABEL[book] if c == CORE else c

# Keyword descriptions follow what the lexer and runtime really do
# (see the Ling Book reference, chapter 01-keywords).
NOEFFECT = {"en": "Reserved word; has no effect.", "zh": "保留字，没有作用。",
            "ja": "予約語です。効果はありません。", "ko": "예약어이며 아무 효과가 없습니다.",
            "th": "คำสงวนที่ไม่มีผลใด ๆ"}
OWNERSHIP = {"en": "Ownership marker. Parsed and ignored; nothing is checked.",
             "zh": "所有权标记。会被解析但被忽略，不做任何检查。",
             "ja": "所有権の印です。構文解析されますが無視され、何も検査されません。",
             "ko": "소유권 표시입니다. 파싱되지만 무시되며 아무것도 검사하지 않습니다.",
             "th": "เครื่องหมายความเป็นเจ้าของ ตัวแยกวิเคราะห์ยอมรับแต่ไม่สนใจ และไม่มีการตรวจสอบใด ๆ"}

KEYWORD_DESC = {
 "bind": {"en": "Bind a value or function to a name. Binding the same name again updates it.",
          "zh": "把值或函数绑定到一个名字。再次绑定同一个名字即可更新它。",
          "ja": "値や関数を名前に束縛します。同じ名前をもう一度束縛すると更新されます。",
          "ko": "값이나 함수를 이름에 바인딩합니다. 같은 이름을 다시 바인딩하면 값이 바뀝니다.",
          "th": "ผูกค่าหรือฟังก์ชันเข้ากับชื่อ การผูกชื่อเดิมอีกครั้งคือการอัปเดตค่า"},
 "do":   {"en": "Block expression `do { … }`. Its value is the last expression.",
          "zh": "代码块表达式 `do { … }`，其值是最后一个表达式。",
          "ja": "ブロック式 `do { … }`。値は最後の式です。",
          "ko": "블록 표현식 `do { … }`. 값은 마지막 표현식입니다.",
          "th": "นิพจน์บล็อก `do { … }` ค่าของมันคือนิพจน์สุดท้าย"},
 "fn":   {"en": "Declare a named function.", "zh": "声明一个具名函数。", "ja": "名前付き関数を宣言します。",
          "ko": "이름 있는 함수를 선언합니다.", "th": "ประกาศฟังก์ชันที่มีชื่อ"},
 "mod":  {"en": "Declare a module (a namespace of functions).", "zh": "声明模块（函数的命名空间）。",
          "ja": "モジュール（関数の名前空間）を宣言します。", "ko": "모듈(함수의 네임스페이스)을 선언합니다.",
          "th": "ประกาศโมดูล (เนมสเปซของฟังก์ชัน)"},
 "type": {"en": "Type alias. Parsed, no effect.", "zh": "类型别名。会被解析，但没有作用。",
          "ja": "型の別名。構文解析されますが効果はありません。", "ko": "타입 별칭. 파싱되지만 효과가 없습니다.",
          "th": "ชื่อแทนชนิดข้อมูล ตัวแยกวิเคราะห์ยอมรับแต่ไม่มีผล"},
 "if":   {"en": "Conditional branch.", "zh": "条件分支。", "ja": "条件分岐。", "ko": "조건 분기.", "th": "การแยกทางตามเงื่อนไข"},
 "else": {"en": "Alternative branch of `if`.", "zh": "`if` 的另一分支。", "ja": "`if` のもう一方の分岐。",
          "ko": "`if`의 다른 분기.", "th": "ทางเลือกอีกทางของ `if`"},
 "while": {"en": "Loop while a condition holds.", "zh": "条件成立时循环。", "ja": "条件が成り立つ間ループします。",
           "ko": "조건이 참인 동안 반복합니다.", "th": "วนซ้ำขณะที่เงื่อนไขเป็นจริง"},
 "for":  {"en": "Loop over a range, list or text.", "zh": "遍历范围、列表或文本。", "ja": "範囲、リスト、テキストを順にたどります。",
          "ko": "범위, 리스트, 텍스트를 순회합니다.", "th": "วนซ้ำตามช่วง รายการ หรือข้อความ"},
 "in":   {"en": "Separates the loop variable from the sequence: `for x in 0..10`.",
          "zh": "分隔循环变量和序列：`for x in 0..10`。", "ja": "ループ変数と列を区切ります：`for x in 0..10`。",
          "ko": "반복 변수와 시퀀스를 구분합니다: `for x in 0..10`.", "th": "คั่นตัวแปรวนซ้ำกับลำดับ: `for x in 0..10`"},
 "match": {"en": "Pattern matching.", "zh": "模式匹配。", "ja": "パターンマッチ。", "ko": "패턴 매칭.", "th": "การจับคู่รูปแบบ"},
 "return": {"en": "Leave the function with a value.", "zh": "带着一个值离开函数。", "ja": "値を返して関数を抜けます。",
            "ko": "값을 가지고 함수를 빠져나옵니다.", "th": "ออกจากฟังก์ชันพร้อมค่า"},
 "own": OWNERSHIP, "lend": OWNERSHIP, "share": OWNERSHIP, "move": OWNERSHIP, "copy": OWNERSHIP,
 "async": {"en": "Asynchronous marker. Parsed and ignored; code runs synchronously.",
           "zh": "异步标记。会被解析但被忽略，代码同步运行。", "ja": "非同期の印です。無視され、コードは同期的に実行されます。",
           "ko": "비동기 표시입니다. 무시되며 코드는 동기적으로 실행됩니다.", "th": "เครื่องหมายอะซิงโครนัส ถูกละเลย โค้ดทำงานแบบซิงโครนัส"},
 "wait": {"en": "Meant as \"await\". Unreliable; avoid it.", "zh": "本意是“等待”（await）。不可靠，请避免使用。",
          "ja": "「await」のつもりの語です。信頼できないので避けてください。", "ko": "\"await\"를 뜻하지만 신뢰할 수 없으니 피하세요.",
          "th": "ตั้งใจให้หมายถึง \"await\" แต่ไม่น่าเชื่อถือ ควรหลีกเลี่ยง"},
 "as":   {"en": "Names an imported module: `use \"file\" as m`.", "zh": "为导入的模块命名：`use \"file\" as m`。",
          "ja": "インポートしたモジュールに名前を付けます：`use \"file\" as m`。", "ko": "가져온 모듈에 이름을 붙입니다: `use \"file\" as m`.",
          "th": "ตั้งชื่อให้โมดูลที่นำเข้า: `use \"file\" as m`"},
 "where": {"en": "Constraint clause after a function signature. Skipped, no effect.",
           "zh": "函数签名后的约束子句。会被跳过，没有作用。", "ja": "関数シグネチャの後の制約節。読み飛ばされ、効果はありません。",
           "ko": "함수 시그니처 뒤의 제약 절. 건너뛰며 효과가 없습니다.", "th": "ส่วนเงื่อนไขหลังลายเซ็นฟังก์ชัน ถูกข้ามไปและไม่มีผล"},
 "post": NOEFFECT, "give": NOEFFECT, "fit": NOEFFECT, "can": NOEFFECT, "change": NOEFFECT,
 "sure": NOEFFECT, "maybe": NOEFFECT, "pure": NOEFFECT,
 "form": {"en": "Declare a record type.", "zh": "声明记录类型。", "ja": "レコード型を宣言します。",
          "ko": "레코드 타입을 선언합니다.", "th": "ประกาศชนิดเรคคอร์ด"},
 "choose": {"en": "Declare an enumeration with variants.", "zh": "声明带变体的枚举。", "ja": "バリアントを持つ列挙型を宣言します。",
            "ko": "변형을 가진 열거형을 선언합니다.", "th": "ประกาศชนิดแจงนับที่มีหลายแบบ"},
 "stop": {"en": "Reserved word; has no effect. Ling has no `break`.", "zh": "保留字，没有作用。Ling 没有 `break`。",
          "ja": "予約語で効果はありません。Ling に `break` はありません。", "ko": "예약어이며 효과가 없습니다. Ling에는 `break`가 없습니다.",
          "th": "คำสงวนที่ไม่มีผล Ling ไม่มี `break`"},
 "again": {"en": "Reserved word; has no effect. Ling has no `continue`.", "zh": "保留字，没有作用。Ling 没有 `continue`。",
           "ja": "予約語で効果はありません。Ling に `continue` はありません。", "ko": "예약어이며 효과가 없습니다. Ling에는 `continue`가 없습니다.",
           "th": "คำสงวนที่ไม่มีผล Ling ไม่มี `continue`"},
 "try":  {"en": "Not syntax: the builtin `try(closure)` turns a runtime error into `bad(...)`.",
          "zh": "不是语法：内置函数 `try(closure)` 把运行时错误变成 `bad(...)`。",
          "ja": "構文ではありません。組み込み関数 `try(closure)` が実行時エラーを `bad(...)` に変えます。",
          "ko": "문법이 아닙니다. 내장 함수 `try(closure)`가 런타임 오류를 `bad(...)`로 바꿉니다.",
          "th": "ไม่ใช่ไวยากรณ์ ฟังก์ชันในตัว `try(closure)` แปลงข้อผิดพลาดขณะรันเป็น `bad(...)`"},
 "spawn": {"en": "Reserved word; has no effect. Use `thread_spawn` for threads.", "zh": "保留字，没有作用。线程请用 `thread_spawn`。",
           "ja": "予約語で効果はありません。スレッドには `thread_spawn` を使います。", "ko": "예약어이며 효과가 없습니다. 스레드는 `thread_spawn`을 쓰세요.",
           "th": "คำสงวนที่ไม่มีผล ถ้าต้องการเธรดให้ใช้ `thread_spawn`"},
 "ok":   {"en": "Success result `ok(value)`; also a `match` pattern.", "zh": "成功结果 `ok(value)`，也可用作 `match` 模式。",
          "ja": "成功の結果 `ok(value)`。`match` のパターンにもなります。", "ko": "성공 결과 `ok(value)`. `match` 패턴으로도 씁니다.",
          "th": "ผลลัพธ์สำเร็จ `ok(value)` ใช้เป็นรูปแบบใน `match` ได้ด้วย"},
 "bad":  {"en": "Failure result `bad(reason)`; also a `match` pattern.", "zh": "失败结果 `bad(reason)`，也可用作 `match` 模式。",
          "ja": "失敗の結果 `bad(reason)`。`match` のパターンにもなります。", "ko": "실패 결과 `bad(reason)`. `match` 패턴으로도 씁니다.",
          "th": "ผลลัพธ์ล้มเหลว `bad(reason)` ใช้เป็นรูปแบบใน `match` ได้ด้วย"},
 "none": {"en": "Not defined. Use `()` for \"nothing\".", "zh": "未定义。表示“无”请用 `()`。",
          "ja": "定義されていません。「何もない」は `()` を使います。", "ko": "정의되어 있지 않습니다. \"없음\"은 `()`를 쓰세요.",
          "th": "ไม่ได้นิยามไว้ ถ้าต้องการค่า \"ว่าง\" ให้ใช้ `()`"},
 "start": {"en": "Usual name of the entry point: `bind start = do { … }`. A name, not a keyword.",
           "zh": "入口点的惯用名字：`bind start = do { … }`。它是名字，不是关键字。",
           "ja": "エントリーポイントの慣例的な名前：`bind start = do { … }`。キーワードではなく名前です。",
           "ko": "진입점의 관례적인 이름: `bind start = do { … }`. 키워드가 아니라 이름입니다.",
           "th": "ชื่อที่ใช้กันสำหรับจุดเริ่มต้นโปรแกรม: `bind start = do { … }` เป็นชื่อ ไม่ใช่คำสงวน"},
 "result": {"en": "Listed in the lexicon only; the lexer treats it as a name.", "zh": "只出现在词库中，词法分析器把它当作普通名字。",
            "ja": "語彙ファイルにだけ載っています。字句解析器はただの名前として扱います。",
            "ko": "어휘 파일에만 있습니다. 렉서는 일반 이름으로 취급합니다.",
            "th": "มีอยู่ในไฟล์คำศัพท์เท่านั้น ตัวแยกคำถือว่าเป็นชื่อธรรมดา"},
}

DOCONLY = {"en": "No such type exists; the name is documentation only.", "zh": "不存在这种类型，这个名字仅用于文档。",
           "ja": "この型は存在しません。名前は説明用だけです。", "ko": "이런 타입은 없으며 이름은 문서용입니다.",
           "th": "ไม่มีชนิดนี้จริง ชื่อนี้ใช้ในเอกสารเท่านั้น"}
TYPE_DESC = {
 "number": {"en": "Number. Every number is a 64-bit float.", "zh": "数字。所有数字都是 64 位浮点数。",
            "ja": "数値。数値はすべて 64 ビット浮動小数点数です。", "ko": "숫자. 모든 숫자는 64비트 부동소수점입니다.",
            "th": "ตัวเลข ตัวเลขทุกตัวเป็นทศนิยมลอยตัว 64 บิต"},
 "text": {"en": "Text (string).", "zh": "文本（字符串）。", "ja": "テキスト（文字列）。", "ko": "텍스트(문자열).", "th": "ข้อความ (สตริง)"},
 "bool": {"en": "Boolean: `true` or `false`.", "zh": "布尔值：`true` 或 `false`。", "ja": "真偽値：`true` または `false`。",
          "ko": "불리언: `true` 또는 `false`.", "th": "ค่าความจริง: `true` หรือ `false`"},
 "list": {"en": "List; mixed types, indexed from 0.", "zh": "列表；可混合类型，从 0 开始编号。", "ja": "リスト。型を混ぜられ、0 から数えます。",
          "ko": "리스트. 타입을 섞을 수 있고 0부터 셉니다.", "th": "รายการ ใส่ค่าต่างชนิดกันได้ เริ่มนับจาก 0"},
 "map": DOCONLY, "tuple": DOCONLY,
}

def t(d, book):
    return d.get(book) or d["en"]

# ── page text ────────────────────────────────────────────────────────────────

INDEX_TEXT = {
 "en": """This glossary lists Ling's keywords, types, literals and builtins with their spelling in
English, 中文, 日本語, 한국어 and ไทย. In the tables, the column for your language is shown next to
English. The pages are generated from the lexicon files (`lexicons/*.ling`) by `docs/gen_glossary.py`.

- **{kw}** keywords · **{ty}** types · **{bi}** builtins in **{cr}** crates

| Page | Contents |
|---|---|
| [Keywords](keywords.md) | Reserved words and what each one really does |
| [Types](types.md) | The type names |
| [Literals and Operators](literals.md) | How to write values, and the operators |
| [Builtins by Crate](builtins.md) | Builtin names, grouped by the crate they come from |

The lexer accepts a few extra spellings that are not in the lexicon files. For the full
story of each keyword, see [Keywords](../language/keywords.md) in the Language Guide. The
glossary lists only builtins that have names in the lexicon files; the
[Builtins Reference](../reference/index.md) covers many more.
""",
 "zh": """本词汇表列出 Ling 的关键字、类型、字面量和内置函数，以及它们在 English、中文、日本語、한국어
和 ไทย 中的写法。表格中，你所选语言的那一列显示在英文旁边。这些页面由 `docs/gen_glossary.py`
根据词库文件（`lexicons/*.ling`）生成。

- **{kw}** 个关键字 · **{ty}** 个类型 · **{bi}** 个内置函数，分属 **{cr}** 个 crate

| 页面 | 内容 |
|---|---|
| [关键字](keywords.md) | 保留字以及每个保留字的实际作用 |
| [类型](types.md) | 类型名称 |
| [字面量与运算符](literals.md) | 值的写法和运算符 |
| [按 Crate 分类的内置函数](builtins.md) | 按所属 crate 分组的内置函数名称 |

词法分析器还接受少量不在词库文件中的写法。每个关键字的完整说明见语言指南中的
[关键字](../language/keywords.md)。本词汇表只列出在词库文件中有名称的内置函数；
[内置函数参考](../reference/index.md)涵盖的更多。
""",
 "ja": """この用語集では、Ling のキーワード、型、リテラル、組み込み関数と、それぞれの English、中文、
日本語、한국어、ไทย での書き方を一覧にしています。表では、選んだ言語の列が英語の隣に表示されます。
これらのページは `docs/gen_glossary.py` が語彙ファイル（`lexicons/*.ling`）から生成しています。

- キーワード **{kw}** 個 · 型 **{ty}** 個 · 組み込み関数 **{bi}** 個（**{cr}** クレート）

| ページ | 内容 |
|---|---|
| [キーワード](keywords.md) | 予約語と、それぞれの実際の働き |
| [型](types.md) | 型の名前 |
| [リテラルと演算子](literals.md) | 値の書き方と演算子 |
| [クレート別組み込み関数](builtins.md) | 提供元のクレートごとに分けた組み込み関数の名前 |

字句解析器は、語彙ファイルにない書き方もいくつか受け付けます。各キーワードの詳しい説明は、
言語ガイドの[キーワード](../language/keywords.md)を見てください。この用語集には語彙ファイルに
名前がある組み込み関数だけを載せています。[組み込み関数リファレンス](../reference/index.md)には
さらに多くの関数があります。
""",
 "ko": """이 용어집은 Ling의 키워드, 타입, 리터럴, 내장 함수와 각각의 English, 中文, 日本語, 한국어, ไทย
표기를 정리합니다. 표에서는 선택한 언어의 열이 영어 옆에 표시됩니다. 이 페이지들은
`docs/gen_glossary.py`가 어휘 파일(`lexicons/*.ling`)에서 생성합니다.

- 키워드 **{kw}**개 · 타입 **{ty}**개 · 내장 함수 **{bi}**개(크레이트 **{cr}**개)

| 페이지 | 내용 |
|---|---|
| [키워드](keywords.md) | 예약어와 각 예약어가 실제로 하는 일 |
| [타입](types.md) | 타입 이름 |
| [리터럴과 연산자](literals.md) | 값을 쓰는 방법과 연산자 |
| [크레이트별 내장 함수](builtins.md) | 제공하는 크레이트별로 묶은 내장 함수 이름 |

렉서는 어휘 파일에 없는 표기도 몇 가지 받아들입니다. 각 키워드의 자세한 내용은 언어 가이드의
[키워드](../language/keywords.md)를 보세요. 이 용어집에는 어휘 파일에 이름이 있는 내장 함수만
있습니다. [내장 함수 레퍼런스](../reference/index.md)에는 훨씬 더 많은 함수가 있습니다.
""",
 "th": """อภิธานศัพท์นี้รวบรวมคำสงวน ชนิดข้อมูล ค่าคงที่ และฟังก์ชันในตัวของ Ling พร้อมวิธีสะกดใน English
中文 日本語 한국어 และ ไทย ในตาราง คอลัมน์ภาษาที่คุณเลือกจะแสดงอยู่ข้างภาษาอังกฤษ หน้าเหล่านี้สร้างโดย
`docs/gen_glossary.py` จากไฟล์คำศัพท์ (`lexicons/*.ling`)

- คำสงวน **{kw}** คำ · ชนิดข้อมูล **{ty}** ชนิด · ฟังก์ชันในตัว **{bi}** ฟังก์ชัน ใน **{cr}** crate

| หน้า | เนื้อหา |
|---|---|
| [คำสงวน](keywords.md) | คำสงวนและสิ่งที่แต่ละคำทำจริง |
| [ชนิดข้อมูล](types.md) | ชื่อชนิดข้อมูล |
| [ค่าคงที่และตัวดำเนินการ](literals.md) | วิธีเขียนค่าต่าง ๆ และตัวดำเนินการ |
| [ฟังก์ชันในตัวตาม Crate](builtins.md) | ชื่อฟังก์ชันในตัว จัดกลุ่มตาม crate ที่มา |

ตัวแยกคำยอมรับการสะกดบางแบบที่ไม่มีในไฟล์คำศัพท์ด้วย ดูรายละเอียดของแต่ละคำได้ที่
[คำสงวน](../language/keywords.md) ในคู่มือภาษา อภิธานศัพท์นี้แสดงเฉพาะฟังก์ชันในตัวที่มีชื่ออยู่ในไฟล์คำศัพท์
ส่วน[ข้อมูลอ้างอิงฟังก์ชันในตัว](../reference/index.md)ครอบคลุมมากกว่านี้
""",
}

KEYWORDS_INTRO = {
 "en": "Reserved words and their spelling in each language. Some are reserved but do nothing yet; the description says so. See [Keywords](../language/keywords.md) for examples.",
 "zh": "保留字及其在各语言中的写法。有些词虽是保留字，但目前没有作用，说明中会注明。示例见[关键字](../language/keywords.md)。",
 "ja": "予約語と、各言語での書き方です。予約されているだけでまだ何もしない語もあり、説明にそう書いてあります。例は[キーワード](../language/keywords.md)を見てください。",
 "ko": "예약어와 각 언어의 표기입니다. 예약만 되어 있고 아직 아무 일도 하지 않는 단어도 있으며, 설명에 그렇게 적혀 있습니다. 예제는 [키워드](../language/keywords.md)를 보세요.",
 "th": "คำสงวนและวิธีสะกดในแต่ละภาษา บางคำสงวนไว้แต่ยังไม่มีผลใด ๆ ซึ่งคำอธิบายจะบอกไว้ ดูตัวอย่างได้ที่[คำสงวน](../language/keywords.md)",
}

TYPES_INTRO = {
 "en": "Ling is dynamically typed. These type names can appear in annotations such as `x: number` or `-> text`, but annotations are parsed and ignored. See [Types and Values](../language/types.md).",
 "zh": "Ling 是动态类型语言。这些类型名可以出现在 `x: number` 或 `-> text` 这样的标注中，但标注只会被解析，然后被忽略。详见[类型与值](../language/types.md)。",
 "ja": "Ling は動的型付けの言語です。これらの型名は `x: number` や `-> text` のような注釈に書けますが、注釈は構文解析されたあと無視されます。[型と値](../language/types.md)も見てください。",
 "ko": "Ling은 동적 타입 언어입니다. 이 타입 이름은 `x: number`나 `-> text` 같은 표기에 쓸 수 있지만, 표기는 파싱된 뒤 무시됩니다. [타입과 값](../language/types.md)을 보세요.",
 "th": "Ling เป็นภาษาที่กำหนดชนิดข้อมูลแบบไดนามิก ชื่อชนิดเหล่านี้เขียนในคำกำกับอย่าง `x: number` หรือ `-> text` ได้ แต่คำกำกับจะถูกแยกวิเคราะห์แล้วละเลยไป ดู[ชนิดข้อมูลและค่า](../language/types.md)",
}

BUILTINS_INTRO = {
 "en": "Builtin names in each language, grouped by the crate that provides them. Only builtins that the runtime really provides and that have names in the lexicon files are listed. For parameters and examples, see the [Builtins Reference](../reference/index.md).",
 "zh": "各语言中的内置函数名称，按提供它们的 crate 分组。这里只列出运行时真正提供、并且在词库文件中有名称的内置函数。参数和示例见[内置函数参考](../reference/index.md)。",
 "ja": "各言語での組み込み関数の名前を、提供元のクレートごとにまとめています。ランタイムが実際に提供し、語彙ファイルに名前がある組み込み関数だけを載せています。引数や例は[組み込み関数リファレンス](../reference/index.md)を見てください。",
 "ko": "각 언어의 내장 함수 이름을 제공하는 크레이트별로 묶었습니다. 런타임이 실제로 제공하고 어휘 파일에 이름이 있는 내장 함수만 실었습니다. 매개변수와 예제는 [내장 함수 레퍼런스](../reference/index.md)를 보세요.",
 "th": "ชื่อฟังก์ชันในตัวในแต่ละภาษา จัดกลุ่มตาม crate ที่ให้ฟังก์ชันนั้น แสดงเฉพาะฟังก์ชันในตัวที่รันไทม์มีอยู่จริงและมีชื่ออยู่ในไฟล์คำศัพท์ ดูพารามิเตอร์และตัวอย่างได้ที่[ข้อมูลอ้างอิงฟังก์ชันในตัว](../reference/index.md)",
}

LANGROW = "| " + " | ".join(HEADERS[c] for c in LANGS) + " |\n|" + "|".join(["---"] * len(LANGS)) + "|\n"
BOOL_TABLE = LANGROW + "| `true` | `真` | `真` | `참` | `จริง` |\n| `false` | `假` | `偽` | `거짓` | `เท็จ` |\n"

LITERALS = {
 "en": """Literals and operators are written the same way in every language, except `true` and
`false`, which have their own spellings.

## Literals

| Literal | Meaning |
|---|---|
| `42`, `3.14` | Number. Every number is a 64-bit float. There is no hex or exponent form. |
| `"hello"` | Text. Double quotes only. Escapes: `\\n` `\\t` `\\r` `\\"` `\\\\` `\\0`. No interpolation. |
| `true`, `false` | Boolean |
| `()` | Unit, the "nothing" value |
| `[1, "two", 3.0]` | List; mixed types, indexed from 0 |
| `0..5` | Range, half-open: 0, 1, 2, 3, 4. Used with `for`. |
| `\\|x\\| x + 1` | Function value (closure) |
| `ok(v)`, `bad(e)` | Result values |

There are no character, map or tuple literals.

## Booleans in each language

{bools}
## Operators

From the tightest binding to the loosest:

| Operator | Meaning |
|---|---|
| `.` `()` `[]` `::` `..` | Field or method, call, index, path, range |
| `-x` `!x` | Negate, logical not |
| `*` `/` `%` | Multiply, divide, remainder |
| `+` `-` | Add, subtract. `+` also joins text. |
| `==` `!=` `<` `>` `<=` `>=` | Compare |
| `&&` | Logical and |
| `\\|\\|` | Logical or |

Other symbols: `=` in `bind name = value`, `=>` between a pattern and its result in `match`,
and `->` before a return type (parsed and ignored). In the interpreter, `&&` and `||` always
evaluate both sides. See [Known Limits](../language/limits.md).
""",
 "zh": """字面量和运算符在所有语言中写法都相同，只有 `true` 和 `false` 有各自语言的写法。

## 字面量

| 字面量 | 含义 |
|---|---|
| `42`、`3.14` | 数字。所有数字都是 64 位浮点数。没有十六进制或指数写法。 |
| `"hello"` | 文本。只能用双引号。转义：`\\n` `\\t` `\\r` `\\"` `\\\\` `\\0`。不支持插值。 |
| `true`、`false` | 布尔值 |
| `()` | 单元值，表示“无” |
| `[1, "two", 3.0]` | 列表；可混合类型，从 0 开始编号 |
| `0..5` | 范围，左闭右开：0、1、2、3、4。与 `for` 一起使用。 |
| `\\|x\\| x + 1` | 函数值（闭包） |
| `ok(v)`、`bad(e)` | 结果值 |

没有字符、映射或元组字面量。

## 各语言的布尔值

{bools}
## 运算符

按结合从紧到松排列：

| 运算符 | 含义 |
|---|---|
| `.` `()` `[]` `::` `..` | 字段或方法、调用、索引、路径、范围 |
| `-x` `!x` | 取负、逻辑非 |
| `*` `/` `%` | 乘、除、取余 |
| `+` `-` | 加、减。`+` 也能拼接文本。 |
| `==` `!=` `<` `>` `<=` `>=` | 比较 |
| `&&` | 逻辑与 |
| `\\|\\|` | 逻辑或 |

其他符号：`bind name = value` 中的 `=`，`match` 中模式与结果之间的 `=>`，以及返回类型前的 `->`
（会被解析但被忽略）。在解释器中，`&&` 和 `||` 总会计算两边。详见[已知限制](../language/limits.md)。
""",
 "ja": """リテラルと演算子はどの言語でも同じ書き方です。ただし `true` と `false` には各言語の書き方があります。

## リテラル

| リテラル | 意味 |
|---|---|
| `42`、`3.14` | 数値。数値はすべて 64 ビット浮動小数点数です。16 進数や指数の書き方はありません。 |
| `"hello"` | テキスト。二重引用符だけを使います。エスケープ：`\\n` `\\t` `\\r` `\\"` `\\\\` `\\0`。埋め込み（補間）はありません。 |
| `true`、`false` | 真偽値 |
| `()` | ユニット。「何もない」値です |
| `[1, "two", 3.0]` | リスト。型を混ぜられ、0 から数えます |
| `0..5` | 範囲。終わりを含みません：0、1、2、3、4。`for` で使います。 |
| `\\|x\\| x + 1` | 関数値（クロージャ） |
| `ok(v)`、`bad(e)` | 結果の値 |

文字、マップ、タプルのリテラルはありません。

## 各言語の真偽値

{bools}
## 演算子

結合の強いものから順に並べています。

| 演算子 | 意味 |
|---|---|
| `.` `()` `[]` `::` `..` | フィールドやメソッド、呼び出し、添字、パス、範囲 |
| `-x` `!x` | 符号反転、論理否定 |
| `*` `/` `%` | 乗算、除算、剰余 |
| `+` `-` | 加算、減算。`+` はテキストの連結にも使えます。 |
| `==` `!=` `<` `>` `<=` `>=` | 比較 |
| `&&` | 論理積 |
| `\\|\\|` | 論理和 |

そのほかの記号：`bind name = value` の `=`、`match` でパターンと結果の間に書く `=>`、戻り値の型の前に書く
`->`（構文解析されますが無視されます）。インタプリタでは `&&` と `||` は常に両辺を評価します。
[既知の制限](../language/limits.md)も見てください。
""",
 "ko": """리터럴과 연산자는 모든 언어에서 똑같이 씁니다. 단, `true`와 `false`는 언어마다 표기가 있습니다.

## 리터럴

| 리터럴 | 의미 |
|---|---|
| `42`, `3.14` | 숫자. 모든 숫자는 64비트 부동소수점입니다. 16진수나 지수 표기는 없습니다. |
| `"hello"` | 텍스트. 큰따옴표만 씁니다. 이스케이프: `\\n` `\\t` `\\r` `\\"` `\\\\` `\\0`. 보간은 없습니다. |
| `true`, `false` | 불리언 |
| `()` | 유닛, "없음"을 뜻하는 값 |
| `[1, "two", 3.0]` | 리스트. 타입을 섞을 수 있고 0부터 셉니다 |
| `0..5` | 범위, 끝은 포함하지 않음: 0, 1, 2, 3, 4. `for`와 함께 씁니다. |
| `\\|x\\| x + 1` | 함수 값(클로저) |
| `ok(v)`, `bad(e)` | 결과 값 |

문자, 맵, 튜플 리터럴은 없습니다.

## 언어별 불리언

{bools}
## 연산자

결합이 강한 것부터 약한 순서입니다.

| 연산자 | 의미 |
|---|---|
| `.` `()` `[]` `::` `..` | 필드나 메서드, 호출, 인덱스, 경로, 범위 |
| `-x` `!x` | 부호 반전, 논리 부정 |
| `*` `/` `%` | 곱하기, 나누기, 나머지 |
| `+` `-` | 더하기, 빼기. `+`는 텍스트도 이어 붙입니다. |
| `==` `!=` `<` `>` `<=` `>=` | 비교 |
| `&&` | 논리 AND |
| `\\|\\|` | 논리 OR |

그 밖의 기호: `bind name = value`의 `=`, `match`에서 패턴과 결과 사이의 `=>`, 반환 타입 앞의 `->`
(파싱되지만 무시됩니다). 인터프리터에서 `&&`와 `||`는 항상 양쪽을 모두 계산합니다.
[알려진 제한 사항](../language/limits.md)을 보세요.
""",
 "th": """ค่าคงที่และตัวดำเนินการเขียนเหมือนกันทุกภาษา ยกเว้น `true` และ `false` ซึ่งมีคำสะกดของแต่ละภาษา

## ค่าคงที่

| ค่าคงที่ | ความหมาย |
|---|---|
| `42`, `3.14` | ตัวเลข ตัวเลขทุกตัวเป็นทศนิยมลอยตัว 64 บิต ไม่มีรูปแบบเลขฐานสิบหกหรือเลขยกกำลัง |
| `"hello"` | ข้อความ ใช้อัญประกาศคู่เท่านั้น อักขระหลีก: `\\n` `\\t` `\\r` `\\"` `\\\\` `\\0` ไม่มีการแทรกค่าในข้อความ |
| `true`, `false` | ค่าความจริง |
| `()` | ค่ายูนิต หมายถึง "ไม่มีอะไร" |
| `[1, "two", 3.0]` | รายการ ใส่ค่าต่างชนิดกันได้ เริ่มนับจาก 0 |
| `0..5` | ช่วง ไม่รวมค่าสุดท้าย: 0, 1, 2, 3, 4 ใช้กับ `for` |
| `\\|x\\| x + 1` | ค่าฟังก์ชัน (โคลเชอร์) |
| `ok(v)`, `bad(e)` | ค่าผลลัพธ์ |

ไม่มีค่าคงที่แบบอักขระ แมป หรือทูเพิล

## ค่าความจริงในแต่ละภาษา

{bools}
## ตัวดำเนินการ

เรียงจากที่ผูกแน่นที่สุดไปหาหลวมที่สุด:

| ตัวดำเนินการ | ความหมาย |
|---|---|
| `.` `()` `[]` `::` `..` | ฟิลด์หรือเมธอด การเรียก ดัชนี พาธ ช่วง |
| `-x` `!x` | กลับเครื่องหมาย นิเสธทางตรรกะ |
| `*` `/` `%` | คูณ หาร เศษจากการหาร |
| `+` `-` | บวก ลบ `+` ใช้ต่อข้อความได้ด้วย |
| `==` `!=` `<` `>` `<=` `>=` | เปรียบเทียบ |
| `&&` | และ (ตรรกะ) |
| `\\|\\|` | หรือ (ตรรกะ) |

สัญลักษณ์อื่น: `=` ใน `bind name = value`, `=>` ระหว่างรูปแบบกับผลลัพธ์ใน `match` และ `->` หน้าชนิดค่าที่คืน
(แยกวิเคราะห์แล้วละเลย) ในอินเทอร์พรีเตอร์ `&&` และ `||` จะคำนวณทั้งสองข้างเสมอ ดู[ข้อจำกัดที่ทราบ](../language/limits.md)
""",
}

# ── table helpers ────────────────────────────────────────────────────────────

def esc(s):
    return s.replace("|", "\\|")

def cells(key):
    return " | ".join(f"`{esc(v)}`" for v in surfaces(key))

def described_table(keys, descmap, book):
    cols = " | ".join(HEADERS[c] for c in LANGS)
    sep = "|".join(["---"] * (len(LANGS) + 1))
    out = f"| {cols} | {DESC_HEAD[book]} |\n|{sep}|\n"
    for key in keys:
        d = descmap.get(key)
        out += f"| {cells(key)} | {t(d, book) if d else ''} |\n"
    return out

def names_table(keys):
    out = LANGROW
    for key in keys:
        out += f"| {cells(key)} |\n"
    return out

# ── build ────────────────────────────────────────────────────────────────────

def main():
    structured = parse_en_structured()
    kw = [k for (k, s) in structured if s == "keywords"]
    ty = [k for (k, s) in structured if s == "types"]
    src = runtime_source()
    crates, seen, skipped = {}, set(), []
    for key, s in structured:
        if s != "builtins" or key in seen:
            continue
        seen.add(key)
        if f'"{key}"' not in src:
            skipped.append(key)
            continue
        crates.setdefault(crate_of(key), []).append(key)
    total = sum(len(v) for v in crates.values())
    order = [c for c in CRATE_ORDER if c in crates] + sorted(c for c in crates if c not in CRATE_ORDER)

    written = []
    for book in LANGS:
        d = outdir(book)
        os.makedirs(d, exist_ok=True)
        for stale in glob.glob(os.path.join(d, "crate-*.md")):
            os.remove(stale)

        def write(name, body):
            title = TITLES[f"glossary/{name}"][book]
            p = os.path.join(d, name)
            with open(p, "w", encoding="utf-8", newline="\n") as f:
                f.write(f"# {title}\n\n{body.rstrip()}\n")
            written.append(p)

        write("index.md", INDEX_TEXT[book].format(kw=len(kw), ty=len(ty), bi=total, cr=len(crates)))
        write("keywords.md", KEYWORDS_INTRO[book] + "\n\n" + described_table(kw, KEYWORD_DESC, book))
        write("types.md", TYPES_INTRO[book] + "\n\n" + described_table(ty, TYPE_DESC, book))
        write("literals.md", LITERALS[book].replace("{bools}", BOOL_TABLE))
        body = [BUILTINS_INTRO[book], "\n"]
        for c in order:
            desc = t(CRATE_DESC.get(c, {"en": ""}), book)
            body.append(f"\n## {crate_label(c, book)}\n\n{desc}\n\n{names_table(crates[c])}")
        write("builtins.md", "".join(body))

    print(f"wrote {len(written)} files: {len(kw)} keywords, {len(ty)} types, "
          f"{total} builtins in {len(crates)} crates")
    if skipped:
        print(f"skipped {len(skipped)} lexicon builtins with no runtime implementation: "
              + ", ".join(skipped))

if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    main()
