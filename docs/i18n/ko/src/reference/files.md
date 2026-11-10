# 파일, 시간, 시스템

이 내장 함수들은 파일을 읽고 쓰고, 경로를 살피고, 시간을 알려 주고, 명령줄과 환경 변수를 읽고, 다른 프로그램을 실행하고,
다른 스레드에서 작업을 돌립니다. 모두 일반 터미널에서 동작하며 창이 필요 없습니다.

경로는 텍스트입니다. 상대 경로는 `ling`을 실행한 디렉터리에서 시작합니다. 대부분의 파일 내장 함수는 프로그램을 멈추지 않고
반환값(`false`, `0`, 빈 리스트)으로 실패를 알립니다. 어떤 값인지는 항목마다 적혀 있습니다.

## 요약

### 파일과 경로

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `read_file(path)` | — | — | — | `อ่านไฟล์` | UTF-8 텍스트 파일을 읽습니다 |
| `write_file(path, text)` | — | — | — | `เขียนไฟล์` | 텍스트 파일을 만들거나 덮어씁니다 |
| `read_file_bytes(path)` | `读字节` | — | `바이트읽기` | `อ่านไบต์` | 어떤 파일이든 바이트 리스트로 읽습니다 |
| `write_file_bytes(path, bytes)` | `写字节` | — | `바이트쓰기` | `เขียนไบต์` | 바이트 리스트를 파일에 씁니다 |
| `file_write_bytes(path, bytes, append)` | — | — | — | — | 바이트를 쓰거나 덧붙입니다 |
| `file_exists(path)` | `文件存在` | — | — | — | 경로가 있는지 |
| `is_file(path)` | — | — | — | `เป็นไฟล์` | 파일인지 |
| `is_dir(path)` | — | — | — | `เป็นไดเรกทอรี` | 디렉터리인지 |
| `list_dir(path)` | — | — | — | `รายการไดเรกทอรี` | 디렉터리 안 항목들의 경로 |
| `make_dir(path)` | — | — | — | `สร้างไดเรกทอรี` | 디렉터리를 만듭니다 |
| `copy_file(from, to)` | — | — | — | `คัดลอกไฟล์` | 파일을 복사합니다 |
| `rename_file(from, to)` | `重命名文件` | `ファイル改名` | `파일이름변경` | `เปลี่ยนชื่อไฟล์` | 파일 이름을 바꾸거나 옮깁니다 |
| `remove_file(path)` | `删除文件` | `ファイル削除` | `파일삭제` | `ลบไฟล์` | 파일을 지웁니다 |
| `remove_dir_all(path)` | — | — | — | `ลบไดเรกทอรีทั้งหมด` | 디렉터리를 안의 내용까지 모두 지웁니다 |
| `file_size(path)` | — | — | — | `ขนาดไฟล์` | 바이트 단위 크기 |
| `file_modified(path)` | — | — | — | `เวลาที่แก้ไข` | 마지막 수정 시각(유닉스 초) |
| `file_created(path)` | — | — | — | `เวลาที่สร้าง` | 생성 시각(유닉스 초) |
| `file_magic(path)` | — | — | — | `มายาไฟล์` | `gzip`, `zip`, `png`, `other`를 판별합니다 |
| `path_name(path)` | — | — | — | `ชื่อไฟล์` | 경로의 마지막 부분 |
| `path_ext(path)` | — | — | — | `นามสกุลไฟล์` | 확장자 |
| `absolute_path(path)` | — | — | — | `พาธสัมบูรณ์` | 절대 경로로 만듭니다 |

### 시간, 환경, 프로세스

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `time_now()` | `当前时间` | `経過時間` | `현재시간` | `เวลาปัจจุบัน` | 프로그램 시작 후 지난 초 |
| `epoch_now()` | `现在时刻` | — | `현재시각` | `เวลาโลก` | 1970-01-01 UTC부터의 초(별칭 `now_unix`) |
| `random_hex(bytes)` | — | — | `무작위16진수` | — | 안전한 난수 바이트를 16진 텍스트로 |
| `get_args()` | — | — | — | `รับอาร์กิวเมนต์` | 명령줄 전체를 담은 리스트 |
| `cli_arg(name, default)` | — | — | — | `อาร์กิวเมนต์` | `--name value` 옵션의 값 |
| `env_get(name, default)` | — | — | — | `รับตัวแปรแวดล้อม` | 환경 변수를 읽습니다 |
| `run_process(cmd, args, timeout, log)` | — | — | — | `รันโปรเซส` | 프로그램을 실행하고 끝날 때까지 기다립니다 |
| `proc_start(cmd, args, log)` | — | — | — | — | 프로그램을 백그라운드에서 시작합니다 |
| `proc_poll(id)` | — | — | — | — | 종료 코드, 실행 중이면 `-2` |
| `proc_kill(id)` | — | — | — | — | 백그라운드 프로그램을 멈춥니다 |

