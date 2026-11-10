# lingfu

`lingfu` (灵符 แปลว่า "ยันต์วิญญาณ") คือตัวจัดการโปรเจกต์ของ Ling ใช้สร้างโปรเจกต์ รันโปรเจกต์และชุดทดสอบ
บิลด์ไฟล์โปรแกรม ติดต่อกับคลังแพ็กเกจ และเขียนทั้งโปรเจกต์ใหม่เป็นภาษามนุษย์อีกภาษาได้
เวลารันและบิลด์ มันจะเรียก `ling` ซึ่งหาจากข้างตัวเองก่อน แล้วจึงหาใน `PATH`

```text
lingfu <command> [arguments]
lingfu --help | -h | help
lingfu --version | -V | version
```

`lingfu` หาโปรเจกต์โดยค้นหาแมนิเฟสต์ในโฟลเดอร์ปัจจุบันและโฟลเดอร์แม่: `Ling.toml`, `灵符.toml`,
`霊符.toml`, `영부.toml` หรือ `ลิงฟู.toml`

## ภาพรวมคำสั่ง

| คำสั่ง | ชื่ออื่น | หน้าที่ |
|---|---|---|
| `new <name>` | `create` | สร้างโฟลเดอร์โปรเจกต์ |
| `init` | `initialize` | สร้างโปรเจกต์ในโฟลเดอร์ปัจจุบัน |
| `run` | `execute`, `start` | รันโปรเจกต์ |
| `test` | `verify` | รันชุดทดสอบของโปรเจกต์ |
| `build` | `compile`, `make` | บิลด์ไฟล์โปรแกรมแบบเนทีฟ |
| `clean` | `purge`, `clear` | ลบผลลัพธ์จากการบิลด์ |
| `tree` | `structure`, `ls` | พิมพ์โครงสร้างโปรเจกต์ |
| `manifest` | `show`, `info` | พิมพ์แมนิเฟสต์ |
| `normalize <lang>` | `norm`, `localize` | เขียนโปรเจกต์ใหม่เป็นภาษาอื่น |
| `add <package>` | `install`, `include` | ติดตั้งแพ็กเกจจากคลัง |
| `search <query>` | `find`, `seek` | ค้นหาในคลัง |
| `publish` | `release`, `share` | เผยแพร่โปรเจกต์ |
| `login <api-key>` | `sign-in`, `auth` | บันทึกข้อมูลยืนยันตัวตนของคลัง |
| `logout` | `sign-out`, `deauth` | ลบข้อมูลยืนยันตัวตน |
| `doc` | `docs`, `document` | บิลด์เอกสาร |
| `bench`, `update` | | รัน `cargo bench` / `cargo update` |
| `wizard` | `guide`, `create-wizard` | สร้างโปรเจกต์แบบโต้ตอบ |
| `seed` | | ลายนิ้วมือความสมบูรณ์ของโปรเจกต์ |
| `ast`, `convert` | | ส่งต่อให้ `ling ast` / `ling convert` |
| `check`, `fmt`, `translate` | | ยังไม่ควรใช้ (ดูด้านล่าง) |

ตัวเลือกที่ใช้บ่อย: `--lang`/`-l <code>` (ภาษาของโครงโปรเจกต์: `en`, `zh`, `ja`, `ko`, `th`, `ru`),
`--type`/`-t <kind>`, `--root-lang <code>`, `--crates`, `--path <dir>`, `--dry-run`/`-n`

แต่ละคำสั่งมีชื่อในภาษาอื่นด้วย เช่น `试` แทน `test` ในภาษาจีน หรือ `ทดสอบ` ในภาษาไทย
ชื่อเหล่านี้ใช้ได้เฉพาะเมื่อไฟล์โปรแกรมเองมีชื่อเป็นภาษานั้น (เช่น `灵符.exe` หรือ `ลิงฟู`)
ถ้าเรียกด้วยชื่อ `lingfu` ให้ใช้คำสั่งภาษาอังกฤษ

### `lingfu new` และ `lingfu init`

```text
lingfu new <name> [--type bin|lib|game|ui|ai|crypto|web|polyglot] [--lang en|zh|ja|ko|th|ru]
```

สร้าง `<name>/` ที่มีแมนิเฟสต์ โฟลเดอร์ซอร์สพร้อมโปรแกรมจุดเริ่มต้น โฟลเดอร์ทดสอบ โฟลเดอร์คลังคำ `README.md`
`.gitignore` และ `ling.ignore` ส่วน `lingfu init` ทำแบบเดียวกันในโฟลเดอร์ปัจจุบัน และใช้ชื่อโฟลเดอร์เป็นชื่อโปรเจกต์

`--lang` เลือกภาษาของไฟล์ที่สร้าง**และ**ชื่อโฟลเดอร์:

