# 文件、时间与系统

这些内置函数用来读写文件、检查路径、获取时间、读取命令行和环境变量、启动其他程序，以及在其他线程上运行任务。
这里的一切都能在普通终端里使用，不需要窗口。

路径就是文本。相对路径从你运行 `ling` 的目录开始算。大多数文件内置函数通过返回值（`false`、`0`、空列表）
报告失败，而不会让程序停下来；每个条目都会说明是哪一种。

## 一览

### 文件与路径

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `read_file(path)` | — | — | — | `อ่านไฟล์` | 读取 UTF-8 文本文件 |
| `write_file(path, text)` | — | — | — | `เขียนไฟล์` | 创建或覆盖文本文件 |
| `read_file_bytes(path)` | `读字节` | — | `바이트읽기` | `อ่านไบต์` | 把任意文件读成字节列表 |
| `write_file_bytes(path, bytes)` | `写字节` | — | `바이트쓰기` | `เขียนไบต์` | 把字节列表写入文件 |
| `file_write_bytes(path, bytes, append)` | — | — | — | — | 写入或追加字节 |
| `file_exists(path)` | `文件存在` | — | — | — | 路径是否存在 |
| `is_file(path)` | — | — | — | `เป็นไฟล์` | 是否为文件 |
| `is_dir(path)` | — | — | — | `เป็นไดเรกทอรี` | 是否为目录 |
| `list_dir(path)` | — | — | — | `รายการไดเรกทอรี` | 目录中各条目的路径 |
| `make_dir(path)` | — | — | — | `สร้างไดเรกทอรี` | 创建目录 |
| `copy_file(from, to)` | — | — | — | `คัดลอกไฟล์` | 复制文件 |
| `rename_file(from, to)` | `重命名文件` | `ファイル改名` | `파일이름변경` | `เปลี่ยนชื่อไฟล์` | 重命名或移动文件 |
| `remove_file(path)` | `删除文件` | `ファイル削除` | `파일삭제` | `ลบไฟล์` | 删除文件 |
| `remove_dir_all(path)` | — | — | — | `ลบไดเรกทอรีทั้งหมด` | 删除目录及其中的一切 |
| `file_size(path)` | — | — | — | `ขนาดไฟล์` | 以字节计的大小 |
| `file_modified(path)` | — | — | — | `เวลาที่แก้ไข` | 最后修改时间（Unix 秒） |
| `file_created(path)` | — | — | — | `เวลาที่สร้าง` | 创建时间（Unix 秒） |
| `file_magic(path)` | — | — | — | `มายาไฟล์` | 识别 `gzip`、`zip`、`png` 或 `other` |
| `path_name(path)` | — | — | — | `ชื่อไฟล์` | 路径的最后一部分 |
| `path_ext(path)` | — | — | — | `นามสกุลไฟล์` | 文件扩展名 |
| `absolute_path(path)` | — | — | — | `พาธสัมบูรณ์` | 转为绝对路径 |

### 时间、环境与进程

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `time_now()` | `当前时间` | `経過時間` | `현재시간` | `เวลาปัจจุบัน` | 程序启动以来的秒数 |
| `epoch_now()` | `现在时刻` | — | `현재시각` | `เวลาโลก` | 自 1970-01-01 UTC 起的秒数（别名 `now_unix`） |
| `random_hex(bytes)` | — | — | `무작위16진수` | — | 以十六进制文本表示的安全随机字节 |
| `get_args()` | — | — | — | `รับอาร์กิวเมนต์` | 整个命令行组成的列表 |
| `cli_arg(name, default)` | — | — | — | `อาร์กิวเมนต์` | `--name value` 选项的值 |
| `env_get(name, default)` | — | — | — | `รับตัวแปรแวดล้อม` | 读取环境变量 |
| `run_process(cmd, args, timeout, log)` | — | — | — | `รันโปรเซส` | 运行程序并等待其结束 |
| `proc_start(cmd, args, log)` | — | — | — | — | 在后台启动程序 |
| `proc_poll(id)` | — | — | — | — | 退出码；仍在运行时为 `-2` |
| `proc_kill(id)` | — | — | — | — | 停止后台程序 |

