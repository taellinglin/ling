# Files, Time and System

These builtins read and write files, inspect paths, tell the time, read the command line and
environment, start other programs and run work on other threads. Everything here works in a
plain terminal; nothing needs a window.

Paths are text. Relative paths start from the directory you run `ling` in. Most file builtins
report failure with a return value (`false`, `0`, an empty list) rather than stopping the program;
each entry says which.

## Summary

### Files and paths

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `read_file(path)` | — | — | — | `อ่านไฟล์` | Read a UTF-8 text file |
| `write_file(path, text)` | — | — | — | `เขียนไฟล์` | Create or overwrite a text file |
| `read_file_bytes(path)` | `读字节` | — | `바이트읽기` | `อ่านไบต์` | Read any file as a list of bytes |
| `write_file_bytes(path, bytes)` | `写字节` | — | `바이트쓰기` | `เขียนไบต์` | Write a list of bytes to a file |
| `file_write_bytes(path, bytes, append)` | — | — | — | — | Write or append bytes |
| `file_exists(path)` | `文件存在` | — | — | — | Does the path exist? |
| `is_file(path)` | — | — | — | `เป็นไฟล์` | Is it a file? |
| `is_dir(path)` | — | — | — | `เป็นไดเรกทอรี` | Is it a directory? |
| `list_dir(path)` | — | — | — | `รายการไดเรกทอรี` | Paths of the entries in a directory |
| `make_dir(path)` | — | — | — | `สร้างไดเรกทอรี` | Create a directory |
| `copy_file(from, to)` | — | — | — | `คัดลอกไฟล์` | Copy a file |
| `rename_file(from, to)` | `重命名文件` | `ファイル改名` | `파일이름변경` | `เปลี่ยนชื่อไฟล์` | Rename or move a file |
| `remove_file(path)` | `删除文件` | `ファイル削除` | `파일삭제` | `ลบไฟล์` | Delete a file |
| `remove_dir_all(path)` | — | — | — | `ลบไดเรกทอรีทั้งหมด` | Delete a directory and everything in it |
| `file_size(path)` | — | — | — | `ขนาดไฟล์` | Size in bytes |
| `file_modified(path)` | — | — | — | `เวลาที่แก้ไข` | Last change time (Unix seconds) |
| `file_created(path)` | — | — | — | `เวลาที่สร้าง` | Creation time (Unix seconds) |
| `file_magic(path)` | — | — | — | `มายาไฟล์` | Detect `gzip`, `zip`, `png` or `other` |
| `path_name(path)` | — | — | — | `ชื่อไฟล์` | Last part of a path |
| `path_ext(path)` | — | — | — | `นามสกุลไฟล์` | File extension |
| `absolute_path(path)` | — | — | — | `พาธสัมบูรณ์` | Make a path absolute |

### Time, environment and processes

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `time_now()` | `当前时间` | `経過時間` | `현재시간` | `เวลาปัจจุบัน` | Seconds since the program started |
| `epoch_now()` | `现在时刻` | — | `현재시각` | `เวลาโลก` | Seconds since 1970-01-01 UTC (alias `now_unix`) |
| `random_hex(bytes)` | — | — | `무작위16진수` | — | Secure random bytes as hex text |
| `get_args()` | — | — | — | `รับอาร์กิวเมนต์` | The whole command line as a list |
| `cli_arg(name, default)` | — | — | — | `อาร์กิวเมนต์` | Value of a `--name value` option |
| `env_get(name, default)` | — | — | — | `รับตัวแปรแวดล้อม` | Read an environment variable |
| `run_process(cmd, args, timeout, log)` | — | — | — | `รันโปรเซส` | Run a program and wait for it |
| `proc_start(cmd, args, log)` | — | — | — | — | Start a program in the background |
| `proc_poll(id)` | — | — | — | — | Exit code, or `-2` while running |
| `proc_kill(id)` | — | — | — | — | Stop a background program |

## Files

### Reading and writing text

`write_file` creates or overwrites a file. `read_file` returns its contents as text. A missing
file is an error on the interpreter, but returns `()` on the default backend, so check with
`file_exists` first.

```ling
bind start = do {
  write_file("notes.txt", "line one\nline two")
  print(file_exists("notes.txt"), " ", file_size("notes.txt"))
  if file_exists("notes.txt") {
    bind lines = split(read_file("notes.txt"), "\n")
    print(len(lines), " lines, first: ", lines[0])
  }
  print(remove_file("notes.txt"), " ", remove_file("notes.txt"))
}
```
```output
true 17
2 lines, first: line one
true false
```

`remove_file` returns `false` when there was nothing to delete.

### Bytes

For binary files, `read_file_bytes` returns a list of numbers 0 to 255 (an empty list on error)
and `write_file_bytes` writes one back, returning `true` on success. `file_write_bytes(path,
bytes, true)` appends instead of overwriting.

```ling
bind start = do {
  print(write_file_bytes("hi.bin", [72, 105]))
  print(read_file_bytes("hi.bin"))
  file_write_bytes("hi.bin", [33], true)
  print(read_file("hi.bin"))
  print(file_magic("hi.bin"), " ", file_magic("nothing.bin"))
}
```
```output
true
[72, 105]
Hi!
other missing
```

### Directories

`make_dir` creates a directory (and any missing parents) and returns `true` on success.
`list_dir` returns each entry as a path that starts with the directory you passed. `copy_file` and
`rename_file` return `true` on success.

