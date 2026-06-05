// Ling docs — 5-language page selector.
// Injects a language dropdown into the mdBook menu bar and re-skins the whole
// page (title, navigation, headings and key phrases) into the chosen language,
// persisted in localStorage. Languages: en · zh · ja · ko · th.
(function () {
  var LANGS = [["en", "EN"], ["zh", "中文"], ["ja", "日本語"], ["ko", "한국어"], ["th", "ไทย"]];

  // English → [zh, ja, ko, th]
  var DICT = {
    "Ling Language Reference": ["灵 语言参考", "灵 言語リファレンス", "灵 언어 레퍼런스", "灵 อ้างอิงภาษา"],
    "Introduction": ["简介", "はじめに", "소개", "บทนำ"],
    "Showcase & Comparison": ["展示与对比", "ショーケースと比較", "쇼케이스 & 비교", "โชว์เคสและเปรียบเทียบ"],
    "Getting Started": ["入门", "はじめる", "시작하기", "เริ่มต้นใช้งาน"],
    "Installation": ["安装", "インストール", "설치", "การติดตั้ง"],
    "Hello World": ["你好世界", "ハローワールド", "안녕 세계", "สวัสดีชาวโลก"],
    "Your First Room": ["你的第一个房间", "最初の部屋", "첫 번째 방", "ห้องแรกของคุณ"],
    "Language Reference": ["语言参考", "言語リファレンス", "언어 레퍼런스", "อ้างอิงภาษา"],
    "Syntax": ["语法", "構文", "문법", "ไวยากรณ์"],
    "Builtins — Core": ["内置函数 — 核心", "組み込み — コア", "내장 — 코어", "ฟังก์ชันในตัว — แกน"],
    "Builtins — 3-D Primitive Shapes": ["内置 — 3D 基本形状", "組み込み — 3D基本形状", "내장 — 3D 기본 도형", "ในตัว — รูปทรง 3 มิติ"],
    "Builtins — Vector Geometry (vtex)": ["内置 — 矢量几何 (vtex)", "組み込み — ベクタ幾何 (vtex)", "내장 — 벡터 지오메트리 (vtex)", "ในตัว — เรขาคณิตเวกเตอร์ (vtex)"],
    "Builtins — Pixel Textures (tex)": ["内置 — 像素纹理 (tex)", "組み込み — ピクセルテクスチャ (tex)", "내장 — 픽셀 텍스처 (tex)", "ในตัว — เท็กซ์เจอร์พิกเซล (tex)"],
    "Builtins — Audio": ["内置 — 音频", "組み込み — オーディオ", "내장 — 오디오", "ในตัว — เสียง"],
    "Builtins — Music & Spatial Audio": ["内置 — 音乐与空间音频", "組み込み — 音楽と空間音声", "내장 — 음악 & 공간 음향", "ในตัว — ดนตรีและเสียงเชิงพื้นที่"],
    "Builtins — Vector Fonts": ["内置 — 矢量字体", "組み込み — ベクタフォント", "내장 — 벡터 폰트", "ในตัว — ฟอนต์เวกเตอร์"],
    "Builtins — UI Toolkit": ["内置 — 界面工具包", "組み込み — UIツールキット", "내장 — UI 툴킷", "ในตัว — ชุดเครื่องมือ UI"],
    "Builtins — Physics": ["内置 — 物理", "組み込み — 物理", "내장 — 물리", "ในตัว — ฟิสิกส์"],
    "Builtins — Window & Camera": ["内置 — 窗口与摄像机", "組み込み — ウィンドウとカメラ", "내장 — 창 & 카메라", "ในตัว — หน้าต่างและกล้อง"],
    "Multilingual Index": ["多语言索引", "多言語インデックス", "다국어 색인", "ดัชนีหลายภาษา"],
    "Keyword Aliases by Language": ["各语言关键字别名", "言語別キーワード別名", "언어별 키워드 별칭", "ชื่อคีย์เวิร์ดตามภาษา"],
    "Builtin Aliases by Language": ["各语言内置别名", "言語別組み込み別名", "언어별 내장 별칭", "ชื่อฟังก์ชันตามภาษา"],
    "Examples": ["示例", "サンプル", "예제", "ตัวอย่าง"],
    "What makes Ling unique": ["灵 的独特之处", "灵 のユニークな点", "灵 만의 특별함", "อะไรทำให้ 灵 พิเศษ"],
    "How Ling compares": ["灵 的对比", "灵 の比較", "灵 비교", "灵 เทียบกับเครื่องมืออื่น"],
    "See it run": ["运行示例", "実行してみる", "실행해 보기", "ดูการทำงาน"],
    "Capability": ["能力", "機能", "기능", "ความสามารถ"],
    "Demo": ["演示", "デモ", "데모", "สาธิต"]
  };

  var langIndex = { en: -1, zh: 0, ja: 1, ko: 2, th: 3 };
  // store originals so we can revert / re-apply
  var nodes = [];

  function collect(root) {
    nodes = [];
    var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
    var n;
    while ((n = walker.nextNode())) {
      var key = n.nodeValue.trim();
      if (DICT[key]) nodes.push({ node: n, en: n.nodeValue, key: key });
    }
  }

  function apply(lang) {
    var idx = langIndex[lang];
    nodes.forEach(function (e) {
      e.node.nodeValue = (idx < 0) ? e.en : e.en.replace(e.key, DICT[e.key][idx]);
    });
    document.documentElement.setAttribute("data-ling-lang", lang);
    try { localStorage.setItem("ling-doc-lang", lang); } catch (_) {}
  }

  function build() {
    var bar = document.querySelector(".menu-bar .right-buttons") || document.querySelector(".menu-bar");
    if (!bar || document.getElementById("ling-lang-select")) return;
    var sel = document.createElement("select");
    sel.id = "ling-lang-select";
    sel.title = "Language";
    LANGS.forEach(function (l) {
      var o = document.createElement("option");
      o.value = l[0]; o.textContent = l[1]; sel.appendChild(o);
    });
    sel.addEventListener("change", function () { apply(sel.value); });
    bar.appendChild(sel);
    collect(document.querySelector("#content") || document.body);
    // also include the sidebar nav + menu title
    collect2();
    var saved = "en";
    try { saved = localStorage.getItem("ling-doc-lang") || "en"; } catch (_) {}
    sel.value = saved;
    if (saved !== "en") apply(saved);
  }

  function collect2() {
    // append nav + title text nodes to the node list
    [".sidebar", ".menu-title"].forEach(function (s) {
      var el = document.querySelector(s);
      if (!el) return;
      var walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, null);
      var n;
      while ((n = walker.nextNode())) {
        var key = n.nodeValue.trim();
        if (DICT[key]) nodes.push({ node: n, en: n.nodeValue, key: key });
      }
    });
  }

  if (document.readyState !== "loading") build();
  else document.addEventListener("DOMContentLoaded", build);
})();
