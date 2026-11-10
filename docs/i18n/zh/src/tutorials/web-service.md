# 一个小型 Web 服务

Ling 内置了 Web 服务器。在这一页里，你会提供一个网页，读取浏览器发来的内容，用 JSON 回应，
把笔记存进 SQLite 数据库，最后把它们组合成一个小小的笔记 API。不需要任何框架，也不需要额外安装。

启动服务器的代码无法在终端里检查，所以这些代码块下面没有输出。辅助函数和处理函数都是普通函数，
不用服务器也能测试，下面就会看到。

Web 服务器和数据库的内置函数（`http_*`、`db_*`、`query_param` 等）没有中文别名，所以在中文代码里
也保留英文名称。请求和响应的字段名（`query`、`body`、`status` 等）也必须用英文，因为服务器按名字读取它们。

## 你好，Web

```ling-gui
函 你好(请求) {
    归 "<h1>来自 Ling 的问候！</h1>"
}

令 始 = 执 {
    http_route("GET", "/", 你好)
    http_serve("127.0.0.1", 8080)
}
```

把它保存为 `hello.ling`，运行后在浏览器中打开 <http://127.0.0.1:8080/>：

```sh
ling run hello.ling
```

- `http_route(method, path, handler)` 为一个方法和一个精确路径注册**处理函数**。请在启动服务器之前
  注册所有路由。
- `http_serve(host, port)` 启动服务器并持续响应请求，直到你按下 Ctrl+C。`"127.0.0.1"` 只允许本机访问，
  `"0.0.0.0"` 接受来自网络的连接。
- 处理函数接收请求并返回一个字符串。字符串会以 `200` 和 HTML 页面的形式返回。
- 没有注册的路径返回 `404 not found`。

要提供 CSS、图片或整个文件夹这样的文件，请加上 `http_static(prefix, dir)`：
`http_static("/css", "css")` 把 `/css/site.css` 映射到文件 `css/site.css`。静态文件按原样逐字节发送，
不经过你的处理函数。

## 浏览器发来了什么

处理函数的参数是一个 `Request`，它有这些文本字段：

| 字段 | 内容 |
|---|---|
| `method` | 大写的 `"GET"`、`"POST"` 等 |
| `path` | 不含查询字符串的路径 |
| `query` | `?` 之后的原始查询字符串，例如 `name=Ada&page=2` |
| `body` | 文本形式的请求体 |
| `cookie` | 原始的 `Cookie` 头 |
| `authorization` | 原始的 `Authorization` 头 |
| `client_ip` | 从代理头得到的客户端地址，或 `""` |

路由精确匹配路径，所以没有 `/note/2` 这样的路径参数。请改用查询字符串传值（`/note?id=2`），再用
`query_param(text, name, default)` 读取。它会对值做百分号解码，也适用于表单请求体，因为表单使用同样的
`a=1&b=2` 格式。

```ling
令 始 = 执 {
    令 查询 = "name=%E5%BC%A0%E4%B8%89&lang=zh"
    打印(query_param(查询, "name", "无名氏"))
    打印(query_param(查询, "page", "1"))
}
```

```output
张三
1
```

永远不要把用户输入的文本直接放进 HTML。`html_escape` 会把 `& < > " '` 转换成实体：

```ling
令 始 = 执 {
    打印(html_escape("<b>猫 & 老鼠</b>"))
}
```

```output
&lt;b&gt;猫 &amp; 老鼠&lt;/b&gt;
```

所以一个问候页面是这样的：

```ling-module
函 问候(请求) {
    令 名字 = html_escape(query_param(请求.query, "name", "世界"))
    归 "<h1>你好，" + 名字 + "！</h1>"
}
```

## 用 JSON 回应

要设置状态码或内容类型，就返回一个记录而不是字符串。声明一个 `form`（中文写作 `形`），字段名取自
`status`、`content_type`、`body`、`set_cookie` 和 `location`。记录按位置构造，服务器按名字读取字段：

```ling-module
形 Response { status, content_type, body }

函 回应JSON(状态, 内容) {
    归 Response(状态, "application/json", 内容)
}
```

Ling 通过拼接字符串来构造 JSON。每段文本都要经过 `json_escape`，它会转义引号、反斜杠和控制字符：

```ling
令 始 = 执 {
    令 标题 = "说 \"你好\""
    打印("{\"title\":\"" + json_escape(标题) + "\"}")
}
```

