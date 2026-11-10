# Web 服务器与 HTTP

Ling 自带一个小巧、快速的 Web 服务器。你先注册路由、静态目录和虚拟主机，然后调用
`http_serve`，它会阻塞并开始回答请求。`http_*` 内置函数在 Ling 代码中驱动
[`ling-http`](https://crates.io/crates/ling-http)（axum 和 tokio）。同一系列里还有
HTTP 客户端（`http_get` 等），以及处理查询字符串、Cookie 和转义的辅助函数。

**ling-lang.org 就运行在它上面。** 它的服务器是一个 Ling 文件 `site.ling`，负责提供
首页、`/docs/` 下的这些文档、Ling 之书的 PDF 和示例 SVG，并用 `http_vhost` 把已经
停用的 `docs.ling-lang.org` 子域名转到 `/docs/`。

> **安装配置。** 服务器和客户端内置函数属于 `web` 安装配置（以及默认的 `full`）。
> 用 `--no-default-features --features core` 构建的 `ling` 会报错
> `unknown function 'http_static'`。请用 `cargo install ling-lang` 重新安装，
> 或者加上 `--features web`。

## 请求如何被回答

1. **虚拟主机**（`http_vhost`）最先检查。如果 `Host` 头指向一个已注册的主机，
   请求就从该主机的目录回答；如果目标是 URL，则重定向。
2. 接着是**静态前缀**（`http_static`）。在 `/css` 前缀下，`/css/style.css` 会按原样
   逐字节从磁盘读取。
3. 最后是**路由**（`http_route`）：方法加上**完全一致**的路径。处理函数是一个 Ling 函数，
   它接收一个 `Request`，返回文本或响应记录。
4. 其他情况返回 `404`，正文为 `not found`。

第 1 步和第 2 步在异步运行时上执行，从不经过解释器，所以大文件（视频、PDF、zip）
可以并发传输，并支持 HTTP 范围请求。除非给 `http_serve` 传入 `workers`，否则 Ling
处理函数在解释器上逐个运行。

调用 `http_serve` 的程序总是在解释器上运行，即使用的是普通的 `ling run`，
因为处理函数是作为参数传入的函数。

## 一览

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `http_route(method, path, handler)` | — | — | — | `เว็บเส้นทาง` | 处理一个方法和一个完全一致的路径。 |
| `http_static(prefix, dir)` | — | — | — | `เว็บสแตติก` | 在 URL 前缀下按原样提供一个目录。 |
| `http_vhost(host, target)` | — | — | — | — | 在某个主机名的根路径提供一个目录，或重定向该主机名。 |
| `http_serve(host, port, workers)` | — | — | — | `เว็บเสิร์ฟ` | 启动 HTTP 并运行请求循环。会阻塞。 |
| `http_serve_tls(host, port, cert, key, workers)` | — | — | — | — | 使用 PEM 证书和私钥文件提供 HTTPS。 |
| `http_serve_dev_tls(host, port, workers)` | — | — | — | — | 使用临时自签名证书提供 HTTPS。 |
| `http_get(url)` | — | — | — | `เว็บรับ` | 获取 URL 并等待正文。 |
| `http_get_status()` | — | — | — | — | 上一次 `http_get` 的状态码。 |
| `http_get_all(urls)` | — | — | — | — | 同时获取多个 URL。 |
| `http_get_async(url)` | — | — | — | — | 在后台发起 GET，返回任务 id。 |
| `http_post_async(url, body, type)` | — | — | — | `เว็บโพสต์ไม่บล็อก` | 在后台发起 POST，返回任务 id。 |
| `http_job_poll(id)` | — | — | — | `เว็บงานสำรวจ` | 任务运行时返回 `""`，之后返回结果。 |
| `query_param(qs, name, default)` | — | — | — | `พารามิเตอร์` | 从 `a=1&b=2` 中取出一个值并解码。 |
| `cookie_get(header, name, default)` | — | — | — | `รับคุกกี้` | 从 `Cookie` 头中取出一个 Cookie。 |
| `html_escape(text)` | — | — | — | `กันเอชทีเอ็มแอล` | 为 HTML 转义 `& < > " '`。 |
| `json_escape(text)` | — | — | — | `หนีเจสัน` | 为 JSON 字符串转义文本。 |
| `url_decode(text)` | `网址解码` | — | — | — | 解码 `%XX` 和 `+`。 |
| `cli_arg(name, default)` | — | — | — | `อาร์กิวเมนต์` | 命令行中的 `--name value`，以文本形式返回。 |
| `env_get(name, default)` | — | — | — | `รับตัวแปรแวดล้อม` | 一个环境变量。 |

## 服务器内置函数

### `http_route(method, path, handler)`

为 `method`（不区分大小写，默认 `"GET"`）和 `path`（默认 `"/"`）注册处理函数
`handler`。路径**完全一致**才匹配，并且不含查询字符串，所以没有路径参数：
把 id 放进查询字符串（`/note?id=2`），再用 `query_param(req.query, "id", "")` 读取。
所有路由都要在调用 `http_serve` 之前注册。

处理函数出错时会返回 `500`，正文是 `handler error: ...`，这可能泄露内部信息。
请校验输入，别让错误漏出去。

### `http_static(prefix, dir)`

在 `prefix`（默认 `"/static"`）下提供目录 `dir`（默认 `"static"`）：
`http_static("/css", "css")` 会把 `/css/a.css` 映射到 `css/a.css`。请求一个目录时
返回其中的 `index.html`。所有二进制内容都用它提供，因为路由处理函数只能返回文本。

### `http_vhost(host, target)`

为整个主机名指定单独的行为。`target` 可以是一个目录，也可以是一个 URL。

- **目录**：该主机名从这个目录的根路径提供，和 `http_static` 一样按原样逐字节返回。
  找不到文件时返回 `404`，如果目录里有 `404.html` 就用它作为页面（mdBook 和大多数
  静态站点生成器都会生成这个文件）。用它把一个静态站点放到独立的子域名上。
- **`http://` 或 `https://` URL**：对该主机名的每个请求都返回
  `301 Moved Permanently`，跳转到目标地址加上请求的路径和查询字符串。
  用它停用一个子域名，同时不让旧链接失效。

细节：

- `host` 与 `Host` 头比较时不区分大小写，并忽略端口。
- 虚拟主机在所有 `http_static` 前缀和路由之前检查，所以虚拟主机的请求永远不会
  到达你的处理函数。
- 可以注册多个虚拟主机；每个 `Host` 最多匹配其中一个。

```ling-gui
令 始 = 执 {
  http_vhost("docs.example.org", "../docs/book")              // 子域名根路径上的静态站点
  http_vhost("old.example.org", "https://example.org/docs")   // old.example.org/a.html -> https://example.org/docs/a.html
  http_static("/docs", "../docs/book")                        // 主域名 /docs/ 下的同一个站点
  http_serve("0.0.0.0", 8080)
}
```

ling-lang.org 在 `site.ling` 中就是这样停用旧的文档子域名的：

```ling-module
函 注册文档主机(端口) {
  http_vhost("docs.ling-lang.org", "https://ling-lang.org/docs")
  http_vhost("docs.localhost", "http://localhost:" + str(端口) + "/docs")
  归 真
}
```

想在本地不配置 DNS 就试用虚拟主机，可以自己发送这个请求头：

```sh
curl -i -H "Host: docs.localhost" http://127.0.0.1:8080/
```

大多数浏览器也会把 `*.localhost` 解析到本机，所以 `http://docs.localhost:8080/`
同样可用。

### `http_serve(host, port, workers)`

启动服务器并一直回答请求，直到程序被停止。它永远不会返回。

- `host`：`"127.0.0.1"`（默认）只允许本机访问；`"0.0.0.0"` 接受来自网络的连接。
- `port`：默认 `8080`。
- `workers`：默认 `1`。见下面的"工作线程"一节。

套接字绑定成功后会打印一条横幅。如果端口被占用，错误会写到标准错误输出，
程序则继续等待；所以没有响应时请查看日志。

### `http_serve_tls(host, port, cert, key, workers)`

直接提供 HTTPS，不需要反向代理。`cert` 和 `key` 是 PEM 文件的**路径**，
例如 Let's Encrypt 的 `fullchain.pem` 和 `privkey.pem`。私钥文件应只允许服务器用户读取。
其余用法和 `http_serve` 相同。

### `http_serve_dev_tls(host, port, workers)`

使用启动时生成的自签名证书提供 HTTPS。浏览器和 `curl` 会拒绝它（本地可用 `curl -k`）。
只用于本地开发，绝不要暴露到互联网上。

## 请求

处理函数接收一个参数：一个 `Request` 记录，包含这些文本字段：

| 字段 | 内容 |
|---|---|
| `method` | 大写的方法名，例如 `GET`。 |
| `path` | 不含查询字符串的路径。 |
| `query` | 原始查询字符串，不含 `?`。用 `query_param` 读取。 |
| `body` | UTF-8 正文。最多 32 MiB，多部分上传除外（见下文）。 |
| `cookie` | 原始 `Cookie` 头，或 `""`。用 `cookie_get` 读取。 |
| `authorization` | 原始 `Authorization` 头，或 `""`。 |
| `client_ip` | `X-Forwarded-For` 的第一跳，否则是 `X-Real-IP`，否则是 `""`。 |

`client_ip` 来自任何客户端都能设置的请求头。只有在你自己的反向代理后面才能信任它；
直接连接时它为空。

`multipart/form-data` 上传会把文件流式写入 `storage/tmp/` 下。处理函数随后得到一个
表单编码的正文，其中包含文本字段以及 `artifact_path` 和 `artifact_size`；
用 `query_param(req.body, ...)` 读取。

## 响应

返回**文本**会得到 `200`，类型为 `text/html; charset=utf-8`。其他不是记录的值会被转成
文本，以 `text/plain` 发送。

其余情况请返回一个记录，可以包含字段 `status`、`content_type`、`body`、`set_cookie`
和 `location` 中的任意几个。缺少的字段默认为状态 `200`、空正文和 `text/html`。
记录按位置构造，起作用的是字段名；记录本身叫什么无关紧要。字段名必须保持英文：

```ling-gui
形 响应 { status, content_type, body, set_cookie, location }

函 接口状态(请求) {
  归 响应(200, "application/json", "{\"ok\":true}", "", "")
}

函 旧页面(请求) {
  归 响应(301, "text/plain", "", "", "/new")
}

函 登录(请求) {
  令 饼干 = "sid=" + random_hex(16) + "; HttpOnly; Secure; SameSite=Lax; Path=/"
  归 响应(303, "text/plain", "", 饼干, "/me")
}

令 始 = 执 {
  http_route("GET", "/api/status", 接口状态)
  http_route("GET", "/old", 旧页面)
  http_route("POST", "/login", 登录)
  http_serve("127.0.0.1", 8080)
}
```

来自 Ling 路由的响应会带上 `X-Frame-Options: DENY` 和
`Content-Security-Policy: frame-ancestors 'none'`，`/embed` 下的路径除外。除此之外
不会添加任何东西：Cookie 的 `Secure`、`HttpOnly` 和 `SameSite` 需要你自己设置，
其他安全头（HSTS、完整的内容安全策略）请在反向代理上添加。

## 工作线程

默认情况下，一个解释器逐个回答所有请求。一个慢的处理函数会让其他所有访客等待。

使用 `http_serve(host, port, N)` 且 `N` 大于 1 时，`N` 个解释器从同一个队列中取请求。
ling-lang.org 从命令行读取这个数（`--workers 4`）。规则如下：

- 处理函数必须是**有名字的顶层函数**。内联闭包作为处理函数会在启动时报错。
- `始` 块只在主解释器上运行**一次**。工作线程只能看到程序的声明，所以配置要放在函数里，
  不要放在由 `始` 修改的顶层绑定里。
- 每个工作线程都会打开自己的数据库连接，连到 `db_open` 的数据库。
  见[数据库（SQLite）](../reference/database.md)。
- 异步任务是共享的：在一个工作线程上发起的任务，可以在另一个工作线程上轮询。

## 请求辅助函数

这些函数在任何地方都能用，不只限于处理函数，所以可以在无窗口模式下直接试：

```ling
令 始 = 执 {
  令 查询串 = "q=%E4%BD%A0%E5%A5%BD&page=2&tag=x+y"
  打印(query_param(查询串, "q", ""))
  打印(query_param(查询串, "page", "1"))
  打印(query_param(查询串, "tag", ""))
  打印(query_param(查询串, "sort", "最新"))
  打印(cookie_get("sid=abc123; theme=dark", "theme", "light"))
  打印(html_escape("<b>阿达 & '鲍勃'</b>"))
  打印("{\"名字\":\"" + json_escape("他说\"你好\"\n") + "\"}")
  打印(网址解码("a%20b+c%41"))
  打印(cli_arg("port", "8080"))
}
```

```output
你好
2
x y
最新
dark
&lt;b&gt;阿达 &amp; &#39;鲍勃&#39;&lt;/b&gt;
{"名字":"他说\"你好\"\n"}
a b cA
8080
```

- `query_param(qs, name, default)`：`name` 的第一个值，`%XX` 和 `+` 会被解码。
  可用于 `req.query`，也可用于表单正文（`req.body`）。
- `cookie_get(header, name, default)`：从 `req.cookie` 取出一个 Cookie，不做解码。
- `html_escape(text)`：在拼接 HTML 的地方转义每个不可信的值。它足以用于元素内容和
  带引号的属性值，但不适用于 `<script>` 内部或不带引号的属性。
- `json_escape(text)`：转义 `"`、`\` 和控制字符，但不添加两边的引号。Ling 通过拼接
  来构造 JSON，所以文本一定要经过它。
- `url_decode(text)`（`网址解码`）：解码 `%XX` 和 `+`。
- `cli_arg(name, default)`：命令行中的 `--name value`，返回的是**文本**。
  用 `to_number` 转换：`cli_arg("port", "8080") + 0` 得到的是 `"80800"`。
- `env_get(name, default)`：一个环境变量。机密信息应这样加载，而不是写在源码里。

## HTTP 客户端

### `http_get(url)` 和 `http_get_status()`

`http_get` 获取 `http://` 或 `https://` URL 并等待正文。即使状态码表示错误，
它也会返回正文。`http_get_status()` 返回上一次 `http_get` 的状态码；`0` 表示根本
没有响应（DNS、连接或 TLS 失败），此时正文为 `""`。信任正文之前先检查状态码。

`http_get` 会阻塞。在工具里或启动阶段这没问题，但在处理函数里它会卡住所有访客，
所以那里请用异步版本。

### `http_get_all(urls)`

同时获取一个 URL 列表，并等待全部完成。按相同顺序返回正文，失败或 HTTP 错误状态
对应 `""`。

### `http_get_async(url)`、`http_post_async(url, body, type)` 和 `http_job_poll(id)`

在后台发起请求，并立即返回任务 id。`type` 是 POST 正文的 `Content-Type`
（默认 `"application/json"`）。之后调用 `http_job_poll(id)`：任务运行时返回 `""`，
完成后返回响应正文。GET 失败时得到以 `ERROR:` 开头的文本；POST 失败时得到
`{"error":"..."}`。

```ling-gui
令 始 = 执 {
  令 任务 = http_get_async("https://ling-lang.org/api/books.json")
  令 正文 = ""
  令 次数 = 0
  循 正文 == "" && 次数 < 100 {
    令 正文 = http_job_poll(任务)
    睡眠(50)
    令 次数 = 次数 + 1
  }
  打印(正文)
}
```

## 完整示例：一个小网站

```ling-gui
形 响应 { status, content_type, body, set_cookie, location }

函 首页(请求) {
  归 read_file("index.html")
}

函 问候接口(请求) {
  令 名字 = query_param(请求.query, "name", "世界")
  归 响应(200, "application/json", "{\"hello\":\"" + json_escape(名字) + "\"}", "", "")
}

令 始 = 执 {
  http_static("/css", "css")
  http_static("/images", "images")
  http_route("GET", "/", 首页)
  http_route("GET", "/api/hello", 问候接口)
  http_serve(cli_arg("host", "127.0.0.1"), to_number(cli_arg("port", "8080")), 4)
}
```

用 `ling run site.ling --port 8080` 运行，然后打开 `http://127.0.0.1:8080/`
或 `http://127.0.0.1:8080/api/hello?name=Ada`。

## 另请参阅

- [数据库（SQLite）](../reference/database.md)：`db_open`、`db_exec`、`db_query`。
- [一个小型 Web 服务](../tutorials/web-service.md)：一步步搭建完整的服务。
- [网络](../reference/networking.md)：大厅联网、SSH 和点对点。
- [ling-http 与 ling-web](../crates/web.md)：直接在 Rust 中使用同一套技术栈。