```ling
bind start = do {
  make_dir("out")
  write_file("out/a.txt", "A")
  copy_file("out/a.txt", "out/b.txt")
  rename_file("out/b.txt", "out/c.txt")
  print(list_dir("out"))
  print(is_dir("out"), " ", is_file("out"))
  print(remove_dir_all("out"), " ", file_exists("out"))
}
```
```output
[out/a.txt, out/c.txt]
true false
true false
```

`remove_dir_all` deletes the directory and everything inside it, with no undo. A path that does
not exist counts as success.

### Paths

`path_name` and `path_ext` return the last part of a path and its extension (empty text if there
is none). `absolute_path` puts the current directory in front of a relative path; it does not
check that the path exists.

```ling
bind start = do {
  print(path_name("photos/cat.png"), " ", path_ext("photos/cat.png"))
  print(path_ext("README"), "|")
}
```
```output
cat.png png
|
```

## Time

### `time_now()` and `epoch_now()`

`time_now` counts seconds (with fractions) since the program started, which is what you want for
timing and animation. `epoch_now` is the wall-clock time as Unix seconds. There are no
date-formatting builtins.

```ling
bind start = do {
  bind t0 = time_now()
  sleep(20)
  print(time_now() - t0 >= 0.02)
  print(epoch_now() > 1700000000)
}
```
```output
true
true
```

### `random_hex(bytes)`

Returns `bytes` cryptographically secure random bytes as lower-case hex (default 16 bytes, so 32
characters). Use it for tokens and IDs; use `rand()` for games.

```ling
bind start = do {
  print(len(random_hex(16)))
  print(random_hex(4) == random_hex(4))
}
```
```output
32
false
```

## Command line and environment

### `cli_arg(name, default)` and `get_args()`

`cli_arg("port", "8080")` returns the value after `--port` on the command line, or the default.
Only the two-word form `--port 9000` is understood. `get_args()` returns the whole command line:
for `ling run app.ling a b` it starts with the `ling` executable, then `run`, then the script, then
your arguments.

```ling
bind start = do {
  print(cli_arg("port", "8080"))
  print(get_args()[1])
}
```
```output
8080
run
```

### `env_get(name, default)`

Reads an environment variable, or returns the default (empty text if you leave it out).

```ling
bind start = do {
  print(env_get("MY_APP_MODE", "development"))
}
```
```output
development
```

## Processes

`run_process(cmd, args, timeout, log)` runs a program directly (no shell, so no quoting problems
or injection), appends its output to the `log` file and waits. It returns the exit code, `-1` if
the program could not start, or `-2` if it was killed after `timeout` seconds.

`proc_start(cmd, args, log)` starts the program without waiting and returns an id (`-1` on
failure). `proc_poll(id)` returns `-2` while it runs and the exit code afterwards; `proc_kill(id)`
stops it.

```ling-gui
bind start = do {
  print(run_process("git", ["--version"], 10, "git.log"))
  print(trim(read_file("git.log")))
  bind id = proc_start("git", ["status"], "status.log")
  while proc_poll(id) == -2 { sleep(100) }
  print("finished")
}
```

The process builtins belong to the `web` [install profile](index.md). In a build without
it they fail with an error that tells you how to reinstall.

## Threads and channels

Ling threads are isolates: each runs its own interpreter, and they share nothing except channels
and a write-once store, so they cannot race. Values sent between them are copied; functions cannot
be sent.

| Builtin | Purpose |
|---|---|
| `thread_spawn(file, fn_name, args, caps)` | Run a function from another `.ling` file on a new thread; returns `ok(handle)` |
| `thread_join(handle)` | Wait for the thread; returns `ok(value)` or `bad(error)` |
| `thread_cpus()` | Number of hardware threads |
| `chan_new(capacity)` | New channel as `[send_handle, receive_handle]` |
| `chan_send(send, value)` | Send a value (blocks while full) |
| `chan_recv(receive)` | Receive the next value as `ok(value)`, or `bad("closed")` |
| `chan_close(handle)` | Close a channel end |
| `chan_len(handle)` | Number of queued messages |
| `shared_put(key, value)` | Store a value once; `bad("exists")` if the key is taken |
| `shared_get(key, default)` | Read a stored value |

A channel works inside a single program too:

```ling
bind start = do {
  bind ch = chan_new(4)
  chan_send(ch[0], "hello")
  chan_send(ch[0], 42)
  print(chan_len(ch[1]))
  print(chan_recv(ch[1]), " ", chan_recv(ch[1]))
  chan_close(ch[0])
  print(chan_recv(ch[1]))
  print(shared_put("config", [1, 2]), " ", shared_put("config", 0))
  print(shared_get("config", []))
}
```
```output
2
Ok(hello) Ok(42)
Err(closed)
Ok(()) Err(exists)
[1, 2]
```

`thread_spawn` loads the function from a separate file, resolved relative to the calling file.
`caps` lists what the thread may do, from `fs db net p2p proc keys clipboard`; a thread never gets
a capability its parent lacks. Unwrap the handle with `match` before joining:

```ling-module
// main.ling, next to worker.ling, which defines: fn square(n) { n * n }
fn square_on_thread(n) {
  bind h = match thread_spawn("worker.ling", "square", [n], []) { ok(v) => v, bad(e) => e }
  thread_join(h)
}
```