## 文件

### 读写文本

`write_file` 创建或覆盖文件，`read_file` 把内容作为文本返回。文件不存在时，在解释器上是错误，
在默认后端上则返回 `()`，所以请先用 `file_exists` 检查。

```ling
令 start = 执 {
  write_file("notes.txt", "第一行\n第二行")
  印(文件存在("notes.txt"), " ", file_size("notes.txt"))
  若 文件存在("notes.txt") {
    令 各行 = split(read_file("notes.txt"), "\n")
    印(长度(各行), " 行，第一行：", 各行[0])
  }
  印(删除文件("notes.txt"), " ", 删除文件("notes.txt"))
}
```
```output
true 19
2 行，第一行：第一行
true false
```

没有东西可删时，`remove_file` 返回 `false`。

### 字节

处理二进制文件时，`read_file_bytes` 返回由 0 到 255 的数字组成的列表（出错时为空列表），
`write_file_bytes` 把这样的列表写回文件，成功时返回 `true`。`file_write_bytes(path, bytes, true)` 会追加而不是覆盖。

```ling
令 start = 执 {
  印(写字节("hi.bin", [72, 105]))
  印(读字节("hi.bin"))
  file_write_bytes("hi.bin", [33], 真)
  印(read_file("hi.bin"))
  印(file_magic("hi.bin"), " ", file_magic("nothing.bin"))
}
```
```output
true
[72, 105]
Hi!
other missing
```

### 目录

`make_dir` 创建目录（包括缺失的上级目录），成功时返回 `true`。`list_dir` 返回的每个条目都是以你传入的目录开头的路径。
`copy_file` 和 `rename_file` 成功时返回 `true`。

```ling
令 start = 执 {
  make_dir("out")
  write_file("out/a.txt", "甲")
  copy_file("out/a.txt", "out/b.txt")
  重命名文件("out/b.txt", "out/c.txt")
  印(list_dir("out"))
  印(is_dir("out"), " ", is_file("out"))
  印(remove_dir_all("out"), " ", 文件存在("out"))
}
```
```output
[out/a.txt, out/c.txt]
true false
true false
```

`remove_dir_all` 会删除目录及其中的一切，无法撤销。不存在的路径也算成功。

### 路径

`path_name` 和 `path_ext` 返回路径的最后一部分及其扩展名（没有时为空文本）。`absolute_path` 在相对路径前加上当前目录；
它不检查路径是否存在。

```ling
令 start = 执 {
  印(path_name("photos/cat.png"), " ", path_ext("photos/cat.png"))
  印(path_ext("README"), "|")
}
```
```output
cat.png png
|
```

## 时间

### `time_now()` 与 `epoch_now()`

`time_now` 计算程序启动以来的秒数（带小数），适合计时和动画。`epoch_now` 是以 Unix 秒表示的实际时间。
没有格式化日期的内置函数。

```ling
令 start = 执 {
  令 起点 = 当前时间()
  睡眠(20)
  印(当前时间() - 起点 >= 0.02)
  印(现在时刻() > 1700000000)
}
```
```output
true
true
```

### `random_hex(bytes)`

返回 `bytes` 个加密安全的随机字节，以小写十六进制表示（默认 16 字节，即 32 个字符）。
用于令牌和 ID；做游戏请用 `rand()`。

```ling
令 start = 执 {
  印(长度(random_hex(16)))
  印(random_hex(4) == random_hex(4))
}
```
```output
32
false
```

## 命令行与环境

### `cli_arg(name, default)` 与 `get_args()`

