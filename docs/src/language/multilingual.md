# Writing in Five Languages

You can write Ling in English, Chinese, Japanese, Korean or Thai. Every keyword has a
name in each of these languages, and so do most everyday builtins. The interpreter
accepts all of them at once: there is no setting to switch, and you can mix languages
in one file, even in one line.

## The same program, five times

Here is one small program written in each language. Keywords, builtins, names,
comments and text are all in that language, and each version runs as it is.

**English**

```ling
# Count the fruit in a basket
fn describe(n) {
    if n == 1 {
        return "1 fruit"
    } else {
        return to_str(n) + " fruits"
    }
}

bind start = do {
    bind basket = ["apple", "pear", "plum"]
    for fruit in basket {
        print("- ", fruit)
    }
    print(describe(len(basket)))
    print("square root of 16: ", sqrt(16))
}
```

```output
- apple
- pear
- plum
3 fruits
square root of 16: 4
```

**中文**

```ling
# 数一数篮子里的水果
函 描述(数量) {
    若 数量 == 1 {
        归 "1 个水果"
    } 否则 {
        归 to_str(数量) + " 个水果"
    }
}

令 始 = 执 {
    令 篮子 = ["苹果", "梨", "李子"]
    历 水果 于 篮子 {
        打印("- ", 水果)
    }
    打印(描述(长度(篮子)))
    打印("16 的平方根：", 平方根(16))
}
```

```output
- 苹果
- 梨
- 李子
3 个水果
16 的平方根：4
```

**日本語**

```ling
# かごの果物を数える
関数 説明(個数) {
    もし 個数 == 1 {
        戻る "果物 1 個"
    } 他 {
        戻る "果物 " + to_str(個数) + " 個"
    }
}

束縛 開始 = 実行 {
    束縛 かご = ["りんご", "なし", "すもも"]
    繰 果物 の中 かご {
        印刷("- ", 果物)
    }
    印刷(説明(長さ(かご)))
    印刷("16 の平方根：", 平方根(16))
}
```

```output
- りんご
- なし
- すもも
果物 3 個
16 の平方根：4
```

**한국어**

```ling
# 바구니 속 과일 세기
함수 설명(개수) {
    만약 개수 == 1 {
        반환 "과일 1개"
    } 아니면 {
        반환 "과일 " + to_str(개수) + "개"
    }
}

바인드 시작 = 실행 {
    바인드 바구니 = ["사과", "배", "자두"]
    위해 과일 안에 바구니 {
        출력("- ", 과일)
    }
    출력(설명(길이(바구니)))
    출력("16의 제곱근: ", 제곱근(16))
}
```

```output
- 사과
- 배
- 자두
과일 3개
16의 제곱근: 4
```

**ไทย**

```ling
# นับผลไม้ในตะกร้า
ฟังก์ชัน บรรยาย(จำนวน) {
    ถ้า จำนวน == 1 {
        คืน "ผลไม้ 1 ผล"
    } มิฉะนั้น {
        คืน "ผลไม้ " + to_str(จำนวน) + " ผล"
    }
}

ผูก เริ่ม = ทำ {
    ผูก ตะกร้า = ["แอปเปิล", "สาลี่", "พลัม"]
    สำหรับ ผลไม้ ใน ตะกร้า {
        พิมพ์("- ", ผลไม้)
    }
    พิมพ์(บรรยาย(ความยาว(ตะกร้า)))
    พิมพ์("รากที่สองของ 16: ", รากที่สอง(16))
}
```

```output
- แอปเปิล
- สาลี่
- พลัม
ผลไม้ 3 ผล
รากที่สองของ 16: 4
```

A few things to notice:

- Names you choose (`describe`, `篮子`, `かご`, `바구니`, `ตะกร้า`) can use any of these
  scripts.
- The program starts at the binding whose value is a `do` block, so you can call it
  `start`, `始`, `開始`, `시작` or `เริ่ม`.
- `to_str` stays in English in every version, because it has no alias in these
  languages. English names always work.
- When a file uses another language, `ling run` prints a short note such as
  `[detected language: Thai (ภาษาไทย)]` to standard error. Your program's output is
  unchanged.

## Keywords

The most common keywords:

| English | 中文 | 日本語 | 한국어 | ไทย |
|---|---|---|---|---|
| `bind` | `令` | `束縛` | `바인드` | `ผูก` |
| `do` | `执` | `実行` | `실행` | `ทำ` |
| `fn` | `函` | `関数` | `함수` | `ฟังก์ชัน` |
| `return` | `归` | `戻る` | `반환` | `คืน` |
| `if` | `若` | `もし` | `만약` | `ถ้า` |
| `else` | `否则` | `他` | `아니면` | `มิฉะนั้น` |
| `while` | `循` | `間` | `동안` | `ขณะที่` |
| `for` | `历` | `繰` | `위해` | `สำหรับ` |
| `in` | `于` | `の中` | `안에` | `ใน` |
| `match` | `配` | `一致` | `매치` | `จับคู่` |
| `form` | `形` | `形` | `구조` | `โครงสร้าง` |
| `ok` | `好` | `良い` | `좋아` | `ตกลง` |
| `bad` | `坏` | `悪い` | `나쁨` | `ผิดพลาด` |
| `true` | `真` | `真` | `참` | `จริง` |
| `false` | `假` | `偽` | `거짓` | `เท็จ` |

