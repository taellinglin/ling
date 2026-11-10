# 网络

除了 HTTP，Ling 还有四组网络内置函数。按你要做的东西来选：

| 你想要 | 使用 | 安全性 |
|---|---|---|
| 游戏大厅或局域网工具 | `net_*` | 明文，无认证。只在可信的局域网中使用。 |
| 在服务器上执行命令，或接受 SSH 客户端 | `ssh_*` | SSH-2，固定主机密钥，密码登录。 |
| 私有的请求/响应服务 | `lingtp_*` | 后量子握手，固定主机密钥。 |
| 由带身份的节点组成的点对点网络 | `p2p_*` | 后量子，双向认证，帧填充。 |

HTTP 和 HTTPS 请看 [Web 服务器与 HTTP](../reference/web.md)。`net_*` 包含在所有安装中；`ssh_*`、
`lingtp_*` 和 `p2p_*` 需要 `privacy` 安装配置（默认的 `full` 安装已包含）。

这些内置函数都是阻塞或轮询式的，没有一个接受回调。你要写一个循环来检查有没有新工作，并在两次检查之间
调用 `sleep(ms)`。绑定端口或连接失败会体现在状态数字里，而不是作为错误抛出，所以一定要检查状态。

## 局域网大厅：`net_*`

`net_*` 是一个小巧可靠的游戏状态传输层。一个进程要么是**主机**，最多接受 64 个客户端；要么是**客户端**，
连接到一个主机。每条消息是一行 UTF-8 文本。套接字在后台线程中运行，所以游戏循环永远不会阻塞：
`net_send` 把一行放入队列，`net_recv` 取出一行。

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `net_host(port)` | — | — | — | `เน็ตโฮสต์` | 监听 TCP 端口（默认 7777），成为主机 |
| `net_join(ip, port)` | — | — | — | `เน็ตจอย` | 连接到主机，成为客户端 |
| `net_send(text)` | — | — | — | `เน็ตส่ง` | 发送一行：主机发给所有客户端，客户端发给主机 |
| `net_send_to(id, text)` | — | — | — | — | 仅主机：给一个客户端发送一行 |
| `net_recv()` | — | — | — | `เน็ตรับ` | 最早排队的消息，或 `""` |
| `net_recv_from()` | — | — | — | — | 同上，格式为 `"id\|text"` |
| `net_clients()` | — | — | — | — | 主机：客户端 ID，如 `"1,2"` |
| `net_client_count()` | — | — | — | — | 已连接的对端数量 |
| `net_events()` | — | — | — | — | 取出 `connect:<id>` / `disconnect:<id>` 行 |
| `net_status()` | — | — | — | `เน็ตสถานะ` | `0` 空闲或失败，`1` 等待中，`2` 已连接 |
| `net_close()` | — | — | `연결종료` | — | 关闭主机或客户端 |
| `net_announce(port, info)` | — | — | — | `เน็ตประกาศ` | 主机：在局域网上广播大厅描述 |
| `net_announce_stop()` | — | — | — | `เน็ตหยุดประกาศ` | 停止广播 |
| `net_discover(port)` | — | — | — | `เน็ตค้นหา` | 客户端：最近 5 秒内听到的主机 |
| `net_test(port)` | — | — | — | `เน็ตทดสอบ` | 报告端口能否绑定以及局域网地址 |
| `net_local_ip()` | — | — | — | — | 本机的局域网地址，离线时为 `""` |
| `net_upnp_map(port, description)` | — | — | — | — | 请求路由器转发一个 TCP 端口（在后台进行） |
| `net_upnp_poll()` | — | — | — | — | `net_upnp_map` 的结果 |
| `net_upnp_unmap()` | — | — | — | — | 删除端口转发 |

### 作为主机与加入

`net_host(port)` 和 `net_join(ip, port)` 不返回任何值；请观察 `net_status()`。在主机上，`1` 表示正在等待
客户端，`2` 表示至少有一个已连接。在客户端上，`1` 表示正在连接，`2` 表示已连接。如果端口无法绑定或
连接失败，状态会回到 `0`。每个进程只有一个角色：再次调用 `net_host` 或 `net_join` 会替换原来的角色。