```output
{"title":"说 \"你好\""}
```

要重定向，就返回状态 `302` 和 `location`，例如在声明了全部五个字段的记录里写
`Response(302, "text/plain", "", "", "/new")`。

## 基于 SQLite 的笔记 API

现在把笔记存进数据库。三个内置函数就能处理 SQLite：

| 内置函数 | 用途 |
|---|---|
| `db_open(path)` | 打开（或创建）一个数据库文件，或者用 `":memory:"` |
| `db_exec(sql, params...)` | 执行修改数据的语句；返回被修改的行数 |
| `db_query(sql, params...)` | 执行查询；返回行的列表，用 `row.column` 读取 |

参数按位置绑定为 `?1`、`?2` 等。用户输入一定要作为参数传入，绝不要拼接到 SQL 文本里。

处理函数只是普通函数，所以你可以自己构造一个请求，不用服务器就能试用它们。这里的 `Request` 是一个
小小的 `形`，只包含处理函数用到的字段：

```ling
形 Response { status, content_type, body }
形 Request { method, path, query, body }

函 回应JSON(状态, 内容) {
    归 Response(状态, "application/json", 内容)
}

函 笔记JSON(行) {
    归 "{\"id\":" + 行.id + ",\"text\":\"" + json_escape(行.text) + "\"}"
}

函 列出笔记(请求) {
    令 行们 = db_query("SELECT id, text FROM notes ORDER BY id")
    令 结果 = "["
    令 i = 0
    循 i < 长度(行们) {
        若 i > 0 {
            令 结果 = 结果 + ","
        }
        令 结果 = 结果 + 笔记JSON(行们[i])
        令 i = i + 1
    }
    归 回应JSON(200, 结果 + "]")
}

函 添加笔记(请求) {
    令 文本 = query_param(请求.body, "text", "")
    若 文本 == "" {
        归 回应JSON(400, "{\"error\":\"缺少 text\"}")
    }
    db_exec("INSERT INTO notes(text) VALUES (?1)", 文本)
    令 行们 = db_query("SELECT id, text FROM notes ORDER BY id DESC LIMIT 1")
    归 回应JSON(201, 笔记JSON(行们[0]))
}

函 准备() {
    db_open(":memory:")
    db_exec("CREATE TABLE notes(id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL)")
}

令 始 = 执 {
    准备()
    令 r1 = 添加笔记(Request("POST", "/notes", "", "text=买牛奶"))
    打印(r1.status, " ", r1.body)
    令 r2 = 添加笔记(Request("POST", "/notes", "", "text=说+%22你好%22"))
    打印(r2.status, " ", r2.body)
    令 r3 = 添加笔记(Request("POST", "/notes", "", ""))
    打印(r3.status, " ", r3.body)
    令 r4 = 列出笔记(Request("GET", "/notes", "", ""))
    打印(r4.status, " ", r4.body)
}
```

```output
201 {"id":1,"text":"买牛奶"}
201 {"id":2,"text":"说 \"你好\""}
400 {"error":"缺少 text"}
200 [{"id":1,"text":"买牛奶"},{"id":2,"text":"说 \"你好\""}]
```

列表是用 `循` 循环和下标构造的，因为 `历` 循环体不能更新循环外的绑定。

### 提供服务

处理函数和 `准备` 与上面完全相同，去掉测试用的 `Request` 记录，再把 `始` 块换成路由：

```ling-gui
形 Response { status, content_type, body }

函 回应JSON(状态, 内容) {
    归 Response(状态, "application/json", 内容)
}

函 笔记JSON(行) {
    归 "{\"id\":" + 行.id + ",\"text\":\"" + json_escape(行.text) + "\"}"
}

函 列出笔记(请求) {
    令 行们 = db_query("SELECT id, text FROM notes ORDER BY id")
    令 结果 = "["
    令 i = 0
    循 i < 长度(行们) {
        若 i > 0 {
            令 结果 = 结果 + ","
        }
        令 结果 = 结果 + 笔记JSON(行们[i])
        令 i = i + 1
    }
    归 回应JSON(200, 结果 + "]")
}

函 添加笔记(请求) {
    令 文本 = query_param(请求.body, "text", "")
    若 文本 == "" {
        归 回应JSON(400, "{\"error\":\"缺少 text\"}")
    }
    db_exec("INSERT INTO notes(text) VALUES (?1)", 文本)
    令 行们 = db_query("SELECT id, text FROM notes ORDER BY id DESC LIMIT 1")
    归 回应JSON(201, 笔记JSON(行们[0]))
}

函 准备() {
    db_open("notes.db")
    db_exec("CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL)")
}

函 问候(请求) {
    令 名字 = html_escape(query_param(请求.query, "name", "世界"))
    归 "<h1>你好，" + 名字 + "！</h1>"
}

令 始 = 执 {
    准备()
    http_route("GET", "/", 问候)
    http_route("GET", "/notes", 列出笔记)
    http_route("POST", "/notes", 添加笔记)
    http_serve("127.0.0.1", to_number(cli_arg("port", "8080")))
}
```