Some keywords have more than one spelling (Chinese `如` also means `if`, Korean `반복`
also means `while`). For the full list, see [Keywords](../glossary/keywords.md).

## Builtins

Builtins work the same way: `print`, `打印`, `印刷`, `출력` and `พิมพ์` are one function.

| English | 中文 | 日本語 | 한국어 | ไทย |
|---|---|---|---|---|
| `print` | `打印` | `印刷` | `출력` | `พิมพ์` |
| `len` | `长度` | `長さ` | `길이` | `ความยาว` |
| `sqrt` | `平方根` | `平方根` | `제곱근` | `รากที่สอง` |
| `abs` | `绝对值` | `絶対値` | `절댓값` | `ค่าสัมบูรณ์` |
| `max` | `最大` | `最大` | `최댓값` | `สูงสุด` |
| `rand` | `随机` | `乱数` | `난수` | `สุ่ม` |
| `list_push` | `列表添加` | `リスト追加` | `목록추가` | `เพิ่มรายการ` |
| `open_window` | `开窗` | `ウィンドウ開く` | `창열기` | `เปิดหน้าต่าง` |
| `window_is_open` | `窗开` | `開いている` | `창열림` | `หน้าต่างเปิดอยู่` |
| `fill` | `填` | `塗り潰し` | `채우기` | `เติม` |
| `present` | `显` | `表示` | `표시` | `แสดงผล` |
| `set_color` | `设色` | `色設定` | `색설정` | `สีดินสอ` |
| `key_down` | `按键` | `キー押す` | `키누름` | `กดค้าง` |
| `audio_blip` | `提示音` | `ビープ音` | `효과음` | `เสียงบี๊บ` |
| `time_now` | `当前时间` | `経過時間` | `현재시간` | `เวลาปัจจุบัน` |

Which builtins have aliases:

- **In all five languages:** printing, maths, lists, the window and frame loop, 2D and 3D
  drawing, most keyboard input, audio and music playback, time, and the `ui_*` widgets.
- **English only:** most text helpers (`to_str`, `to_number`, `str_upper`, `str_split`, ...),
  file helpers such as `write_file`, the music studio (`daw_*`), and most of the web server,
  database and request helpers (`http_*`, `db_*`, `query_param`, `json_escape`). Some of
  these have a Thai alias (`http_route` is also `เว็บเส้นทาง`, `db_open` is also
  `ฐานข้อมูลเปิด`), but no Chinese, Japanese or Korean one.

The [Builtins by Crate](../glossary/builtins.md) pages list each builtin with its aliases.

> **Tip:** A misspelled or unknown name is not an error on the default engine; the call
> just returns `()`. For example `长` is not a builtin (the Chinese for `len` is `长度`).
> If a call seems to do nothing, run `ling run --interp yourfile.ling`, which stops with
> `unknown function`.

## Mixing languages in one file

Nothing stops you from using several languages together:

```ling
# English, Chinese, Japanese, Korean and Thai in one file
函 double(x) {
    归 x * 2
}

bind start = do {
    束縛 n = 21
    만약 n > 10 {
        พิมพ์("double: ", double(n))
    }
    print(长度([1, 2, 3]))
}
```

```output
double: 42
3
```

This is handy when you learn from an example in another language, or when a team shares
code. For a project you plan to keep, one language per project is easier to read.

## Convert a project with lingfu normalize

`lingfu normalize` rewrites a whole project into one language. Run it in the project
folder:

```sh
lingfu normalize --dry-run zh    # show what would change
lingfu normalize zh              # rewrite to Chinese
lingfu normalize en              # and back to English
```

The language can be `en`, `zh`, `ja`, `ko` or `th` (or a name such as `thai`).

- It translates keywords and builtin names in every `.ling` file under the current folder,
  including subfolders.
- It also **renames files and folders** whose names it knows how to translate. Add
  `--content-only` to change only file contents, or `--files-only` to only rename.
- It doesn't translate your own names, comments or text. Do those by hand.
- Builtins without an alias in the target language keep their English names.

> **Warning:** `normalize` changes files in place. Commit your work or make a copy first,
> and run it only in the project's own folder.

After converting, run the program once to check it. In particular, look at `form`, `ok` and
`bad`: if the new spelling doesn't parse or does nothing, replace it with the
one in the keyword table above.
Builtins can be affected too: if a converted call suddenly returns `()`, run
`ling run --interp`, which names the unknown function, and switch that call back to
its English name.