```ling-gui
# 主机.ling
令 始 = 执 {
    net_host(17777)
    令 次数 = 0
    循 次数 < 100 {
        令 消息 = net_recv()
        若 消息 != "" {
            打印("主机收到：" + 消息)
            net_send("回声：" + 消息)
            令 次数 = 1000
        }
        睡眠(50)
        令 次数 = 次数 + 1
    }
    睡眠(300)
    net_close()
}
```

```ling-gui
# 客户端.ling
令 始 = 执 {
    net_join("127.0.0.1", 17777)
    令 次数 = 0
    循 net_status() != 2 && 次数 < 100 {
        睡眠(50)
        令 次数 = 次数 + 1
    }
    net_send("你好")
    令 回复 = ""
    循 回复 == "" && 次数 < 200 {
        令 回复 = net_recv()
        睡眠(50)
        令 次数 = 次数 + 1
    }
    打印("客户端收到：" + 回复)
    net_close()
}
```

在一个终端运行主机，在另一个终端运行客户端。客户端会打印 `客户端收到：回声：你好`。

### 消息与事件

- 消息中不能有换行符：嵌入的换行会变成两条消息。
- 每个接收队列最多保存 4096 行；队列满时，最早的行会被丢弃。
- `net_recv()` 和 `net_recv_from()` 读取的是同一个队列，所以每个程序只用其中一种方式。
  `net_recv_from()` 返回 `"2|hello"`；请在**第一个** `|` 处拆分，因为消息本身也可能包含 `|`。
- `net_events()` 保留最近 256 个事件，以换行连接。

### 在局域网中寻找主机

`net_announce(port, info)` 大约每秒一次通过 UDP（默认端口 7778）广播 `info`。再次调用会更新文本。
`net_discover(port)` 返回最近 5 秒内听到的主机，每行一个 `"ip|info"`。第一次调用会启动监听，所以在
收到广播之前它返回 `""`。

```ling-gui
令 始 = 执 {
    令 主机列表 = ""
    令 次数 = 0
    循 主机列表 == "" && 次数 < 40 {
        令 主机列表 = net_discover(7778)
        睡眠(250)
        令 次数 = 次数 + 1
    }
    打印(主机列表)
}
```

### 路由器端口转发

`net_upnp_map(port, description)` 请求路由器（UPnP IGD）把一个 TCP 端口转发到本机。`net_upnp_poll()`
在处理期间返回 `""`，之后返回 `"ok|<公网 IP>"`、`"cgnat|<ip>"`（路由器本身处于运营商级 NAT 之后，
转发无法到达你）或 `"fail|<原因>"`。`net_upnp_unmap()` 删除转发。

### 安全

主机监听所有网络接口，流量是明文，任何能访问该端口的人都能加入。请在可信的局域网或隧道中使用 `net_*`。
跨越互联网时，请使用 `lingtp_*`、`ssh_*` 或 HTTPS。`net_test` 和 `net_local_ip` 通过向一个公网地址
打开 UDP 套接字来得知局域网地址，但不会发送任何数据。

## SSH：`ssh_*`

SSH 内置函数包括一个真正的 SSH-2 客户端和一个小型 SSH 服务器。客户端可以连接任何 OpenSSH 服务器。
服务器让 Ling 程序接收来自 OpenSSH 客户端的 shell 输入行和 `exec` 请求。它们都没有本地化别名。

| 内置函数 | 用途 |
|---|---|
| `ssh_connect(host, port, user, password)` | 连接并登录：`1` 成功，`0` 凭据错误，`-1` 连接错误，`-2` 主机密钥已改变 |
| `ssh_exec(command)` | 执行一条命令，返回其标准输出 |
| `ssh_exit_code()` | 上一次 `ssh_exec` 的退出状态（尚无时为 `-1`） |
| `ssh_client_status()` | `0` 未连接，`2` 已连接 |
| `ssh_close()` | 断开客户端 |
| `ssh_config(port, user, password, host_key_path)` | 准备服务器设置 |
| `ssh_serve()` | 在后台启动服务器；正在监听时为 `true` |
| `ssh_serve_recv()` | 任一客户端最早的输入，格式为 `"id\|text"`，或 `""` |
| `ssh_serve_send(id, text)` | 给客户端发送一行 |
| `ssh_serve_exit(id, code)` | 以退出状态结束 `exec` 请求并关闭通道 |
| `ssh_serve_clients()` | 已连接的客户端 ID，以逗号分隔 |
| `ssh_serve_client_count()` | 已连接的客户端数量 |
| `ssh_serve_events()` | 取出 `connect:`、`disconnect:` 和 `authfail:` 行 |
| `ssh_serve_status()` | `0` 空闲，`1` 监听中，`2` 有客户端连接 |
| `ssh_stop()` | 停止监听并断开所有客户端 |