| ภาษา | แมนิเฟสต์ | ซอร์ส | จุดเริ่มต้น | ทดสอบ | บิลด์ |
|---|---|---|---|---|---|
| อังกฤษ | `Ling.toml` | `src` | `start.ling` | `tests` | `build` |
| จีน | `灵符.toml` | `灵源` | `启.灵` | `灵镜` | `灵碑` |
| ญี่ปุ่น | `霊符.toml` | `霊源` | `啓.霊` | `霊鏡` | `霊碑` |
| เกาหลี | `영부.toml` | `령원` | `시작.령` | `령경` | `령비` |
| ไทย | `ลิงฟู.toml` | `ต้นกำเนิด` | `เริ่ม.ลิง` | `ทดสอบ` | `สร้าง` |

แมนิเฟสต์มีตาราง `[spirit]` (`name`, `kind`, `version` และ `entry` ที่ใส่หรือไม่ก็ได้), `[author]`,
`[realm]` (`language`, `level`, `source`), `[root]` (ไลบรารี `core`) และ `[depends]`
ตัวอย่างดูได้ที่[การรันโปรแกรม](../getting-started/running.md#lingtoml)

### `lingfu run`

รันโปรแกรมจุดเริ่มต้นของโปรเจกต์ด้วย `ling run` จุดเริ่มต้นคือไฟล์แรกที่มีอยู่จริงในลำดับนี้: `entry` ในแมนิเฟสต์,
`<src>/<ไฟล์จุดเริ่มต้น>`, `<root>/<ไฟล์จุดเริ่มต้น>`, `<src>/main.ling` และ `<root>/main.ling`
อาร์กิวเมนต์เพิ่มเติมจะถูกส่งให้ `ling run`

```text
$ lingfu run
Running ...\demo\src\start.ling...
🌊 Welcome to demo Spirit Realm 🌊
Spirit Level: 铁
```

### `lingfu test`

รันไฟล์ `.ling` ทุกไฟล์ในโฟลเดอร์ทดสอบด้วย `ling run` และหยุดที่ไฟล์แรกที่ล้มเหลว
การทดสอบคือโปรแกรมธรรมดาที่มีจุดเริ่มต้น (การ `ผูก` ใดก็ตามที่มีค่าเป็นบล็อก `ทำ` นับเป็นจุดเริ่มต้น)

```text
$ lingfu test
Testing Ling project...
  test: ...\demo\tests\test.ling
✨ All mirrors are clear ✨
🔮 Ling tests passed for demo
✓ All tests passed!
```

การทดสอบรันบน JIT ซึ่งไม่หยุดเมื่อเจอฟังก์ชันที่ไม่รู้จัก การทดสอบที่พังจึงอาจ "ผ่าน" ได้
ให้แต่ละการทดสอบพิมพ์ PASS หรือ FAIL ให้ชัดเจน หรือรันเองด้วย `ling run --interp`
ไฟล์ตัวช่วยที่อยู่ในโฟลเดอร์ทดสอบก็จะถูกนับเป็นการทดสอบด้วย

### `lingfu build`

เรียก `ling build <project> --aot` และส่ง `--out`, `--platform`, `--icon` และ `--pack` ต่อไปให้
`--no-aot` จะฝังอินเทอร์พรีเตอร์แทน ส่วน `--path <dir>` ใช้เลือกโฟลเดอร์โปรเจกต์ ถ้ามี `Cargo.toml`
อยู่ข้างแมนิเฟสต์ จะรัน `cargo build` แทน

ก่อนบิลด์ครั้งแรก ให้เพิ่ม `entry = "src/start.ling"` ใต้ `[spirit]` ไม่เช่นนั้น `ling build` จะหา `main.ling`
และหยุดด้วย `entry '...' not found` การบิลด์ต้องใช้ชุดเครื่องมือ Rust ดู[คำสั่ง ling](ling.md#ling-build)

### `lingfu clean`, `tree`, `manifest`

- `clean` ลบโฟลเดอร์บิลด์และโฟลเดอร์ล็อก (`build` และ `.lock` หรือชื่อตามภาษา)
  และรัน `cargo clean` ถ้ามี crate ของ Rust
- `tree` พิมพ์โครงสร้างโฟลเดอร์ของโปรเจกต์
- `manifest` พิมพ์ตำแหน่งและเนื้อหาของแมนิเฟสต์

### `lingfu normalize`

```text
lingfu normalize <lang> [--dry-run|-n] [--content-only] [--files-only]
```

เขียนทั้งโปรเจกต์ในโฟลเดอร์ปัจจุบันใหม่เป็นภาษามนุษย์ภาษาเดียว โดยแปลคำสงวนและชื่อฟังก์ชันในตัวในไฟล์ซอร์สทุกไฟล์
เปลี่ยนชื่อไฟล์ซอร์สและโฟลเดอร์มาตรฐาน และแก้การนำเข้าให้ตรงกัน `<lang>` คือรหัสอย่าง `en`, `zh`, `ja`, `ko`
หรือ `th` (ชื่ออย่าง `chinese` หรือ `中文` ก็ใช้ได้) ถ้าไม่ระบุจะแสดงเมนูให้เลือก

| ตัวเลือก | ผล |
|---|---|
| `--dry-run`, `-n` | แสดงรายการที่จะเปลี่ยนโดยไม่แตะต้องอะไร |
| `--content-only` | เขียนเนื้อหาไฟล์ใหม่ แต่คงชื่อเดิม |
| `--files-only` | เปลี่ยนชื่อไฟล์และโฟลเดอร์ แต่คงเนื้อหาเดิม |

```text
$ lingfu normalize zh --dry-run
Normalizing to Chinese  [dry-run]
  ~ lexicons\en.ling
  ~ src\start.ling
  ~ tests\test.ling
  mv tests\test.ling → tests\测试.ling
  mv build → 构建
  mv src → 源码
  mv tests → 测试

Dry run complete — no files changed.
  ✓ 3 file(s) rewritten
  ~ 1 file(s) renamed
  ↕ 3 folder(s) renamed
```

> **คำเตือน:** `normalize` เขียนไฟล์ Ling **ทุกไฟล์**ในโฟลเดอร์ทับของเดิม ให้คอมมิตโปรเจกต์ (หรือทำสำเนา)
> และทดลองด้วย `--dry-run` ก่อน

ชื่อตัวแปร คอมเมนต์ และข้อความในสตริงจะคงเดิม เปลี่ยนเฉพาะคำสงวนและฟังก์ชันในตัว `normalize` ยังใช้ชื่อโฟลเดอร์ของตัวเอง
(เช่น `源码`, `测试`, `构建` สำหรับภาษาจีน) ซึ่งต่างจากชื่อที่ `lingfu new --lang zh` สร้าง

### `lingfu check`, `fmt`, `translate`

คำสั่งเหล่านี้มีอยู่ แต่เรียกคำสั่งย่อยของ `ling` ที่ไม่มีอยู่จริง (`ling check`, `ling fmt`,
`ling <from> <to> <file>`) `check` และ `fmt` จึงรายงานว่าสำเร็จโดยไม่ได้ทำอะไร และ `translate` แค่พิมพ์ข้อความวิธีใช้ของ `ling`
ถ้าจะเปลี่ยนภาษาของโปรเจกต์ ให้ใช้ `lingfu normalize`

### คำสั่งเกี่ยวกับคลังแพ็กเกจ

คลังเริ่มต้นคือ `https://fu.ling-lang.org` ตั้งค่า `LINGFU_REGISTRY` เพื่อใช้คลังอื่น

| คำสั่ง | หน้าที่ |
|---|---|
| `login <api-key>` | บันทึกคีย์ไว้ที่ `~/.lingfu/credentials.toml` |
| `logout` | ลบข้อมูลยืนยันตัวตนที่บันทึกไว้ |
| `publish` | แพ็กโปรเจกต์เป็น `.tar.gz` (ข้ามไฟล์ที่ระบุใน `ling.ignore`) แล้วอัปโหลด ต้อง `login` ก่อน |
| `search <query>` | ค้นหาในคลัง ถ้าใส่ `--crates` จะค้นหาใน crates.io แทน |
| `add <package>` | ดาวน์โหลดแพ็กเกจลงใน `./ling_modules/<name>/` และพิมพ์บรรทัด `use` สำหรับนำเข้า ถ้าใส่ `--crates` จะรัน `cargo add` |

### `lingfu doc`, `bench`, `update`

`doc` รัน `mdbook build` ถ้ามี `docs/book.toml` หรือ `book/book.toml` ไม่เช่นนั้นจะรัน `cargo doc --no-deps`
ส่วน `bench` และ `update` รัน `cargo bench` และ `cargo update` ซึ่งมีประโยชน์เฉพาะกับโปรเจกต์ที่มี crate ของ Rust

### `lingfu seed`

สร้างลายนิ้วมือความสมบูรณ์แบบเข้ารหัสของโปรเจกต์ git: ราก Merkle แบบ BLAKE3 ของไฟล์ทุกไฟล์ที่ติดตามอยู่
เขียนเป็นไฟล์ `seed.wav` ที่เปิดฟังได้ `--check` จะเทียบโปรเจกต์กับ seed ที่บันทึกไว้ ต้องเป็น repository ของ git

### `lingfu wizard`

แบบสอบถามโต้ตอบ (ภาษา ประเภทโปรเจกต์ ชื่อ ฯลฯ) ที่จบด้วยการสร้างโปรเจกต์แบบเดียวกับ `new`
คำสั่งนี้อ่านจากเทอร์มินัล จึงเขียนเป็นสคริปต์ไม่ได้

## สถานะเมื่อจบการทำงาน

`lingfu` คืนค่า 0 เมื่อสำเร็จ เมื่อล้มเหลวจะพิมพ์ `Error: <message>` (เช่น `build failed` หรือ
`tests failed`) แล้วคืนค่า 1
