# ファイル・時刻・システム

この組み込み関数群は、ファイルの読み書き、パスの確認、時刻の取得、コマンドラインと環境変数の読み取り、ほかのプログラムの起動、
別スレッドでの処理を担当します。どれも普通のターミナルで使え、ウィンドウは必要ありません。

パスはテキストです。相対パスは `ling` を実行したディレクトリが起点です。ほとんどのファイル系組み込み関数は、プログラムを止めずに
戻り値（`false`、`0`、空のリスト）で失敗を知らせます。どれになるかは各項目に書いてあります。

## 一覧

### ファイルとパス

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `read_file(path)` | — | — | — | `อ่านไฟล์` | UTF-8 のテキストファイルを読みます |
| `write_file(path, text)` | — | — | — | `เขียนไฟล์` | テキストファイルを作成または上書きします |
| `read_file_bytes(path)` | `读字节` | — | `바이트읽기` | `อ่านไบต์` | 任意のファイルをバイトのリストとして読みます |
| `write_file_bytes(path, bytes)` | `写字节` | — | `바이트쓰기` | `เขียนไบต์` | バイトのリストをファイルに書きます |
| `file_write_bytes(path, bytes, append)` | — | — | — | — | バイトを書き込むか追記します |
| `file_exists(path)` | `文件存在` | — | — | — | パスが存在するか |
| `is_file(path)` | — | — | — | `เป็นไฟล์` | ファイルか |
| `is_dir(path)` | — | — | — | `เป็นไดเรกทอรี` | ディレクトリか |
| `list_dir(path)` | — | — | — | `รายการไดเรกทอรี` | ディレクトリ内の項目のパス |
| `make_dir(path)` | — | — | — | `สร้างไดเรกทอรี` | ディレクトリを作ります |
| `copy_file(from, to)` | — | — | — | `คัดลอกไฟล์` | ファイルをコピーします |
| `rename_file(from, to)` | `重命名文件` | `ファイル改名` | `파일이름변경` | `เปลี่ยนชื่อไฟล์` | ファイル名の変更・移動 |
| `remove_file(path)` | `删除文件` | `ファイル削除` | `파일삭제` | `ลบไฟล์` | ファイルを削除します |
| `remove_dir_all(path)` | — | — | — | `ลบไดเรกทอรีทั้งหมด` | ディレクトリを中身ごと削除します |
| `file_size(path)` | — | — | — | `ขนาดไฟล์` | バイト単位のサイズ |
| `file_modified(path)` | — | — | — | `เวลาที่แก้ไข` | 最終更新時刻（Unix 秒） |
| `file_created(path)` | — | — | — | `เวลาที่สร้าง` | 作成時刻（Unix 秒） |
| `file_magic(path)` | — | — | — | `มายาไฟล์` | `gzip`、`zip`、`png`、`other` を判別します |
| `path_name(path)` | — | — | — | `ชื่อไฟล์` | パスの最後の部分 |
| `path_ext(path)` | — | — | — | `นามสกุลไฟล์` | 拡張子 |
| `absolute_path(path)` | — | — | — | `พาธสัมบูรณ์` | 絶対パスにします |

### 時刻・環境・プロセス

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `time_now()` | `当前时间` | `経過時間` | `현재시간` | `เวลาปัจจุบัน` | プログラム開始からの秒数 |
| `epoch_now()` | `现在时刻` | — | `현재시각` | `เวลาโลก` | 1970-01-01 UTC からの秒数（別名 `now_unix`） |
| `random_hex(bytes)` | — | — | `무작위16진수` | — | 安全な乱数バイトを 16 進テキストで |
| `get_args()` | — | — | — | `รับอาร์กิวเมนต์` | コマンドライン全体のリスト |
| `cli_arg(name, default)` | — | — | — | `อาร์กิวเมนต์` | `--name value` オプションの値 |
| `env_get(name, default)` | — | — | — | `รับตัวแปรแวดล้อม` | 環境変数を読みます |
| `run_process(cmd, args, timeout, log)` | — | — | — | `รันโปรเซส` | プログラムを実行して終了を待ちます |
| `proc_start(cmd, args, log)` | — | — | — | — | プログラムをバックグラウンドで起動します |
| `proc_poll(id)` | — | — | — | — | 終了コード。実行中は `-2` |
| `proc_kill(id)` | — | — | — | — | バックグラウンドのプログラムを止めます |