## 파일

### 텍스트 읽고 쓰기

`write_file`은 파일을 만들거나 덮어쓰고, `read_file`은 내용을 텍스트로 돌려줍니다. 파일이 없으면 인터프리터에서는 오류이고
기본 백엔드에서는 `()`를 돌려주므로, 먼저 `file_exists`로 확인하세요.

```ling
바인드 start = 실행 {
  write_file("notes.txt", "첫째 줄\n둘째 줄")
  출력(file_exists("notes.txt"), " ", file_size("notes.txt"))
  만약 file_exists("notes.txt") {
    바인드 줄들 = split(read_file("notes.txt"), "\n")
    출력(길이(줄들), "줄, 첫 줄: ", 줄들[0])
  }
  출력(파일삭제("notes.txt"), " ", 파일삭제("notes.txt"))
}
```
```output
true 21
2줄, 첫 줄: 첫째 줄
true false
```

지울 것이 없으면 `remove_file`은 `false`를 돌려줍니다.

### 바이트

바이너리 파일에는 0부터 255까지의 숫자 리스트를 돌려주는 `read_file_bytes`(오류 시 빈 리스트)와,
그 리스트를 다시 써서 성공하면 `true`를 돌려주는 `write_file_bytes`를 씁니다. `file_write_bytes(path, bytes, true)`는 덮어쓰지 않고 덧붙입니다.

```ling
바인드 start = 실행 {
  출력(바이트쓰기("hi.bin", [72, 105]))
  출력(바이트읽기("hi.bin"))
  file_write_bytes("hi.bin", [33], 참)
  출력(read_file("hi.bin"))
  출력(file_magic("hi.bin"), " ", file_magic("nothing.bin"))
}
```
```output
true
[72, 105]
Hi!
other missing
```

### 디렉터리

`make_dir`는 디렉터리를(없는 상위 디렉터리까지) 만들고 성공하면 `true`를 돌려줍니다. `list_dir`는 각 항목을
넘겨준 디렉터리로 시작하는 경로로 돌려줍니다. `copy_file`과 `rename_file`은 성공하면 `true`를 돌려줍니다.

```ling
바인드 start = 실행 {
  make_dir("out")
  write_file("out/a.txt", "가")
  copy_file("out/a.txt", "out/b.txt")
  파일이름변경("out/b.txt", "out/c.txt")
  출력(list_dir("out"))
  출력(is_dir("out"), " ", is_file("out"))
  출력(remove_dir_all("out"), " ", file_exists("out"))
}
```
```output
[out/a.txt, out/c.txt]
true false
true false
```

`remove_dir_all`은 디렉터리와 그 안의 모든 것을 지우며 되돌릴 수 없습니다. 없는 경로도 성공으로 칩니다.

### 경로

`path_name`과 `path_ext`는 경로의 마지막 부분과 확장자를 돌려줍니다(없으면 빈 텍스트). `absolute_path`는 상대 경로 앞에
현재 디렉터리를 붙입니다. 경로가 실제로 있는지는 확인하지 않습니다.

```ling
바인드 start = 실행 {
  출력(path_name("photos/cat.png"), " ", path_ext("photos/cat.png"))
  출력(path_ext("README"), "|")
}
```
```output
cat.png png
|
```

## 시간

### `time_now()`와 `epoch_now()`

`time_now`는 프로그램이 시작된 뒤 지난 초(소수 포함)를 세므로 시간 측정과 애니메이션에 알맞습니다. `epoch_now`는 유닉스 초로 나타낸 실제 시각입니다.
날짜를 서식화하는 내장 함수는 없습니다.

```ling
바인드 start = 실행 {
  바인드 시작 = 현재시간()
  잠자기(20)
  출력(현재시간() - 시작 >= 0.02)
  출력(현재시각() > 1700000000)
}
```
```output
true
true
```

### `random_hex(bytes)`

암호학적으로 안전한 난수 바이트 `bytes`개를 소문자 16진수로 돌려줍니다(기본 16바이트, 즉 32글자).
토큰과 ID에 쓰고, 게임에는 `rand()`를 쓰세요.

```ling
바인드 start = 실행 {
  출력(길이(무작위16진수(16)))
  출력(무작위16진수(4) == 무작위16진수(4))
}
```
```output
32
false
```

## 명령줄과 환경

### `cli_arg(name, default)`와 `get_args()`

`cli_arg("port", "8080")`는 명령줄에서 `--port` 뒤에 오는 값을, 없으면 기본값을 돌려줍니다. 두 단어 형식 `--port 9000`만 이해합니다.
`get_args()`는 명령줄 전체를 돌려줍니다. `ling run app.ling a b`라면 `ling` 실행 파일, `run`, 스크립트, 그리고 여러분의 인수 순서입니다.

