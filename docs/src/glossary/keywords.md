# Keywords

Reserved words and their spelling in each language. Some are reserved but do nothing yet; the description says so. See [Keywords](../language/keywords.md) for examples.

| English | 中文 | 日本語 | 한국어 | ไทย | Description |
|---|---|---|---|---|---|
| `bind` | `令` | `束縛` | `바인드` | `ผูก` | Bind a value or function to a name. Binding the same name again updates it. |
| `do` | `执` | `実行` | `실행` | `ทำ` | Block expression `do { … }`. Its value is the last expression. |
| `fn` | `函` | `関数` | `함수` | `ฟังก์ชัน` | Declare a named function. |
| `mod` | `核` | `モジュール` | `모듈` | `โมดูล` | Declare a module (a namespace of functions). |
| `type` | `符` | `型` | `타입` | `ชนิด` | Type alias. Parsed, no effect. |
| `if` | `若` | `もし` | `만약` | `ถ้า` | Conditional branch. |
| `else` | `否` | `他` | `아니면` | `มิฉะนั้น` | Alternative branch of `if`. |
| `while` | `循` | `間` | `동안` | `ขณะที่` | Loop while a condition holds. |
| `for` | `历` | `繰` | `위해` | `สำหรับ` | Loop over a range, list or text. |
| `in` | `于` | `の中` | `안에` | `ใน` | Separates the loop variable from the sequence: `for x in 0..10`. |
| `match` | `配` | `一致` | `매치` | `ตรงกับ` | Pattern matching. |
| `return` | `归` | `戻る` | `반환` | `คืนค่า` | Leave the function with a value. |
| `own` | `独` | `所有` | `소유` | `เป็นเจ้าของ` | Ownership marker. Parsed and ignored; nothing is checked. |
| `lend` | `借` | `貸す` | `빌려` | `ให้ยืม` | Ownership marker. Parsed and ignored; nothing is checked. |
| `share` | `共` | `共有` | `공유` | `แบ่งปัน` | Ownership marker. Parsed and ignored; nothing is checked. |
| `move` | `移` | `移動` | `이동` | `ย้าย` | Ownership marker. Parsed and ignored; nothing is checked. |
| `copy` | `复` | `複製` | `복사` | `คัดลอก` | Ownership marker. Parsed and ignored; nothing is checked. |
| `async` | `异` | `非同期` | `비동기` | `อะซิงค์` | Asynchronous marker. Parsed and ignored; code runs synchronously. |
| `wait` | `待` | `待つ` | `기다려` | `รอ` | Meant as "await". Unreliable; avoid it. |
| `as` | `为` | `として` | `로서` | `เป็น` | Names an imported module: `use "file" as m`. |
| `where` | `where` | `where` | `where` | `where` | Constraint clause after a function signature. Skipped, no effect. |
| `post` | `出` | `投稿` | `게시` | `ส่ง` | Reserved word; has no effect. |
| `give` | `予` | `渡す` | `전달` | `ให้` | Reserved word; has no effect. |
| `fit` | `fit` | `fit` | `fit` | `จับคู่` | Reserved word; has no effect. |
| `form` | `形` | `形` | `형태` | `รูปแบบ` | Declare a record type. |
| `choose` | `选` | `選ぶ` | `선택` | `เลือก` | Declare an enumeration with variants. |
| `can` | `能` | `できる` | `가능` | `สามารถ` | Reserved word; has no effect. |
| `change` | `变` | `変える` | `변경` | `เปลี่ยนแปลง` | Reserved word; has no effect. |
| `stop` | `止` | `停止` | `멈춤` | `หยุด` | Reserved word; has no effect. Ling has no `break`. |
| `again` | `继` | `続ける` | `계속` | `ทำอีก` | Reserved word; has no effect. Ling has no `continue`. |
| `try` | `试` | `試す` | `시도` | `ลอง` | Not syntax: the builtin `try(closure)` turns a runtime error into `bad(...)`. |
| `sure` | `确` | `確か` | `확실` | `แน่นอน` | Reserved word; has no effect. |
| `maybe` | `或` | `多分` | `아마도` | `อาจจะ` | Reserved word; has no effect. |
| `pure` | `纯` | `純粋` | `순수` | `บริสุทธิ์` | Reserved word; has no effect. |
| `spawn` | `启` | `起動` | `생성` | `สร้าง` | Reserved word; has no effect. Use `thread_spawn` for threads. |
| `ok` | `可` | `良い` | `좋아` | `ตกลง` | Success result `ok(value)`; also a `match` pattern. |
| `bad` | `误` | `悪い` | `나쁨` | `ผิดพลาด` | Failure result `bad(reason)`; also a `match` pattern. |
| `none` | `无` | `なし` | `없음` | `ไม่มี` | Not defined. Use `()` for "nothing". |
| `start` | `始` | `開始` | `시작` | `เริ่ม` | Usual name of the entry point: `bind start = do { … }`. A name, not a keyword. |
| `result` | `果` | `結果` | `결과` | `ผลลัพธ์` | Listed in the lexicon only; the lexer treats it as a name. |