### 客户端

```ling-gui
令 始 = 执 {
    令 结果 = ssh_connect("192.168.1.20", 22, "deploy", env_get("DEPLOY_PASSWORD", ""))
    若 结果 == 1 {
        打印(ssh_exec("uname -a"))
        打印(ssh_exit_code())
        ssh_close()
    } 否则 {
        打印("ssh 失败：" + 结果)
    }
}
```

客户端会用 OpenSSH 格式的 `known_hosts` 文件检查每个主机密钥。第一次连接某个主机时会信任它的密钥，
与 OpenSSH 相同；密钥改变则拒绝连接（`-2`）。认证只支持密码。不要把密码写进源代码，请用 `env_get` 读取。

### 服务器

`ssh_config` 依次接受端口（默认 22）、唯一允许的用户名、不能为空的密码，以及 Ed25519 主机密钥文件
（`""` 表示 `.ling-ssh/host_ed25519`，首次启动时创建）。服务器只提供密码登录，并以常数时间比较密码。
它不会替你执行命令：它把客户端输入的内容放入队列，由你的代码决定如何回答。它是一个协议端点，而不是 shell。

```ling-gui
令 始 = 执 {
    ssh_config(2222, "admin", env_get("SSH_PASSWORD", ""), "")
    若 ssh_serve() {
        循 真 {
            令 输入 = ssh_serve_recv()
            若 输入 != "" {
                令 部分 = split(输入, "|")
                ssh_serve_send(to_number(部分[0]), "你说：" + 部分[1])
                ssh_serve_exit(to_number(部分[0]), 0)
            }
            睡眠(50)
        }
    }
}
```

> 已知限制：在用 Ling 客户端连接 Ling 服务器的本机回环测试中，`ssh_exec` 返回了 `""` 和退出码 `-1`，
> 而不是服务器的回复。对端请优先使用 OpenSSH，并在依赖它之前测试你自己的组合。

## 后量子传输：`lingtp://`

`lingtp://` 是一个小型请求/响应协议，扮演 HTTPS 的角色，但使用后量子密码学，并且没有证书颁发机构。
每个连接都以握手开始：服务器用 **ML-DSA-87** 签名证明身份，双方用 **X25519 + ML-KEM-768** 混合 KEM
协商密钥。之后每一帧都用 XChaCha20-Poly1305 封印，并再用 DICE-42 级联包裹一次。每个连接的密钥都是新的，
从而提供前向保密。

信任方式与 SSH 相同。客户端第一次见到某个主机时，会把主机公钥保存到 `.lingtp/known_hosts`（相对于
当前文件夹）。如果该主机之后出示不同的密钥，连接会被拒绝。如果网络可能有敌意，第一次连接时请通过
其他渠道核对密钥。

请求是一个字符串（路径，包括查询字符串），响应也是一个字符串（正文）。POST 发送的是
`path + "\n" + body`，所以处理函数通过查找换行符来区分两者。

| 内置函数 | 用途 |
|---|---|
| `lingtp_get(host, port, path)` | 获取一个路径（端口默认 7780）；返回正文，失败时为 `""` |
| `lingtp_post(host, port, path, body)` | 发送正文；返回响应正文 |
| `lingtp_get_status()` | 上一次请求的结果：`1` 成功，`0` 连接错误，`-1` 服务器签名无效，`-2` 主机密钥已改变 |
| `lingtp_config(port, host_key_path)` | 准备服务器设置 |
| `lingtp_route(prefix, handler)` | 应答以 `prefix` 开头的路径（最长前缀优先） |
| `lingtp_serve()` | 一直服务到 `lingtp_stop`；会阻塞调用线程 |
| `lingtp_stop()` | 停止服务器 |
| `lingtp_status()` | `0` 已停止，`1` 监听中，`2` 有客户端连接 |
| `lingtp_client_count()` | 已连接的客户端数量 |
| `lingtp_events()` | 取出 `connect:<id>` / `disconnect:<id>` 行 |