```ling
바인드 start = 실행 {
  출력(cli_arg("port", "8080"))
  출력(get_args()[1])
}
```
```output
8080
run
```

### `env_get(name, default)`

환경 변수를 읽고, 없으면 기본값(생략하면 빈 텍스트)을 돌려줍니다.

```ling
바인드 start = 실행 {
  출력(env_get("MY_APP_MODE", "개발"))
}
```
```output
개발
```

## 프로세스

`run_process(cmd, args, timeout, log)`는 프로그램을 직접 실행하고(셸을 거치지 않으므로 따옴표 문제나 인젝션이 없습니다)
출력을 `log` 파일에 덧붙인 뒤 끝날 때까지 기다립니다. 종료 코드를 돌려주며, 시작하지 못하면 `-1`, `timeout`초가 지나 강제 종료되면 `-2`입니다.

`proc_start(cmd, args, log)`는 기다리지 않고 프로그램을 시작해 id를 돌려줍니다(실패하면 `-1`). `proc_poll(id)`는 실행 중에는 `-2`,
끝나면 종료 코드를 돌려주고, `proc_kill(id)`는 프로그램을 멈춥니다.

```ling-gui
바인드 start = 실행 {
  출력(run_process("git", ["--version"], 10, "git.log"))
  출력(trim(read_file("git.log")))
  바인드 번호 = proc_start("git", ["status"], "status.log")
  동안 proc_poll(번호) == -2 { 잠자기(100) }
  출력("완료")
}
```

프로세스 내장 함수는 `web` [프로필](index.md)에 속합니다. 이 프로필이 빠진 빌드에서는 다시 설치하는 방법을 알려 주는 오류가 납니다.

## 스레드와 채널

Ling의 스레드는 아이솔레이트입니다. 각자 자기 인터프리터를 돌리고, 채널과 한 번만 쓸 수 있는 저장소 말고는 아무것도 공유하지 않으므로
경쟁 상태가 생기지 않습니다. 스레드 사이에 보내는 값은 복사되며, 함수는 보낼 수 없습니다.

| Builtin | Purpose |
|---|---|
| `thread_spawn(file, fn_name, args, caps)` | 다른 `.ling` 파일의 함수를 새 스레드에서 실행하고 `ok(handle)`을 돌려줍니다 |
| `thread_join(handle)` | 스레드가 끝나기를 기다려 `ok(value)`나 `bad(error)`를 돌려줍니다 |
| `thread_cpus()` | 하드웨어 스레드 수 |
| `chan_new(capacity)` | 새 채널, `[send_handle, receive_handle]` 형태 |
| `chan_send(send, value)` | 값을 보냅니다(가득 차 있으면 기다림) |
| `chan_recv(receive)` | 다음 값을 `ok(value)`로 받습니다. 닫혔으면 `bad("closed")` |
| `chan_close(handle)` | 채널 한쪽 끝을 닫습니다 |
| `chan_len(handle)` | 대기 중인 메시지 수 |
| `shared_put(key, value)` | 값을 한 번만 저장합니다. 키가 이미 있으면 `bad("exists")` |
| `shared_get(key, default)` | 저장한 값을 읽습니다 |

채널은 프로그램 하나 안에서도 쓸 수 있습니다.

```ling
바인드 start = 실행 {
  바인드 통로 = chan_new(4)
  chan_send(통로[0], "안녕")
  chan_send(통로[0], 42)
  출력(chan_len(통로[1]))
  출력(chan_recv(통로[1]), " ", chan_recv(통로[1]))
  chan_close(통로[0])
  출력(chan_recv(통로[1]))
  출력(shared_put("설정", [1, 2]), " ", shared_put("설정", 0))
  출력(shared_get("설정", []))
}
```
```output
2
Ok(안녕) Ok(42)
Err(closed)
Ok(()) Err(exists)
[1, 2]
```

`thread_spawn`은 함수를 별도 파일에서 불러오며, 경로는 호출하는 파일 기준입니다. `caps`에는 그 스레드에 허용할 일을
`fs db net p2p proc keys clipboard` 중에서 골라 넣습니다. 스레드는 부모에게 없는 권한을 절대 얻지 못합니다. join하기 전에 `match`로 핸들을 꺼내세요.

```ling-module
// main.ling. 옆의 worker.ling에는 다음이 정의되어 있습니다: 함수 제곱(n) { n * n }
함수 스레드에서제곱(n) {
  바인드 핸들 = 매치 thread_spawn("worker.ling", "제곱", [n], []) { 좋아(v) => v, 나쁨(e) => e }
  thread_join(핸들)
}
```
