# Keyword Aliases by Language

Every keyword in Ling works in all supported languages simultaneously.
You can mix languages freely within a single source file.

## Core keyword table

| Token | English | Chinese 中文 | Japanese 日本語 | Korean 한국어 | Thai ไทย | Russian | Arabic | Spanish | French | German | Hindi | Portuguese |
|-------|---------|------------|----------------|--------------|---------|---------|--------|---------|--------|--------|-------|------------|
| Bind | `bind` | `令` `灵符` | `束縛` `バ` | `바인드` `묶` | `ผูก` | `связать` | `ربط` | `enlazar` | `lier` | `binden` | `बाँधो` | `ligar` |
| Do | `do` | `执` | `実行` `執` | `실행` | `ทำ` | `сделать` | `افعل` | `hacer` | `faire` | `machen` | `करो` | `fazer` |
| Function | `fn` | `函` | `関数` `関` | `함수` | `ฟังก์ชัน` | — | — | — | `func` | — | — | — |
| If | `if` | `若` `如` | `もし` | `만약` `조건` | `ถ้า` | `если` | `إذا` | `si` | — | `wenn` | `अगर` | `se` |
| Else | `else` | `否则` `否` | `他` | `아니면` | `มิฉะนั้น` | `иначе` | `وإلا` | `sino` | `sinon` | `sonst` | `नहींतो` | `senão` |
| While | `while` | `循` `当` | `間` `一方` | `동안` `반복` | `ขณะที่` | `пока` | `بينما` | `mientras` | `tantque` | `solange` | `जबकि` | `enquanto` |
| For | `for` | `历` | `繰` `ために` | `위해` | `สำหรับ` | `для` | `لأجل` | `para` | — | `für` | `केलिए` | — |
| In | `in` | `于` | `の中` | `안에` | `ใน` | — | `في` | — | — | — | — | — |
| Return | `return` | `归` | `戻る` `帰る` | `반환` `귀환` | `คืน` | `вернуть` | `أعد` | `retornar` | `retourner` | `zurück` | `वापस` | — |
| Match | `match` | `配` | `一致` | `매치` | `จับคู่` | — | — | — | — | — | — | — |
| Try | `try` | `试` | `試す` | `시도` | — | — | — | — | — | — | — | — |
| Module | `mod` | `核` | `モジュール` `模` | `모듈` | `โมดูล` | — | — | — | `module` | — | — | — |
| Spawn | `spawn` | `启` | `起動` | `생성` | — | — | — | — | — | — | — | — |
| Stop | `stop` | `止` | `停止` | `멈춤` | — | — | — | — | — | — | — | — |
| Continue | `again` | `继续` | `継続` | `계속` | — | — | — | — | — | — | — | — |
| Async | `async` | `异步` `异` | `非同期` | `비동기` | `ไม่พร้อมกัน` | — | — | — | — | — | — | — |
| Wait | `wait` | `待` | `待つ` | `기다려` | `รอ` | — | — | — | — | — | — | — |
| True | `true` | `真` | — | `참` | `จริง` | — | `صحيح` | `verdadero` | `vrai` | `wahr` | `सत्य` | `verdadeiro` |
| False | `false` | `假` `偽` | — | `거짓` | `เท็จ` | — | `خطأ` | `falso` | `faux` | `falsch` | `असत्य` | — |

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

# Spanish
enlazar inicio = hacer { ... }

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
