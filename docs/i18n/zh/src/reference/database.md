# 数据库（SQLite）

Ling 内置了 SQLite。三个内置函数就能覆盖全部用法：`db_open` 打开数据库文件，
`db_exec` 修改数据或表结构，`db_query` 把查询结果读回成记录。SQLite 已经打包进
`ling`，所以不需要安装任何东西，也不需要运行数据库服务器。数据库就是放在程序旁边的
一个普通文件。

这些内置函数属于 `web` 安装配置（以及默认的 `full`），因为它们通常和
[Web 服务器](../reference/web.md)搭配使用。在命令行工具里它们同样好用。

## 一览

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `db_open(path)` | — | — | — | `ฐานข้อมูลเปิด` | 打开或创建数据库，并设为当前连接。 |
| `db_exec(sql, params...)` | — | — | — | `ฐานข้อมูลรัน` | 执行一条修改数据或表结构的语句，返回受影响的行数。 |
| `db_query(sql, params...)` | — | — | — | `ฐานข้อมูลถาม` | 执行查询，返回 `Row` 记录列表。 |

这三个函数没有中文别名，中文程序里也直接写英文名。

## 第一个数据库

```ling
令 始 = 执 {
  db_open(":memory:")
  db_exec("CREATE TABLE 笔记(编号 INTEGER PRIMARY KEY AUTOINCREMENT, 标题 TEXT NOT NULL, 星级 REAL, 备注 TEXT)")
  db_exec("INSERT INTO 笔记(标题, 星级) VALUES (?1, ?2)", "第一条", 4.5)
  db_exec("INSERT INTO 笔记(标题, 星级) VALUES (?1, ?2)", "很安全", 3)
  令 各行 = db_query("SELECT 编号, 标题, 星级, 备注 FROM 笔记 ORDER BY 编号")
  打印(长度(各行))
  打印(各行[0])
  历 行 于 各行 {
    打印(str(行.编号) + ": " + 行.标题)
  }
  打印("[" + 各行[1].备注 + "]")
}
```

```output
2
Row { 编号: 1, 标题: 第一条, 星级: 4.5, 备注:  }
1: 第一条
2: 很安全
[]
```

SQLite 允许用中文作表名和列名，所以记录的字段名也可以是中文。`":memory:"` 创建的
数据库只在程序运行期间存在，适合测试和示例。想保存数据，就用 `"app.db"` 这样的文件名。

## `db_open(path)`

打开位于 `path`（默认 `"app.db"`）的 SQLite 数据库，文件不存在时会自动创建，
并把它设为解释器的当前连接。没有返回值。

- 打开时，Ling 会启用外键（`PRAGMA foreign_keys = ON`）、预写日志
  （`journal_mode = WAL`）以及 5 秒的忙等待超时。这样一个写操作遇到另一个写操作时，
  会排队等待，而不是报错 "database is locked"。
- 每个解释器只持有**一个**连接。再次调用 `db_open` 会替换它。
- 使用 WAL 时，数据库打开期间 SQLite 会在旁边保留两个辅助文件（`app.db-wal` 和
  `app.db-shm`）。备份时请把三个文件一起备份，或者在没有程序打开数据库时再复制。
- 如果文件无法打开，`db_open` 会报错。

## `db_exec(sql, params...)`

执行**一条**不返回行的 SQL 语句：`CREATE`、`INSERT`、`UPDATE`、`DELETE`、
`BEGIN`、`COMMIT` 等。返回被修改的行数（表结构语句返回 `0`）。

- 每次调用只放一条语句。用 `;` 分隔的两条语句会报错。
- 失败时会报错，错误信息里包含 SQL 文本。在 `db_open` 之前调用也会报错。

## `db_query(sql, params...)`

执行查询，并把**所有**行作为 `Row` 记录列表返回。用点号读取列：`行.标题`。
字段名就是 `SELECT` 中的列名，所以计算出来的列要用 `AS` 起个名字：

```ling
令 始 = 执 {
  db_open(":memory:")
  db_exec("CREATE TABLE 日志(数 INTEGER)")
  db_exec("BEGIN")
  令 i = 0
  循 i < 1000 {
    db_exec("INSERT INTO 日志(数) VALUES (?1)", i)
    令 i = i + 1
  }
  db_exec("COMMIT")
  令 结果 = db_query("SELECT count(*) AS 条数, sum(数) AS 总和, avg(数) AS 均值 FROM 日志")[0]
  打印(结果.条数)
  打印(结果.总和)
  打印(结果.均值)
}
```

```output
1000
499500
499.5
```

把大量插入包在 `BEGIN` 和 `COMMIT` 之间，就成了一个事务，比每行单独提交快得多。

### 值如何返回

| SQLite 值 | Ling 值 |
|---|---|
| `INTEGER` | 数字 |
| `REAL` | 数字 |
| `TEXT` | 文本 |
| `NULL` | `""`（空文本） |
| `BLOB` | base64 文本 |

没有匹配结果的查询返回空列表，所以读取 `各行[0]` 之前先检查 `长度(各行)`。

## 参数

参数按**位置**绑定到 `?1`、`?2`、`?3` 等，顺序就是你在 SQL 后面传入的顺序。
用户输入一定要作为参数传入，绝不要拼接进 SQL 文本。参数永远只是一个值，
所以它无法改变语句本身：

