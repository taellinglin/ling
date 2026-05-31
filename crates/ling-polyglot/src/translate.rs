//! Keyword translation between canonical English and native-script forms.

use crate::lexicon::{LanguageCode, Lexicon};

/// Translate a canonical English keyword to its surface form in `lang`.
/// Returns the canonical form unchanged if no mapping exists.
pub fn translate_keyword<'a>(canonical: &'a str, lexicon: &'a Lexicon) -> &'a str {
    lexicon.to_native(canonical).unwrap_or(canonical)
}

/// Build the built-in Chinese (Simplified) lexicon.
pub fn builtin_zh() -> Lexicon {
    let mut lex = Lexicon::new(LanguageCode::Zh);
    for (c, n) in [
        ("fn","函"), ("bind","令"), ("if","若"), ("else","否"), ("while","循"),
        ("for","历"), ("in","于"), ("return","归"), ("match","配"), ("do","执"),
        ("async","异"), ("wait","待"), ("spawn","启"), ("mod","核"), ("type","符"),
        ("true","真"), ("false","假"), ("print","印"), ("debug","查"), ("panic","慌"),
        ("open_window","开窗"), ("fill","填"), ("set_color","设色"), ("present","显"),
        ("draw_line","画线"), ("draw_pixel","画点"), ("triangle","画三角"),
        ("open_fullscreen","全屏"), ("window_is_open","窗开"), ("key_down","按键"),
        ("mouse_x","鼠标X"), ("mouse_y","鼠标Y"), ("time","时"), ("sleep","眠"),
        ("sin","正弦"), ("cos","余弦"), ("sqrt","根"), ("abs","绝对值"),
        ("len","长"), ("push","加"), ("pop","弹"), ("new_list","新列"),
    ] { lex.insert(c, n); }
    lex
}

/// Build the built-in Japanese lexicon.
pub fn builtin_ja() -> Lexicon {
    let mut lex = Lexicon::new(LanguageCode::Ja);
    for (c, n) in [
        ("fn","関数"), ("bind","束縛"), ("if","もし"), ("else","他"), ("while","間"),
        ("for","繰"), ("in","の中"), ("return","戻る"), ("match","一致"), ("do","実行"),
        ("async","非同期"), ("wait","待つ"), ("spawn","起動"), ("mod","モジュール"),
        ("true","真"), ("false","偽"), ("print","印刷"), ("debug","調査"), ("panic","パニック"),
        ("open_window","ウィンドウ開く"), ("fill","塗り潰し"), ("set_color","色設定"),
        ("present","表示"), ("draw_line","線描く"), ("draw_pixel","点描く"),
        ("triangle","三角形"), ("open_fullscreen","全画面"), ("window_is_open","開いている"),
        ("key_down","キー押す"), ("time","時刻"), ("sleep","眠る"),
        ("sin","正弦"), ("cos","余弦"), ("sqrt","平方根"), ("abs","絶対値"),
        ("len","長さ"), ("push","追加"), ("pop","取出"), ("new_list","新リスト"),
    ] { lex.insert(c, n); }
    lex
}

/// Build the built-in Korean lexicon.
pub fn builtin_ko() -> Lexicon {
    let mut lex = Lexicon::new(LanguageCode::Ko);
    for (c, n) in [
        ("fn","함수"), ("bind","바인드"), ("if","만약"), ("else","아니면"), ("while","동안"),
        ("for","반복"), ("in","안에"), ("return","반환"), ("match","매치"), ("do","실행"),
        ("async","비동기"), ("wait","기다려"), ("spawn","생성"), ("mod","모듈"),
        ("true","참"), ("false","거짓"), ("print","출력"), ("debug","디버그"), ("panic","패닉"),
        ("open_window","창열기"), ("fill","채우기"), ("set_color","색설정"), ("present","표시"),
        ("draw_line","선그리기"), ("draw_pixel","점그리기"), ("triangle","삼각형"),
        ("open_fullscreen","전체화면"), ("window_is_open","창열림"), ("key_down","키누름"),
        ("time","시간"), ("sleep","잠"), ("sin","사인"), ("cos","코사인"),
        ("sqrt","제곱근"), ("abs","절대값"), ("len","길이"), ("push","추가"),
        ("pop","꺼내기"), ("new_list","새목록"),
    ] { lex.insert(c, n); }
    lex
}

/// Build the built-in Thai lexicon.
pub fn builtin_th() -> Lexicon {
    let mut lex = Lexicon::new(LanguageCode::Th);
    for (c, n) in [
        ("fn","ฟังก์ชัน"), ("bind","ผูก"), ("if","ถ้า"), ("else","มิฉะนั้น"),
        ("while","ขณะที่"), ("for","สำหรับ"), ("in","ใน"), ("return","คืน"),
        ("match","จับคู่"), ("do","ทำ"), ("async","ไม่พร้อมกัน"), ("wait","รอ"),
        ("spawn","สร้าง"), ("mod","โมดูล"), ("true","จริง"), ("false","เท็จ"),
        ("print","พิมพ์"), ("debug","ตรวจสอบ"), ("panic","ตื่นตระหนก"),
        ("open_window","เปิดหน้าต่าง"), ("fill","เติม"), ("set_color","สีดินสอ"),
        ("present","แสดงผล"), ("draw_line","วาดเส้น"), ("draw_pixel","วาดจุด"),
        ("triangle","วาดสามเหลี่ยม"), ("open_fullscreen","เปิดหน้าต่างเต็มจอ"),
        ("window_is_open","หน้าต่างเปิดอยู่"), ("key_down","กดค้าง"),
        ("mouse_x","เมาส์X"), ("mouse_y","เมาส์Y"), ("time","เวลา"), ("sleep","นอนหลับ"),
        ("sin","ซายน์"), ("cos","โคซายน์"), ("sqrt","รากที่สอง"), ("abs","ค่าสัมบูรณ์"),
        ("len","ความยาว"), ("push","ต่อท้าย"), ("pop","ดึงออก"), ("new_list","รายการใหม่"),
    ] { lex.insert(c, n); }
    lex
}