它们都没有本地化别名。

### 提供服务

请在调用 `lingtp_serve()` 之前注册路由：它启动时会清空路由表。如果没有任何配置，或者无法创建密钥或
套接字，它返回 `false`。处理函数接收路径，返回正文。

```ling-gui
函 问候(路径) {
    归 "来自后量子的问候：" + 路径
}

令 始 = 执 {
    lingtp_config(17780, "hostkey.hex")
    lingtp_route("/", 问候)
    打印("服务中")
    lingtp_serve()
}
```

### 获取

```ling-gui
令 始 = 执 {
    令 正文 = lingtp_get("127.0.0.1", 17780, "/hi?x=1")
    若 lingtp_get_status() == 1 {
        打印(正文)
    } 否则 {
        打印("lingtp 失败：" + lingtp_get_status())
    }
}
```

连接上面的服务器时，它会打印 `来自后量子的问候：/hi?x=1`。

### 安全说明

- 主机密钥以十六进制种子的形式保存在普通文件中（默认是 `.lingtp/host_mldsa87`，或你传给
  `lingtp_config` 的路径）。请像保护 SSH 私钥一样保护它并做好备份：一旦丢失，服务器的身份就会改变，
  所有客户端都会拒绝连接。
- 处理函数出错时，会以 `handler error: ...` 的形式返回给客户端，可能泄露内部信息。请校验输入，避免在
  处理函数中出错。
- 服务器监听所有网络接口，一次只应答一个请求。
- `lingtp_post` 把第一个换行视为正文的开始，所以路径中的换行要做百分号编码。
- Rust crate `ling-net` 有自己的 `lingtp` 模块，线路格式不同。Ling 的 `lingtp_get` 无法与 `ling-net`
  服务器通信。两端请使用同一种实现。

## 点对点：`p2p_*`

`p2p_*` 运行 LWP-1，即 Ling Wallet 项目的传输层。对端在混合 X25519 + ML-KEM-1024 握手中用 ML-DSA-87
身份相互认证，帧被填充到固定大小后封印。节点会维护对端数据库，可以使用签名的种子列表和 SOCKS5 代理
（例如 `127.0.0.1:9050` 上的 Tor），并用 Dandelion++ 中继交易。载荷是十六进制字符串，带有一个由你的
程序定义含义的 `kind` 数字。

| 内置函数 | 用途 |
|---|---|
| `p2p_start(config, events)` | 启动节点；返回 `Ok(handle)` 或 `Err(message)`。`events` 传 `""` |
| `p2p_poll(net, max, timeout_ms)` | 取出最多 `max` 个事件（默认 64），最多等待 `timeout_ms` |
| `p2p_send(net, peer, kind, payload_hex)` | 发送给一个对端；入队成功时为 `true` |
| `p2p_broadcast(net, kind, payload_hex, except_peer)` | 发送给除某个对端外的所有对端（`-1` 表示不排除） |
| `p2p_relay(net, id_hex, payload_hex, kind, from_peer)` | 用 Dandelion++ 中继已验证的交易 |
| `p2p_mark_fluffed(net, id_hex)` | 记录某笔交易已经广播过 |
| `p2p_peers(net)` | 每个对端为 `[id, addr, inbound, fingerprint, is_seed, connected_secs]` |
| `p2p_misbehaving(net, peer, score, reason)` | 增加对端的封禁分数 |
| `p2p_local_addr(net)` | 监听地址 `"ip:port"`，或 `""` |
| `p2p_fingerprint(identity_seed_hex)` | 对端会看到的种子指纹，如 `9ebc-56af-97f2-61af` |
| `p2p_netgroup(net, peer)` | 对端的网络分组，如 `v4:1.2` |
| `p2p_seeds_sign(network, issued_at, entries, authority_seed_hex)` | 以种子权威身份签名种子列表 |
| `p2p_seeds_verify(list_hex, authority_pk_hex)` | `Ok([network, issued_at, entries])` 或 `Err(...)` |
| `p2p_shutdown(net)` | 停止节点；第一次调用时为 `true` |