## ファイル

### テキストの読み書き

`write_file` はファイルを作成または上書きし、`read_file` は中身をテキストで返します。ファイルがないと、インタプリタではエラー、
既定のバックエンドでは `()` になるので、先に `file_exists` で確かめてください。

```ling
束縛 start = 実行 {
  write_file("notes.txt", "1 行目\n2 行目")
  印刷(file_exists("notes.txt"), " ", file_size("notes.txt"))
  もし file_exists("notes.txt") {
    束縛 行 = split(read_file("notes.txt"), "\n")
    印刷(長さ(行), " 行、最初の行：", 行[0])
  }
  印刷(ファイル削除("notes.txt"), " ", ファイル削除("notes.txt"))
}
```
```output
true 17
2 行、最初の行：1 行目
true false
```

削除するものがなければ、`remove_file` は `false` を返します。

### バイト

バイナリファイルには、0 から 255 の数値のリストを返す `read_file_bytes`（エラー時は空のリスト）と、
それを書き戻して成功時に `true` を返す `write_file_bytes` を使います。`file_write_bytes(path, bytes, true)` は上書きではなく追記します。

```ling
束縛 start = 実行 {
  印刷(write_file_bytes("hi.bin", [72, 105]))
  印刷(read_file_bytes("hi.bin"))
  file_write_bytes("hi.bin", [33], 真)
  印刷(read_file("hi.bin"))
  印刷(file_magic("hi.bin"), " ", file_magic("nothing.bin"))
}
```
```output
true
[72, 105]
Hi!
other missing
```

### ディレクトリ

`make_dir` はディレクトリを（足りない親ディレクトリも含めて）作り、成功すると `true` を返します。`list_dir` は各項目を、
渡したディレクトリで始まるパスとして返します。`copy_file` と `rename_file` は成功すると `true` を返します。

```ling
束縛 start = 実行 {
  make_dir("out")
  write_file("out/a.txt", "あ")
  copy_file("out/a.txt", "out/b.txt")
  ファイル改名("out/b.txt", "out/c.txt")
  印刷(list_dir("out"))
  印刷(is_dir("out"), " ", is_file("out"))
  印刷(remove_dir_all("out"), " ", file_exists("out"))
}
```
```output
[out/a.txt, out/c.txt]
true false
true false
```

`remove_dir_all` はディレクトリを中身ごと削除し、元に戻せません。存在しないパスも成功として扱います。

### パス

`path_name` と `path_ext` はパスの最後の部分と拡張子を返します（なければ空のテキスト）。`absolute_path` は相対パスの前に
現在のディレクトリを付けます。パスが存在するかどうかは確かめません。

```ling
束縛 start = 実行 {
  印刷(path_name("photos/cat.png"), " ", path_ext("photos/cat.png"))
  印刷(path_ext("README"), "|")
}
```
```output
cat.png png
|
```

## 時刻

### `time_now()` と `epoch_now()`

`time_now` はプログラム開始からの秒数（小数付き）を数えるので、時間計測やアニメーションに向いています。`epoch_now` は Unix 秒で表した実際の時刻です。
日付を整形する組み込み関数はありません。

```ling
束縛 start = 実行 {
  束縛 開始 = 経過時間()
  眠る(20)
  印刷(経過時間() - 開始 >= 0.02)
  印刷(epoch_now() > 1700000000)
}
```
```output
true
true
```

### `random_hex(bytes)`

暗号論的に安全な乱数バイトを `bytes` 個、小文字の 16 進数で返します（既定は 16 バイト、つまり 32 文字）。
トークンや ID に使い、ゲームには `rand()` を使ってください。

```ling
束縛 start = 実行 {
  印刷(長さ(random_hex(16)))
  印刷(random_hex(4) == random_hex(4))
}
```
```output
32
false
```

## コマンドラインと環境

### `cli_arg(name, default)` と `get_args()`

`cli_arg("port", "8080")` はコマンドラインの `--port` の後ろの値を返し、なければ既定値を返します。理解できるのは 2 語の形 `--port 9000` だけです。
`get_args()` はコマンドライン全体を返します。`ling run app.ling a b` なら、`ling` の実行ファイル、`run`、スクリプト、そしてあなたの引数の順です。