```ling
令 始 = 执 {
  db_open(":memory:")
  db_exec("CREATE TABLE 用户(编号 INTEGER PRIMARY KEY, 名字 TEXT UNIQUE, 管理员 INTEGER)")
  令 名字 = "Robert'); DROP TABLE 用户; --"
  db_exec("INSERT INTO 用户(名字, 管理员) VALUES (?1, ?2)", 名字, 假)
  db_exec("INSERT INTO 用户(名字, 管理员) VALUES (?1, ?2)", "阿达", 真)
  打印(db_query("SELECT count(*) AS 人数 FROM 用户")[0].人数)
  打印(db_query("SELECT 名字 FROM 用户 WHERE 管理员 = ?1", 真)[0].名字)
  打印(db_query("SELECT last_insert_rowid() AS 编号")[0].编号)
  打印(db_exec("UPDATE 用户 SET 管理员 = 0 WHERE 名字 = ?1", "阿达"))
  打印(db_exec("DELETE FROM 用户 WHERE 编号 > ?1", 100))
}
```

```output
2
阿达
2
1
0
```

Ling 值的绑定方式：

| Ling 值 | 绑定为 |
|---|---|
| 整数（`3`） | `INTEGER` |
| 小数（`4.5`） | `REAL` |
| `真` / `假` | `1` / `0` |
| 其他 | `TEXT`（值的文本形式） |

想取得刚插入的那一行的 id，就像上面那样在同一个连接上查询
`SELECT last_insert_rowid() AS 编号`。

## 错误

语句失败（语法错误、表不存在、违反 `UNIQUE` 约束）时会引发运行时错误，
错误信息以 `db_exec:` 或 `db_query:` 开头，并显示 SQL。

在解释器上（`ling run --interp`，以及所有调用 `http_serve` 的程序），错误会终止程序；
在 `尝试` 里则会以 `坏` 的形式返回。使用 `ling run` 默认的 JIT 后端时，失败的语句
目前会被悄悄跳过，没有任何提示，所以开发数据库脚本时请加上 `--interp` 运行。

```ling-module
函 添加用户(名字) {
  归 db_exec("INSERT INTO 用户(名字) VALUES (?1)", 名字)
}

函 尝试添加(名字) {
  配 尝试(| | 添加用户(名字)) {
    好(n) => 归 "已添加"
    坏(e) => 归 "无法添加 " + 名字 + "：" + e
  }
}
```

## 与 Web 服务器配合

常见的结构是：在 `始` 里打开数据库，必要时建表，注册路由，然后开始服务。
处理函数直接调用 `db_query` 和 `db_exec`。

```ling-gui
形 响应 { status, content_type, body, set_cookie, location }

函 笔记列表(请求) {
  令 各行 = db_query("SELECT 编号, 标题 FROM 笔记 ORDER BY 编号 DESC LIMIT 50")
  令 输出 = "<ul>"
  历 行 于 各行 {
    令 输出 = 输出 + "<li>" + html_escape(行.标题) + "</li>"
  }
  归 输出 + "</ul>"
}

函 添加笔记(请求) {
  令 标题 = query_param(请求.body, "title", "")
  若 标题 == "" {
    归 响应(400, "text/plain; charset=utf-8", "标题不能为空", "", "")
  }
  db_exec("INSERT INTO 笔记(标题) VALUES (?1)", 标题)
  归 响应(303, "text/plain", "", "", "/")
}

令 始 = 执 {
  db_open("notes.db")
  db_exec("CREATE TABLE IF NOT EXISTS 笔记(编号 INTEGER PRIMARY KEY AUTOINCREMENT, 标题 TEXT NOT NULL)")
  http_route("GET", "/", 笔记列表)
  http_route("POST", "/add", 添加笔记)
  http_serve("127.0.0.1", 8080, 4)
}
```

响应记录的字段名（`status`、`content_type`、`body`、`set_cookie`、`location`）以及
请求的字段名（`body`、`query` 等）必须保持英文，服务器靠这些名字识别它们。
记录本身的名字（这里是 `响应`）可以随意取。

### 工作线程与连接

使用 `http_serve(host, port, 1)`（默认值）时，一个解释器通过它唯一的连接，
逐个回答所有请求。

当 `workers` 大于 1 时，每个额外的工作线程都是独立的解释器，拥有**自己的连接**，
连到你传给 `db_open` 的路径。读操作可以并行；写操作仍然一次一个：WAL 加上忙等待超时
让它们排队而不是失败。由此有两点要注意：

- 在 `始` 里、`http_serve` **之前**调用 `db_open`。工作线程打开的是服务器启动时的
  当前路径。
- 使用工作线程时不要用 `":memory:"`。每个工作线程都会得到一个各自独立的空内存数据库。

处理函数里的事务要尽量短。同一个处理函数里只有 `BEGIN` 而没有 `COMMIT`，
会一直占着写锁，让其他所有写操作等待。

## 另请参阅

- [Web 服务器与 HTTP](../reference/web.md)：路由、请求、响应和工作线程。
- [一个小型 Web 服务](../tutorials/web-service.md)：一步步搭建完整的服务。
- [ling-http 与 ling-web](../crates/web.md)：在 Rust 中使用同一套 SQLite 层。
