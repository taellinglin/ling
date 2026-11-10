// Ling docs: language switcher for the five translated books.
//
// English lives at <docs>/, the translations at <docs>/zh/, /ja/, /ko/, /th/
// with identical page paths, so switching keeps you on the same page.
// Each book's <html lang> says which language it is (book.toml `language`).
//
// Also: multilingual tables (header cells English | 中文 | 日本語 | 한국어 | ไทย)
// show English plus the reader's language column only, and the English root
// page offers the reader's browser language once.
(function () {
  var LANGS = [["en", "English"], ["zh", "中文"], ["ja", "日本語"], ["ko", "한국어"], ["th", "ไทย"]];
  var CODES = LANGS.map(function (l) { return l[0]; });

  function pageLang() {
    var l = (document.documentElement.getAttribute("lang") || "en").slice(0, 2).toLowerCase();
    return CODES.indexOf(l) >= 0 ? l : "en";
  }

  // Absolute URL of this book's root, from mdBook's global path_to_root.
  function bookRoot() {
    var rel = (typeof path_to_root === "string") ? path_to_root : "";
    return new URL(rel || "./", location.href);
  }

  function docsRoot() {
    var root = bookRoot();
    return pageLang() === "en" ? root : new URL("../", root);
  }

  function pagePath() {
    var rel = location.href.slice(bookRoot().href.length).split("#")[0];
    return rel || "index.html";
  }

  function urlFor(lang) {
    return new URL((lang === "en" ? "" : lang + "/") + pagePath(), docsRoot()).href;
  }

  // ── Multilingual tables ─────────────────────────────────────────────────
  var HEAD = { "english": "en", "中文": "zh", "chinese": "zh", "日本語": "ja", "japanese": "ja",
               "한국어": "ko", "korean": "ko", "ไทย": "th", "thai": "th" };

  function filterCols(lang) {
    document.querySelectorAll("#content table").forEach(function (t) {
      var row = t.tHead && t.tHead.rows[0];
      if (!row) return;
      var langs = Array.prototype.map.call(row.cells, function (c) {
        return HEAD[(c.textContent || "").trim().toLowerCase()] || null;
      });
      var present = langs.filter(Boolean);
      if (present.length < 2) return;            // not a multilingual table
      var keep = { en: true };
      keep[present.indexOf(lang) >= 0 ? lang : "en"] = true;
      langs.forEach(function (lc, i) {
        if (!lc) return;
        var show = !!keep[lc];
        Array.prototype.forEach.call(t.rows, function (r) {
          if (r.cells[i]) r.cells[i].style.display = show ? "" : "none";
        });
      });
    });
  }

  // ── Switcher ────────────────────────────────────────────────────────────
  function build() {
    var bar = document.querySelector(".menu-bar .right-buttons") || document.querySelector(".menu-bar");
    if (!bar || document.getElementById("ling-lang-select")) return;
    var cur = pageLang();
    var sel = document.createElement("select");
    sel.id = "ling-lang-select";
    sel.title = "Language / 语言 / 言語 / 언어 / ภาษา";
    sel.setAttribute("aria-label", sel.title);
    LANGS.forEach(function (l) {
      var o = document.createElement("option");
      o.value = l[0];
      o.textContent = l[1];
      if (l[0] === cur) o.selected = true;
      sel.appendChild(o);
    });
    sel.addEventListener("change", function () {
      try { localStorage.setItem("ling-doc-lang", sel.value); } catch (_) {}
      location.href = urlFor(sel.value);
    });
    bar.appendChild(sel);
    filterCols(cur);
    try { localStorage.setItem("ling-doc-lang", cur); } catch (_) {}
  }

  // First visit to the English docs from a zh/ja/ko/th browser: go to that
  // language once. An explicit choice (stored) always wins.
  function suggest() {
    if (pageLang() !== "en") return;
    var stored = null;
    try { stored = localStorage.getItem("ling-doc-lang"); } catch (_) { return; }
    if (stored) return;
    var nav = ((navigator.languages && navigator.languages[0]) || navigator.language || "").slice(0, 2).toLowerCase();
    if (nav && nav !== "en" && CODES.indexOf(nav) >= 0) {
      try { localStorage.setItem("ling-doc-lang", nav); } catch (_) {}
      location.replace(urlFor(nav));
    }
  }

  suggest();
  if (document.readyState !== "loading") build();
  else document.addEventListener("DOMContentLoaded", build);
})();