```ling
束縛 start = 実行 {
  印刷(cli_arg("port", "8080"))
  印刷(get_args()[1])
}
```
```output
8080
run
```

### `env_get(name, default)`

環境変数を読みます。設定されていなければ既定値（省略時は空のテキスト）を返します。

```ling
束縛 start = 実行 {
  印刷(env_get("MY_APP_MODE", "開発"))
}
```
```output
開発
```

## プロセス

`run_process(cmd, args, timeout, log)` はプログラムを直接実行し（シェルを通さないので、クォートの問題もインジェクションもありません）、
出力を `log` ファイルに追記して終了を待ちます。戻り値は終了コードで、起動できなければ `-1`、`timeout` 秒を過ぎて強制終了されたら `-2` です。

`proc_start(cmd, args, log)` はプログラムを起動して待たずに id を返します（失敗時は `-1`）。`proc_poll(id)` は実行中は `-2`、
終了後は終了コードを返し、`proc_kill(id)` はプログラムを止めます。

```ling-gui
束縛 start = 実行 {
  印刷(run_process("git", ["--version"], 10, "git.log"))
  印刷(trim(read_file("git.log")))
  束縛 番号 = proc_start("git", ["status"], "status.log")
  間 proc_poll(番号) == -2 { 眠る(100) }
  印刷("完了")
}
```

プロセス系の組み込み関数は `web` [プロファイル](index.md)に含まれます。これを含まないビルドでは、再インストールの方法を示すエラーになります。

## スレッドとチャネル

Ling のスレッドはアイソレートです。それぞれが自分のインタプリタを動かし、チャネルと一度だけ書き込める保管場所以外は何も共有しないので、
競合が起きません。スレッド間で送る値はコピーされ、関数は送れません。

| Builtin | Purpose |
|---|---|
| `thread_spawn(file, fn_name, args, caps)` | 別の `.ling` ファイルの関数を新しいスレッドで実行し、`ok(handle)` を返します |
| `thread_join(handle)` | スレッドの終了を待ち、`ok(value)` か `bad(error)` を返します |
| `thread_cpus()` | ハードウェアスレッドの数 |
| `chan_new(capacity)` | 新しいチャネル。`[send_handle, receive_handle]` の形です |
| `chan_send(send, value)` | 値を送ります（満杯の間は待ちます） |
| `chan_recv(receive)` | 次の値を `ok(value)` で受け取ります。閉じていれば `bad("closed")` |
| `chan_close(handle)` | チャネルの片側を閉じます |
| `chan_len(handle)` | 待っているメッセージの数 |
| `shared_put(key, value)` | 値を一度だけ保存します。キーが使用済みなら `bad("exists")` |
| `shared_get(key, default)` | 保存した値を読みます |

チャネルは 1 つのプログラムの中でも使えます。

```ling
束縛 start = 実行 {
  束縛 道 = chan_new(4)
  chan_send(道[0], "こんにちは")
  chan_send(道[0], 42)
  印刷(chan_len(道[1]))
  印刷(chan_recv(道[1]), " ", chan_recv(道[1]))
  chan_close(道[0])
  印刷(chan_recv(道[1]))
  印刷(shared_put("設定", [1, 2]), " ", shared_put("設定", 0))
  印刷(shared_get("設定", []))
}
```
```output
2
Ok(こんにちは) Ok(42)
Err(closed)
Ok(()) Err(exists)
[1, 2]
```

`thread_spawn` は関数を別のファイルから読み込みます。パスは呼び出し元のファイルからの相対です。`caps` にはそのスレッドに許すことを
`fs db net p2p proc keys clipboard` から選んで並べます。親が持たない権限を子スレッドが得ることはありません。join の前に `match` でハンドルを取り出してください。

```ling-module
// main.ling。隣の worker.ling では次を定義しています：関数 二乗(n) { n * n }
関数 スレッドで二乗(n) {
  束縛 ハンドル = 一致 thread_spawn("worker.ling", "二乗", [n], []) { 良し(v) => v, 悪い(e) => e }
  thread_join(ハンドル)
}
```