这个版本打开程序旁边的文件 `notes.db`，所以重启后笔记依然存在。`cli_arg("port", "8080")` 以文本形式
读取命令行里的 `--port`，`to_number` 把它转换成数字。

```sh
ling run notes.ling --port 8080
```

在另一个终端里试试：

```text
$ curl -X POST -d "text=Buy milk" http://127.0.0.1:8080/notes
{"id":1,"text":"Buy milk"}
$ curl http://127.0.0.1:8080/notes
[{"id":1,"text":"Buy milk"}]
$ curl "http://127.0.0.1:8080/?name=%3Cb%3EAda"
<h1>你好，&lt;b&gt;Ada！</h1>
```

## 同时服务更多访客：workers

默认情况下，一个解释器依次处理请求，所以一个慢的处理函数会让其他请求等待。`http_static` 提供的
静态文件不受影响：它们单独处理，可以并行传输。要让处理函数并行运行，请把 worker 数作为第三个参数
传给 `http_serve`。这里从命令行读取：

```ling-gui
函 你好(请求) {
    归 "<h1>来自 Ling 的问候！</h1>"
}

函 工作数() {
    归 to_number(cli_arg("workers", "1"))
}

令 始 = 执 {
    http_route("GET", "/", 你好)
    http_serve("127.0.0.1", 8080, 工作数())
}
```

```sh
ling run hello.ling --workers 4
```

当 worker 多于一个时，每个额外的解释器都会重新加载你的程序，而 `始` 块只在主解释器上运行一次。因此：

- 处理函数必须是**有名字的顶层函数**，就像本页所有的处理函数一样。
- 把设置放在函数里（比如上面的 `工作数()`），不要放在顶层的 `令` 里。
- 每个 worker 都会重新打开你用 `db_open` 打开的数据库。请使用文件，而不是 `":memory:"`，否则每个
  worker 都会得到自己的空数据库。两个 worker 同时写入时，SQLite 会让写入排队等待。

## ling-lang.org 就运行在这上面

ling-lang.org 网站由一个 Ling 程序 `site.ling` 提供，用的正是这些内置函数。它用 `http_static` 注册
CSS、图片、视频、Ling 之书 PDF 和这份文档的静态文件夹，用 `http_route` 响应首页和几个 JSON 接口，
并以多个 worker 启动：

```sh
ling run site.ling --host 0.0.0.0 --port 80 --workers 4
```

它还用到了 `http_vhost(host, dir)`，这个函数在另一个主机名的根路径上提供一个文件夹，按请求的
`Host` 头匹配。如果 `dir` 不是文件夹，而是 `http://` 或 `https://` 地址，该主机就会重定向到那里。
旧的 `docs.ling-lang.org` 就是这样把访客送到 `ling-lang.org/docs` 的：

```ling-module
函 注册文档主机() {
    http_vhost("docs.ling-lang.org", "https://ling-lang.org/docs")
    归 真
}
```

## 动手试试

1. 添加 `GET /note?id=2`，返回一条笔记；当 `db_query` 返回空列表时，返回状态 `404` 和
   `{"error":"not found"}`。
2. 添加 `POST /delete?id=2`，使用 `db_exec("DELETE FROM notes WHERE id = ?1", id)`。用 `db_exec` 返回的
   数字判断，什么都没删除时返回 `404`。
3. 用 `http_static` 在 `/app` 下提供一个包含 `index.html` 的 `public/` 文件夹。
4. 让问候页面在 `visits` 表里记录访问次数，并显示总数。

所有服务器相关的内置函数见 [Web 服务器与 HTTP](../reference/web.md)和[数据库（SQLite）](../reference/database.md)。