它们都没有本地化别名。`str_to_hex(text)` 和 `hex_to_str(hex)` 用于在文本和十六进制之间转换载荷。

### 配置

`config` 是由 `[key, value]` 对组成的列表。未知的键、重复的键和类型错误都会以 `Err(message)` 拒绝。

| 键 | 含义 |
|---|---|
| `magic_hex` | **必需。** 8 个十六进制字符；magic 不同的对端永远不会连接。 |
| `network` | **必需。** 网络 ID，0–255。 |
| `data_dir` | **必需。** 存放对端数据库和身份的文件夹；不存在时会创建。 |
| `listen` | 接受对端的 `"ip:port"`，或 `""` 表示只出站。端口 `0` 会选一个空闲端口。 |
| `peers` | 总是尝试连接的 `"host:port"` 对端列表。 |
| `identity_seed_hex` | 长期身份的 64 个十六进制字符；省略则使用随机身份。 |
| `ephemeral_outbound` | 每个出站连接使用新的身份（默认 `true`）。 |
| `max_outbound`、`max_inbound` | 连接数上限（默认 8 和 32）。 |
| `proxy` | 所有出站流量使用的 SOCKS5 代理 `"ip:port"`。 |
| `seeds_hex`、`authority_pk_hex` | 签名的种子列表，以及必须能验证它的公钥。 |
| `fast_timers` | 缩短计时器，用于测试。 |

### 事件

`p2p_poll` 返回一个事件列表，每个事件本身也是一个列表：

| 第一个元素 | 结构 |
|---|---|
| `"connected"` | `["connected", peer_id, addr, inbound, fingerprint, is_seed]` |
| `"disconnected"` | `["disconnected", peer_id]` |
| `"message"` | `["message", peer_id, kind, payload_hex]` |
| `"stem"` | `["stem", peer_id, payload_hex]`：需要验证的 Dandelion++ 交易 |

请持续轮询：事件在一个有界队列中等待，队列满了会拖慢所有对端。消息的 kind 为 16–255（更小的值保留），
载荷最大约 4 MiB。

### 示例

同一台机器上的两个节点：A 监听，B 连接到 A 并发送 `你好`。

```ling-gui
函 解包(结果) { 归 配 结果 { 好(值) => 值, 坏(错) => "错误：" + 错 } }

令 始 = 执 {
    令 甲 = 解包(p2p_start([["magic_hex", "4c494e47"], ["network", 1], ["listen", "127.0.0.1:0"], ["data_dir", "node_a"]], ""))
    令 乙 = 解包(p2p_start([["magic_hex", "4c494e47"], ["network", 1], ["listen", ""], ["data_dir", "node_b"], ["peers", [p2p_local_addr(甲)]]], ""))
    令 对端 = []
    令 次数 = 0
    循 长度(对端) == 0 && 次数 < 100 {
        令 对端 = p2p_peers(乙)
        睡眠(100)
        令 次数 = 次数 + 1
    }
    p2p_send(乙, 对端[0][0], 16, str_to_hex("你好"))
    令 收到 = ""
    令 次数 = 0
    循 收到 == "" && 次数 < 20 {
        令 事件 = p2p_poll(甲, 10, 500)
        令 序号 = 0
        循 序号 < 长度(事件) {
            若 事件[序号][0] == "message" { 令 收到 = hex_to_str(事件[序号][3]) }
            令 序号 = 序号 + 1
        }
        令 次数 = 次数 + 1
    }
    打印("A 收到：" + 收到)
    p2p_shutdown(甲)
    p2p_shutdown(乙)
}
```

### 安全说明

- 在受限的隔离区中，每个 `p2p_*` 内置函数都需要 `"p2p"` 能力。主线程拥有全部能力。
- 网络句柄是一个无法猜测的 128 位十六进制字符串，只在启动它的隔离区中有效。未知句柄会引发错误；
  `p2p_shutdown` 之后，调用会返回 `false` 或 `[]`。
- `data_dir` 保存着节点的身份。请像保护私钥一样保护它。

## 另请参阅

- [Web 服务器与 HTTP](../reference/web.md)：`http_*`。
- [密码学](../reference/crypto.md)：`lingtp://` 和 `p2p_*` 背后的原语。
