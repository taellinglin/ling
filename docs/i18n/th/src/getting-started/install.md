# การติดตั้ง

ติดตั้ง Ling ได้ด้วยคำสั่งเดียว คุณจะได้โปรแกรมสองตัว คือ `ling` สำหรับรันและบิลด์โปรแกรม Ling
และ `lingfu` ซึ่งเป็นตัวจัดการโปรเจกต์

## ติดตั้งด้วยคำสั่งเดียว

```sh
# macOS / Linux
curl --proto '=https' --tlsv1.2 -sSf https://ling-lang.org/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://ling-lang.org/install.ps1 | iex
```

ถ้ายังไม่มีชุดเครื่องมือ Rust สคริปต์จะติดตั้งให้ก่อน (แบบเดียวกับ [rustup](https://rustup.rs))
จากนั้นจึงบิลด์ `ling` และ `lingfu` จาก crates.io

## ติดตั้งด้วย Cargo

ถ้ามี Rust อยู่แล้ว ติดตั้งทั้งสองโปรแกรมจาก crates.io ได้เลย:

```sh
cargo install ling-lang lingfu
```

ทั้งสองจะอยู่ในโฟลเดอร์เดียวกัน (`~/.cargo/bin`) `lingfu` จะหา `ling` ที่อยู่ข้างตัวเองก่อน
ถ้าไม่เจอจึงหาใน `PATH`

## บิลด์จากซอร์สโค้ด

```sh
git clone https://github.com/taellinglin/ling
cd ling
cargo build --release
# ไฟล์โปรแกรมอยู่ใน target/release/ (ling รวมทั้ง lingc, lx และ ling-repl)
```

ถ้าจะบิลด์ `lingfu` จากโค้ดชุดเดียวกัน ให้รัน `cargo build --release -p lingfu`

## ตรวจสอบว่าใช้งานได้

รัน `ling` โดยไม่ใส่อาร์กิวเมนต์ บรรทัดแรกของข้อความวิธีใช้จะบอกเวอร์ชัน:

```sh
ling
# ling 2030.1.53 — The Omniglot Systems Language
# Usage:
#   ling run <file.ling>   ...
lingfu --version
# lingfu v2030.1.19
```

`ling` ไม่มีตัวเลือก `--version` ต้องดูเวอร์ชันจากข้อความวิธีใช้นี้

## โปรไฟล์การติดตั้ง

`cargo install ling-lang` จะติดตั้งทุกอย่าง (โปรไฟล์ `full`) ถ้าอยากบิลด์น้อยลง
ให้ปิดค่าเริ่มต้นแล้วเลือกเฉพาะโปรไฟล์ที่ต้องการ:

```sh
cargo install ling-lang --no-default-features --features core        # เฉพาะคอมไพเลอร์และอินเทอร์พรีเตอร์
cargo install ling-lang --no-default-features --features web         # เพิ่มเซิร์ฟเวอร์/ไคลเอนต์ HTTP และ SQLite
cargo install ling-lang --no-default-features --features game,audio  # ชุดสร้างเกมพร้อมเสียง
```

| โปรไฟล์ | สิ่งที่เพิ่มเข้ามา |
|---|---|
| `core` | ตัวแยกคำ ตัวแจงไวยากรณ์ Cranelift JIT และ AOT อินเทอร์พรีเตอร์ หน้าต่างแบบซอฟต์แวร์และการวาด (`draw_*` ฟอนต์ รูปภาพ เมช) ทุกโปรไฟล์มีส่วนนี้ |
| `web` | เซิร์ฟเวอร์และไคลเอนต์ `http_*` ([เว็บเซิร์ฟเวอร์และ HTTP](../reference/web.md)) SQLite `db_*` ([ฐานข้อมูล (SQLite)](../reference/database.md)) ไฟล์สแตติก TOTP zip/tar การเล่นวิดีโอ โพรเซส แฮชและ `password_hash` |
| `privacy` | ตัวตนแบบหลังควอนตัม `lingtp://` เพียร์ทูเพียร์ `p2p_*` Ling Wallet `lw_*` `ssh_*` รวมทั้ง `web` และวิดเจ็ต `ui_*` |
| `game` | ฟิสิกส์ (วัตถุนิ่ม วัตถุแข็ง ของเหลว glTF) การเรนเดอร์ด้วย GPU แอนิเมชัน กล่องโต้ตอบ จอยเกม วัตถุเสียง วิดเจ็ต `ui_*` |
| `ai` | โครงข่ายประสาทเทียม (`nn_*`) ต้นไม้พฤติกรรม (`bt_*`) บทสนทนาที่เรียนรู้ได้ และการแปลงเสียงเป็นข้อความ (`stt_*`) |
| `audio` | เอนจินเสียง การถอดรหัสเพลง การสังเคราะห์เสียงและ MIDI ไมโครโฟน DAW LingStationX |
| `os` | `ling build --platform kernel`, `rpi`, `lingos`, `lingos-rpi` (อิมเมจ LingOS) |
| `full` | ทั้งหมดข้างต้น (ค่าเริ่มต้น) และยังรวม `art` ซึ่งเป็นเอนจินวาดภาพ LingAnime (`art_*`) |

ถ้าโปรแกรมเรียกฟังก์ชันในตัวจากโปรไฟล์ที่บิลด์ของคุณไม่ได้รวมไว้ โปรแกรมจะหยุดพร้อมข้อผิดพลาด
ที่บอกชื่อโปรไฟล์ที่ต้องติดตั้งใหม่ ส่วนเสริมที่มีขนาดใหญ่บางตัวต้องเปิดเพิ่มเองไม่ว่าจะใช้โปรไฟล์ใด:
`llm`, `llm-cuda`, `vision`, `cuda`, `gpu-cuda`, `gpu`, `llvm`, `emu`

## ความต้องการของระบบ

- Rust 1.85 ขึ้นไป (สคริปต์ติดตั้งจะจัดการให้)
- บน Linux: ต้องมี `libasound2-dev` สำหรับเสียง
- บน macOS และ Windows: ไม่ต้องติดตั้งอะไรเพิ่ม
- `ling build` สำหรับเดสก์ท็อปจะคอมไพล์โปรเจกต์ Rust ขนาดเล็ก จึงต้องมีชุดเครื่องมือ Rust ด้วย
  ดู[คำสั่ง ling](../tools/ling.md)

## ขั้นต่อไป

- เขียนโปรแกรมแรก: [สวัสดีชาวโลก](hello.md)
- เรียนรู้คำสั่งต่าง ๆ: [การรันโปรแกรม](running.md)
- อ่าน[หนังสือ Ling](../books.md) ชุด PDF สี่เล่มที่ดาวน์โหลดได้ฟรี ได้แก่ ระดับเริ่มต้น ระดับกลาง
  ระดับสูง และคู่มืออ้างอิงฉบับสมบูรณ์