`cli_arg("port", "8080")` 返回命令行中 `--port` 后面的值，没有时返回默认值。只认两个词的形式 `--port 9000`。
`get_args()` 返回整个命令行：对于 `ling run app.ling a b`，它以 `ling` 可执行文件开头，然后是 `run`、脚本，最后是你的参数。

```ling
令 start = 执 {
  印(cli_arg("port", "8080"))
  印(get_args()[1])
}
```
```output
8080
run
```

### `env_get(name, default)`

读取环境变量，没有时返回默认值（省略时为空文本）。

```ling
令 start = 执 {
  印(env_get("MY_APP_MODE", "开发"))
}
```
```output
开发
```

## 进程

`run_process(cmd, args, timeout, log)` 直接运行程序（不经过 shell，所以没有引号问题，也不会被注入），
把输出追加到 `log` 文件并等待结束。它返回退出码；程序无法启动时返回 `-1`，超过 `timeout` 秒被终止时返回 `-2`。

`proc_start(cmd, args, log)` 启动程序但不等待，返回一个 id（失败时为 `-1`）。`proc_poll(id)` 在程序运行时返回 `-2`，
结束后返回退出码；`proc_kill(id)` 停止它。

```ling-gui
令 start = 执 {
  印(run_process("git", ["--version"], 10, "git.log"))
  印(trim(read_file("git.log")))
  令 编号 = proc_start("git", ["status"], "status.log")
  循 proc_poll(编号) == -2 { 睡眠(100) }
  印("完成")
}
```

进程相关的内置函数属于 `web` [安装配置](index.md)。在不含它的构建中调用会报错，错误信息会告诉你如何重新安装。

## 线程与通道

Ling 的线程是隔离体（isolate）：每个线程运行自己的解释器，彼此之间除了通道和一个只能写一次的存储区外什么都不共享，
因此不会出现竞争。线程之间传递的值会被复制；函数不能传递。

| Builtin | Purpose |
|---|---|
| `thread_spawn(file, fn_name, args, caps)` | 在新线程上运行另一个 `.ling` 文件中的函数；返回 `ok(handle)` |
| `thread_join(handle)` | 等待线程结束；返回 `ok(value)` 或 `bad(error)` |
| `thread_cpus()` | 硬件线程数 |
| `chan_new(capacity)` | 新通道，形式为 `[send_handle, receive_handle]` |
| `chan_send(send, value)` | 发送一个值（满时阻塞） |
| `chan_recv(receive)` | 接收下一个值，得到 `ok(value)` 或 `bad("closed")` |
| `chan_close(handle)` | 关闭通道的一端 |
| `chan_len(handle)` | 排队中的消息数 |
| `shared_put(key, value)` | 存入一个值，只能一次；键已被占用时为 `bad("exists")` |
| `shared_get(key, default)` | 读取已存入的值 |

通道在单个程序里也能用：

```ling
令 start = 执 {
  令 道 = chan_new(4)
  chan_send(道[0], "你好")
  chan_send(道[0], 42)
  印(chan_len(道[1]))
  印(chan_recv(道[1]), " ", chan_recv(道[1]))
  chan_close(道[0])
  印(chan_recv(道[1]))
  印(shared_put("配置", [1, 2]), " ", shared_put("配置", 0))
  印(shared_get("配置", []))
}
```
```output
2
Ok(你好) Ok(42)
Err(closed)
Ok(()) Err(exists)
[1, 2]
```

`thread_spawn` 从单独的文件加载函数，路径相对于调用它的文件。`caps` 列出该线程可以做的事，取自
`fs db net p2p proc keys clipboard`；线程永远得不到父线程没有的权限。在 join 之前先用 `match` 取出句柄：

```ling-module
// main.ling，与 worker.ling 放在一起，后者定义了：函 平方(n) { n * n }
函 在线程上求平方(n) {
  令 句柄 = 配 thread_spawn("worker.ling", "平方", [n], []) { 好(v) => v, 坏(e) => e }
  thread_join(句柄)
}
```
