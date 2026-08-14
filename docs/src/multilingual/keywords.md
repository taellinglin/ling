# Keyword Aliases by Language

Every keyword in Ling works in all supported languages simultaneously.
You can mix languages freely within a single source file.

## Core keyword table

| Token | English | Chinese 中文 | Japanese 日本語 | Korean 한국어 | Thai ไทย | Russian русский | Arabic العربية | Persian فارسی | Hebrew עברית | Urdu اردو | Spanish | French français | German Deutsch | Hindi | Portuguese |
|-------|---------|------------|----------------|--------------|---------|---------|-----------------|----------------|---------------|-----------|---------|--------|--------|-------|------------|
| Bind | `bind` | `令` `灵符` | `束縛` `バ` | `바인드` `묶` | `ผูก` | `связать` | `ربط` | `پیوند` | `קשר` | `باندھو` | `enlazar` | `lier` | `binden` | `बाँधो` | `ligar` |
| Do | `do` | `执` | `実行` `執` | `실행` | `ทำ` | `сделать` | `افعل` | `انجام` | `בצע` | `کرو` | `hacer` | `faire` | `machen` | `करो` | `fazer` |
| Function | `fn` | `函` | `関数` `関` | `함수` | `ฟังก์ชัน` | `функция` | `دالة` | `تابع` | `פונקציה` | `تفاعل` | — | `fonction` | `funktion` | — | — |
| If | `if` | `若` `如` | `もし` | `만약` `조건` | `ถ้า` | `если` | `إذا` | `اگر` | `אם` | `اگر` | `si` | `si` | `wenn` | `अगर` | `se` |
| Else | `else` | `否则` `否` | `他` | `아니면` | `มิฉะนั้น` | `иначе` | `وإلا` | `وگرنه` | `אחרת` | `ورنہ` | `sino` | `sinon` | `sonst` | `नहींतो` | `senão` |
| While | `while` | `循` `当` | `間` `一方` | `동안` `반복` | `ขณะที่` | `пока` | `بينما` | `هنگامی` | `כל_עוד` | `جب_تک` | `mientras` | `tantque` | `solange` | `जबकि` | `enquanto` |
| For | `for` | `历` | `繰` `ために` | `위해` | `สำหรับ` | `для` | `لأجل` | `برای` | `עבור` | `کے_لیے` | `para` | `pour` | `für` | `केलिए` | — |
| In | `in` | `于` | `の中` | `안에` | `ใน` | `в` | `في` | `در` | `בתוך` | `میں` | — | `dans` | — | — | — |
| Return | `return` | `归` | `戻る` `帰る` | `반환` `귀환` | `คืน` | `вернуть` | `أعد` | `بازگشت` | `החזר` | `واپس` | `retornar` | `retourner` | `zurück` | `वापस` | — |
| Match | `match` | `配` | `一致` | `매치` | `จับคู่` | `сопоставить` | `طابق` | `تطبیق` | `התאמה` | `مطابقت` | — | `correspondre` | `abgleichen` | — | — |
| Try | `try` | `试` | `試す` | `시도` | `ลอง` | `пробовать` | `حاول` | `تلاش` | `נסה` | `کوشش` | — | `essayer` | `versuchen` | — | — |
| Module | `mod` | `核` | `モジュール` `模` | `모듈` | `โมดูล` | `модуль` | `وحدة` | `ماژول` | `מודול` | `ماڈیول` | — | `module` | `modul` | — | — |
| Spawn | `spawn` | `启` | `起動` | `생성` | `สร้าง` | `создать` | `أنشئ` | `ایجاد` | `צור` | `پیدا` | — | `engendrer` | `erzeugen` | — | — |
| Stop | `stop` | `止` | `停止` | `멈춤` | `หยุด` | `стоп` | `توقف` | `توقف` | `עצור` | `رکو` | — | `arrêter` | `stoppen` | — | — |
| Continue | `again` | `继续` | `継続` | `계속` | `ทำอีก` | `снова` | `مرة_أخرى` | `دوباره` | `שוב` | `دوبارہ` | — | `encore` | `wieder` | — | — |
| Async | `async` | `异步` `异` | `非同期` | `비동기` | `ไม่พร้อมกัน` | `асинхронно` | `غير_متزامن` | `ناهمگام` | `אסינכרוני` | `غیر_ہمزمان` | — | `asynchrone` | `asynchron` | — | — |
| Wait | `wait` | `待` | `待つ` | `기다려` | `รอ` | `ждать` | `انتظر` | `انتظار` | `המתן` | `انتظار` | — | `attendre` | `warten` | — | — |
| True | `true` | `真` | — | `참` | `จริง` | `истина` | `صحيح` | `درست` | `אמת` | `سچ` | `verdadero` | `vrai` | `wahr` | `सत्य` | `verdadeiro` |
| False | `false` | `假` `偽` | — | `거짓` | `เท็จ` | `ложь` | `خطأ` | `نادرست` | `שקר` | `جھوٹ` | `falso` | `faux` | `falsch` | `असत्य` | — |

---

## Entry point forms by language

The program entry point `令 启 = 执 { ... }` in different languages:

```ling
# Chinese
令 启 = 执 { ... }

# Thai
ผูก เริ่ม = ทำ { ... }

# English
bind start = do { ... }

# Korean
바인드 시작 = 실행 { ... }

# Japanese
束縛 スタート = 実行 { ... }

# Russian
связать начало = сделать { ... }

# Arabic
ربط ابدأ = افعل { ... }

# Persian
پیوند شروع = انجام { ... }

# Hebrew
קשר התחל = בצע { ... }

# Urdu
باندھو شروع = کرو { ... }

# Spanish
enlazar inicio = hacer { ... }

# French
lier début = faire { ... }

# German
binden anfang = machen { ... }

# Hindi
बाँधो शुरू = करो { ... }
```

---

## Function definition forms

```ling
# Chinese
函 add(a, b) { 归 a + b }

# Thai
ฟังก์ชัน add(a, b) { คืน a + b }

# English
fn add(a, b) { return a + b }

# Korean
함수 더하기(a, b) { 반환 a + b }

# Arabic
دالة اجمع(a, b) { أعد a + b }

# Persian
تابع جمع(a, b) { بازگشت a + b }

# Hebrew
פונקציה חבר(a, b) { החזר a + b }

# Urdu
تفاعل جمع(a, b) { واپس a + b }

# Russian
функция сложить(a, b) { вернуть a + b }

# French
fonction ajouter(a, b) { retourner a + b }

# German
funktion addieren(a, b) { zurück a + b }
```

---

## Loop forms

```ling
# Chinese while loop
令 i = 0
循 i < 10 {
    令 i = i + 1
}

# Thai while loop
ให้ i = 0
ขณะที่ i < 10 {
    ให้ i = i + 1
}

# English while loop
bind i = 0
while i < 10 {
    bind i = i + 1
}
```
