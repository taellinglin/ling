# 密码学

Ling 内置了密码学功能：哈希、认证加密、密码哈希、经典签名与后量子签名、混合后量子密钥交换、
双因素验证码，以及带 BIP-39 助记词的 Ling Wallet 原语（`lw_*`）。这些内置函数是一层以字符串为接口的
薄封装，底层是 `ling-crypto` crate 中经过审查的 Rust 实现（RustCrypto、dalek、BLAKE3、`ml-kem`、
`ml-dsa`、Argon2）。

所需的安装配置：

| 内置函数 | 安装配置 |
|---|---|
| 哈希、`crypto_seal`/`crypto_open`、签名、`knot_*`、`password_*` | `web` 或 `privacy`（两者都包含密码学组件） |
| `totp_secret`、`totp_now` | `web` |
| `lw_*` | `privacy` |

默认安装（`full`）包含以上全部。如果缺少某个安装配置，调用其中的内置函数会报错，并提示应该用哪个
安装配置重新安装。

## 约定

- **十六进制进，十六进制出。** 密钥、nonce、密文、摘要和签名都是小写十六进制字符串。32 字节的密钥
  是 64 个十六进制字符。
- **句柄。** `ed25519_keygen`、`mldsa_keygen`、`mldsa87_keygen` 和 `knot_keygen` 返回一个小数字，
  指向解释器内部的密钥表。`lw_*` 系列则返回 32 个字符的十六进制句柄。私钥字节永远不会出现在你的代码里。
- **结果。** 输入错误时可能失败的内置函数会返回结果。打印出来是 `Ok(...)` 或 `Err(message)`。
  用 `match r { ok(v) => ..., bad(e) => ... }` 解包。
- **拼错的名字不会报错。** 未知函数的值是 `()`。如果某个调用"什么都没做"，先检查拼写。

## 安全建议

1. **使用能完成任务的最高层内置函数。** 用已有密钥加密，就用 `crypto_seal`；存储密码，就用
   `password_hash`。不要自己用哈希和异或拼凑方案。
2. **密钥必须是 32 字节的真随机数。** `crypto_seal`、`crypto_open` 和 `*_from_seed` 系列对十六进制的
   解析很宽松：短密钥会补零，长密钥会被截断，无效的十六进制字符对会被丢弃。打错一个字得到的是弱密钥，
   而不是错误。用 `random_hex(32)` 生成密钥。把口令变成密钥要用 `lw_password_key`（Argon2id），
   绝不要只做一次哈希。
3. **`password_hash` 用于登录，`lw_password_key` 用于生成密钥。**
4. **绝不重复使用 nonce。** `crypto_seal` 每次调用都会随机生成新的 nonce。`lw_aead_seal` 的 nonce
   由你提供：每条消息都用 `random_hex(24)`。
5. **不要用游戏随机数生成器产生秘密。** `rand` 是给游戏逻辑用的。请用 `random_hex` 或 `lw_random`，
   它们读取操作系统的安全随机数生成器。
6. **字符串不会被清零。** Ling 变量里的十六进制秘密，会和该字符串存活得一样久。API 提供句柄时就把
   秘密留在句柄里，并用 `lw_key_drop` 销毁 `lw_*` 密钥。
7. **用验证类内置函数做比较。** 字符串的 `==` 不是常数时间的。请使用 `ed25519_verify`、
   `mldsa87_verify`、`password_verify` 和 `totp_verify`。
8. **`crypto_hash` 不是普通的 SHA3-256。** 它加入了 Ling 专用的标签，因此与其他工具的结果不同。
   需要互通时请用 `sha256_hex` 或 `hash_hex`。
9. **把 Ling 特有的设计视为未经审计。** 几何层（`knot_points`、`holo_points`、`crypto_hash`）、
   `lingtp://` 内部的 DICE-42 级联以及 LRS-87 环签名，都是构建在标准原语之上的原创设计。LRS-87 被标记为
   实验性。对高价值系统，请直接使用标准原语，并自行安排审查。
10. **这里的后量子都是混合方案。** KEM 内置函数把 X25519 与 ML-KEM 结合起来，即使其中一半被攻破，
    秘密依然安全。被篡改的 KEM 密文会得到一个不同的秘密而不是错误，所以一定要用这个秘密去认证某些内容。

## 哈希与随机字节

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `sha256_hex(text)` | `SHA256哈希` | — | — | — | SHA-256，64 个十六进制字符 |
| `hash_hex(text)` | — | — | — | `แฮชสตริง` | BLAKE3，64 个十六进制字符 |
| `file_hash(path)` | — | — | — | `แฮชไฟล์` | 文件原始字节的 BLAKE3 |
| `crypto_hash(text)` | `几何哈希` | `幾何ハッシュ` | `기하해시` | `แฮชเข้ารหัส` | Ling 的带标签 SHA3-256 |
| `shake_hex(text, len)` | `SHAKE哈希` | — | — | — | SHAKE-256，输出 `len` 字节 |
| `hash_str(text)` | — | — | — | `แฮช` | 快速的非密码学哈希，0 到 1 |
| `hash_int(text, n)` | — | — | — | `แฮชจำนวน` | 快速的非密码学哈希，0 到 `n - 1` |
| `random_hex(n)` | — | — | `무작위16진수` | — | `n` 个安全随机字节的十六进制 |

### `sha256_hex`、`hash_hex`、`file_hash`

`sha256_hex` 与其他所有 SHA-256 工具一致，包括浏览器的 `crypto.subtle.digest`。`hash_hex` 是
BLAKE3：快速且抗碰撞。`file_hash` 用 BLAKE3 对文件的原始字节做哈希（可处理二进制），文件无法读取时
返回 `""`。它们都不是密码哈希。

```ling
令 始 = 执 {
    打印(sha256_hex("abc"))
    打印(hash_hex("abc"))
    write_file("note.txt", "你好，文件")
    打印(file_hash("note.txt") == hash_hex("你好，文件"))
    remove_file("note.txt")
}
```

```output
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85
true
```

### `crypto_hash`、`shake_hex`

`crypto_hash(text)` 计算 `SHA3-256("ling-geo-holo-hash-v1" + text)`，也就是 `knot_points` 所绘制的
摘要。`shake_hex(text, len)` 从 SHAKE-256 中挤出 `len` 字节（默认 32）。

```ling
令 始 = 执 {
    打印(几何哈希("abc"))
    打印(shake_hex("abc", 16))
}
```

```output
0f5ae6a06fa73d3e18615b33ee9a9efafdb1a25a7bbf5f3ca9c4d7f1ac4ddfb9
483366601360a8771c6863080cc4114d
```

### `hash_str`、`hash_int`

用于种子、颜色和分桶的 FNV-1a 哈希。它们是确定性的，而且**不安全**：绝不要用在攻击者能控制的数据上。
`hash_int` 的 `n` 默认为 100。

### `random_hex(n)`

从操作系统的安全随机数生成器取 `n` 个字节（默认 32，限制在 1–256），返回 `2 * n` 个十六进制字符。
用它生成密钥、nonce 和会话 ID。

```ling
令 始 = 执 {
    令 密钥 = random_hex(32)
    打印(长度(密钥))
}
```

```output
64
```

## 封印消息

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `crypto_seal(key_hex, text)` | `封印` | `封印する` | `봉인` | `ผนึก` | 用 XChaCha20-Poly1305 加密并认证 |
| `crypto_open(key_hex, sealed_hex)` | `解封` | `封印解除` | `봉인해제` | `เปิดผนึก` | 验证并解密 |

`crypto_seal` 以十六进制返回 `nonce（24 字节）+ 密文 + 标签（16 字节）`。同一段文本封印两次，结果
也不同，因为每次调用都会生成新的 nonce。`crypto_open` 返回原文；如果密钥错误或有任何一位被改动，
则返回 `Err("decryption/auth failed")`。原文按 UTF-8 解码，所以二进制数据要先编码（例如用
`base64_encode`）。

```ling
令 始 = 执 {
    令 密钥 = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
    令 密文 = 封印(密钥, "中午见")
    打印(解封(密钥, 密文))
    打印(解封("ff" + 密钥, 密文))
}
```

```output
中午见
Err(decryption/auth failed)
```

## 密码

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `password_hash(password)` | — | — | `비밀번호해시` | — | Argon2id 哈希，PHC 字符串格式 |
| `password_verify(password, stored)` | — | — | `비밀번호확인` | — | 用存储的哈希校验密码 |

`password_hash` 使用 Argon2id，参数为 64 MiB 内存、3 轮、4 条通道，并带有新的随机盐，返回形如
`$argon2id$v=19$m=65536,t=3,p=4$...` 的字符串。请完整存储这个字符串。每次调用按设计需要几十毫秒。
`password_verify` 从存储的字符串中读取参数，返回 `true` 或 `false`（格式错误的字符串返回 `false`）。

```ling
令 始 = 执 {
    令 存储值 = password_hash("正确的马")
    打印(substr(存储值, 0, 30))
    打印(password_verify("正确的马", 存储值))
    打印(password_verify("错误", 存储值))
}
```

```output
$argon2id$v=19$m=65536,t=3,p=4
true
false
```

## 签名

三个签名系列用法相同：生成密钥对（随机或由 32 字节种子派生），读取公钥，对字符串签名，用公钥验证。

| 系列 | 前缀 | 公钥 | 签名 | 说明 |
|---|---|---|---|---|
| Ed25519 | `ed25519_` | 32 字节 | 64 字节 | 经典算法，快速，确定性 |
| ML-DSA-65（FIPS 204） | `mldsa_` | 1952 字节 | 3309 字节 | 后量子，NIST 第 3 类 |
| ML-DSA-87（FIPS 204） | `mldsa87_` | 2592 字节 | 4627 字节 | 后量子，NIST 第 5 类；`lingtp://` 使用 |

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `ed25519_keygen()` | `生成签名密钥` | `署名鍵生成` | `생성서명키` | `สร้างกุญแจลายเซ็น` | 随机密钥对句柄 |
| `ed25519_keygen_from_seed(seed_hex)` | `从种子生成签名密钥` | `シードから署名鍵生成` | `씨앗에서생성서명키` | `สร้างกุญแจลายเซ็นจากเมล็ด` | 由 32 字节种子派生密钥对 |
| `ed25519_public(handle)` | `签名公钥` | `署名公開鍵` | `서명공개키` | `กุญแจสาธารณะลายเซ็น` | 十六进制公钥 |
| `ed25519_sign(handle, message)` | `签名` | `署名する` | `서명하다` | `เซ็นชื่อ` | 十六进制签名 |
| `ed25519_verify(public_hex, message, sig_hex)` | `验证签名` | `署名検証` | `서명확인` | `ยืนยันลายเซ็น` | 有效时为 `true` |
| `mldsa_keygen()` | `生成量子签名密钥` | `量子署名鍵生成` | `양자서명키생성` | `สร้างกุญแจลายเซ็นควอนตัม` | 随机 ML-DSA-65 密钥对 |
| `mldsa_keygen_from_seed(seed_hex)` | `从种子生成量子签名密钥` | `シードから量子署名鍵生成` | `씨앗에서양자서명키생성` | `สร้างกุญแจลายเซ็นควอนตัมจากเมล็ด` | 由种子派生 ML-DSA-65 密钥对 |
| `mldsa_public(handle)` | `量子签名公钥` | `量子署名公開鍵` | `양자서명공개키` | `กุญแจสาธารณะลายเซ็นควอนตัม` | 十六进制公钥 |
| `mldsa_sign(handle, message)` | `量子签名` | `量子署名する` | `양자서명하다` | `เซ็นชื่อควอนตัม` | 十六进制签名 |
| `mldsa_verify(public_hex, message, sig_hex)` | `验证量子签名` | `量子署名検証` | `양자서명확인` | `ยืนยันลายเซ็นควอนตัม` | 有效时为 `true` |
| `mldsa87_keygen()` | — | — | — | — | 随机 ML-DSA-87 密钥对 |
| `mldsa87_keygen_from_seed(seed_hex)` | — | — | — | — | 由种子派生 ML-DSA-87 密钥对 |
| `mldsa87_public(handle)` | — | — | — | — | 十六进制公钥 |
| `mldsa87_sign(handle, message)` | — | — | — | — | 十六进制签名 |
| `mldsa87_verify(public_hex, message, sig_hex)` | — | — | — | — | 有效时为 `true` |

### 使用签名

有了种子，你只需保存 64 个十六进制字符，重启后就能重建同一个密钥对；请像保护私钥一样保护种子。
未知句柄会让 `*_public` 和 `*_sign` 返回 `""`。签名无效或输入格式错误时，验证类内置函数返回 `false`。
Ed25519 签名是确定性的：同一密钥和同一消息总是得到同一个签名。

对于许可证文件、发布清单这类长期有效的产物，请同时用 Ed25519 **和** ML-DSA-87 签名，并且只有两者都
验证通过时才接受：

```ling
令 始 = 执 {
    令 种子 = "0101010101010101010101010101010101010101010101010101010101010101"
    令 经典 = ed25519_keygen_from_seed(种子)
    令 量子 = mldsa87_keygen_from_seed(种子)
    令 文档 = "许可证：acme，2031-01-01 到期"
    令 签名甲 = ed25519_sign(经典, 文档)
    令 签名乙 = mldsa87_sign(量子, 文档)
    打印(长度(ed25519_public(经典)))
    打印(ed25519_verify(ed25519_public(经典), 文档, 签名甲) && mldsa87_verify(mldsa87_public(量子), 文档, 签名乙))
    打印(mldsa87_verify(mldsa87_public(量子), 文档 + "！", 签名乙))
}
```

```output
64
true
false
```

## 后量子密钥交换

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `knot_keygen()` | `生成密钥` | `鍵生成` | `키생성` | `สร้างกุญแจปม` | 混合 X25519 + ML-KEM-768 密钥对 |
| `knot_public(handle)` | `公钥` | `公開鍵` | `공개키` | `กุญแจสาธารณะปม` | 十六进制公钥（1216 字节） |
| `knot_encapsulate(public_hex)` | `封装密钥` | `カプセル化` | `캡슐화` | `ห่อกุญแจปม` | `[ciphertext_hex, secret_hex]` |
| `knot_decapsulate(handle, ciphertext_hex)` | `解封装密钥` | `カプセル解除` | `캡슐해제` | `แกะกุญแจปม` | 相同的 32 字节秘密 |

`hybrid_keygen`、`hybrid_public`、`hybrid_encapsulate` 和 `hybrid_decapsulate` 是英文同义名。流程是：
接收方生成密钥对并公开公钥；发送方对该公钥封装，保留秘密并发送密文；接收方解封装，得到相同的秘密。
把这个秘密用作 `crypto_seal` 的密钥。

`knot_keygen` 的密钥无法由种子生成，也无法导出：它只在进程存活期间存在。需要持久的后量子 KEM 身份时，
请使用 `lw_kem_keypair`。

```ling
令 始 = 执 {
    令 小明 = 生成密钥()
    令 封装结果 = 封装密钥(公钥(小明))
    令 小明的秘密 = 解封装密钥(小明, 封装结果[0])
    打印(封装结果[1] == 小明的秘密)
    打印(解封(小明的秘密, 封印(封装结果[1], "你好，小明")))
}
```

```output
true
你好，小明
```

## 可视指纹

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `knot_points(text)` | `结点坐标` | `結び目点` | `매듭점` | `จุดปม` | 环面纽结指纹的 256 个三维点 |
| `knot_label(text)` | `结点标签` | `結び目ラベル` | `매듭라벨` | `ป้ายปม` | 该纽结的简短名称 |
| `holo_points(text)` | `全息点` | `ホログラム点` | `홀로그램점` | `จุดโฮโลแกรม` | 全息拆分的四维点（装饰用） |
| `holo_fragment_count(text)` | `全息碎片数` | `ホログラム断片数` | `홀로그램조각수` | `จำนวนชิ้นโฮโลแกรม` | 字符串对应的碎片数 |

它们把摘要变成人眼可以比对的图形：用 `draw_line_3d` 把 `knot_points` 中相邻的三元组连起来。它们本身
不增加任何密码学强度。

```ling
令 始 = 执 {
    打印(长度(结点坐标("abc")))
    打印(结点标签("abc"))
}
```

```output
768
knot-17_12-v5.2
```

## 一次性验证码（TOTP）

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `totp_secret()` | — | — | — | `โทเทนลับ` | 新的随机 Base32 秘密（160 位） |
| `totp_uri(secret, account, issuer)` | — | — | — | `โทเทนยูอาร์ไอ` | 供验证器应用导入的 `otpauth://` 链接 |
| `totp_verify(secret, code)` | — | — | — | `โทเทนตรวจ` | 校验六位验证码 |
| `totp_now(secret)` | — | — | — | `โทเทนตอนนี้` | 当前有效的验证码 |

这里的 TOTP 遵循 RFC 6238：SHA-1、六位数字、30 秒一步，与 Google Authenticator 和 Authy 兼容。
`totp_verify` 前后各接受一步，并且不防重放：请自己记录最后一次接受的时间步。`totp_uri` 不做 URL
编码，所以账户名和发行方中不要有 `&` 或空格。可以用 `lw_qr` 把 URI 显示成二维码。

```ling
令 始 = 执 {
    打印(totp_uri("JBSWY3DPEHPK3PXP", "alice@example.com", "Acme"))
    令 秘密 = totp_secret()
    打印(长度(秘密))
    打印(totp_verify(秘密, totp_now(秘密)))
}
```

```output
otpauth://totp/Acme:alice@example.com?secret=JBSWY3DPEHPK3PXP&issuer=Acme&algorithm=SHA1&digits=6&period=30
32
true
```

## Ling Wallet 原语：`lw_*`

`lw_*` 系列是 Ling Wallet 项目的原语集。它与上面的内置函数有三点不同：

- **严格的输入。** 十六进制必须格式正确，并且长度与文档完全一致。错误的输入返回 `Err(reason)`，
  而不会被自动修正。
- **秘密句柄。** 创建秘密的内置函数返回 `Ok(handle)`，句柄是 32 个字符的十六进制。`lw_key_drop`
  会销毁密钥并清零其内存。
- **验证函数从不报错。** `lw_id_verify`、`lw_ring_verify` 和 `lw_pk_valid` 对任何格式错误的输入都
  返回 `false`。

它们都没有本地化别名。

| 内置函数 | 用途 |
|---|---|
| `lw_h256(domain, parts)` | 域分离的 SHAKE-256，输出 32 字节；`parts` 是十六进制字符串列表 |
| `lw_xof(domain, parts, len)` | 同上，输出 `len` 字节 |
| `lw_random(n)` | `n` 个安全随机字节（最多 1 MiB）的十六进制 |
| `lw_aead_seal(key, nonce, aad, plaintext)` | 使用你提供的 24 字节 nonce 和附加数据的 XChaCha20-Poly1305（全部为十六进制） |
| `lw_aead_open(key, nonce, aad, ciphertext)` | `Ok(plaintext_hex)` 或 `Err("authentication failed")` |
| `lw_password_key(password, salt, m_kib, t, p)` | Argon2id：由口令和 16 字节盐派生 32 字节密钥 |
| `lw_kem_keypair(seed)` | 由 96 字节种子生成 X25519 + ML-KEM-1024 密钥 |
| `lw_kem_public(handle)` | KEM 公钥（1600 字节） |
| `lw_kem_encaps(public_hex, context_hex)` | `Ok([ciphertext_hex, secret_hex])`，绑定到上下文 |
| `lw_kem_decaps(handle, ciphertext_hex, context_hex)` | `Ok(secret_hex)`；上下文不同则秘密不同 |
| `lw_id_keypair(seed)` | 由 32 字节种子生成 ML-DSA-87 身份 |
| `lw_id_public(handle)` | 身份公钥（2592 字节） |
| `lw_id_sign(handle, context, message_hex)` | 在上下文标签（如 `"tx.v1"`）下签名 |
| `lw_id_verify(public_hex, context, message_hex, sig_hex)` | `true` 或 `false` |
| `lw_id_fingerprint(public_hex)` | 简短指纹，如 `9ebc-56af-97f2-61af` |
| `lw_key_drop(handle)` | 销毁秘密句柄；句柄存在时返回 `true` |
| `lw_spend_keypair`、`lw_spend_public`、`lw_onetime_secret`、`lw_onetime_public`、`lw_onetime_public_of`、`lw_onetime_key_image` | LRS-87 环签名所用的密钥 |
| `lw_ring_sign(message_hex, ring, index, onetime_handle)` | 可链接环签名（实验性） |
| `lw_ring_verify(message_hex, ring, key_image_hex, sig_hex)` | `true` 或 `false` |
| `lw_pk_valid(public_hex)` | 是否为有效的 2944 字节环公钥 |
| `lw_mnemonic_from(entropy_hex)` | 把 32 字节熵编码为 24 个 BIP-39 英文单词（列表） |
| `lw_mnemonic_to(phrase)` | `Ok(entropy_hex)` 或 `Err(reason)` |
| `lw_pow_hash(header_hex)` | 内存困难的工作量证明哈希（16 MiB，按设计很慢） |
| `lw_pow_trace(header_hex, every)` | 同上，并返回用于可视化的遍历点 |
| `lw_meets_difficulty(hash_hex, difficulty)` | 哈希是否满足难度目标 |
| `lw_qr(text)` | 二维码，每行是由 `1`（深色）和 `0`（浅色）组成的字符串列表 |

在受限的隔离区（isolate）中，创建或使用秘密句柄的内置函数需要 `"keys"` 能力。主线程拥有全部能力。

### 身份

身份在上下文标签下签名；验证方必须使用相同的标签。

```ling
函 解包(结果) { 归 配 结果 { 好(值) => 值, 坏(错) => "错误：" + 错 } }

令 始 = 执 {
    令 身份 = 解包(lw_id_keypair("0101010101010101010101010101010101010101010101010101010101010101"))
    令 公开密钥 = lw_id_public(身份)
    令 签名值 = lw_id_sign(身份, "上下文", "68656c6c6f")
    打印(lw_id_verify(公开密钥, "上下文", "68656c6c6f", 签名值))
    打印(lw_id_verify(公开密钥, "其他", "68656c6c6f", 签名值))
    打印(lw_id_fingerprint(公开密钥))
    打印(lw_key_drop(身份))
}
```

```output
true
false
9ebc-56af-97f2-61af
true
```

### 由口令派生密钥

`lw_password_key` 返回 `Ok(key_hex)` 或 `Err("invalid Argon2id parameters")`。把盐和密文存放在一起；
盐不需要保密。Ling Wallet 的参数是 `m_kib = 262144`（256 MiB）、`t = 3`、`p = 1`。示例为了立即运行
使用了极小的参数，请不要照抄。

```ling
函 解包(结果) { 归 配 结果 { 好(值) => 值, 坏(错) => "错误：" + 错 } }

令 始 = 执 {
    令 盐 = "00112233445566778899aabbccddeeff"
    打印(解包(lw_password_key("hunter2", 盐, 1024, 1, 1)))
}
```

```output
a97cf9fed0c0703d0b60b7351990a06b086636d9fec2c5ef4d2b7a4bae25bb34
```

### 助记词（BIP-39）

`lw_mnemonic_from` 把 32 字节变成 24 个英文单词。`lw_mnemonic_to` 接受以空格分隔的单词（大小写
不限），并检查单词数量、单词本身和校验和。

```ling
函 解包(结果) { 归 配 结果 { 好(值) => 值, 坏(错) => "错误：" + 错 } }

令 始 = 执 {
    令 熵 = "1111111111111111111111111111111111111111111111111111111111111111"
    令 单词 = lw_mnemonic_from(熵)
    打印(长度(单词))
    打印(单词[0] + " " + 单词[1] + " " + 单词[23])
    打印(解包(lw_mnemonic_to(join(单词, " "))) == 熵)
    打印(解包(lw_mnemonic_to("abandon abandon")))
}
```

```output
24
baby mass cake
true
错误：a recovery phrase has exactly 24 words
```

### 环签名与工作量证明

LRS-87 环签名（`lw_ring_sign`）在源码中被标记为实验性。请把它用于研究和原型，而不要用于输不起的资产。
`lw_pow_hash` 会分配 16 MiB 的暂存区，并且故意很慢。`lw_meets_difficulty` 接受数字形式的难度，
对于超过 2^53 的值也接受十进制字符串。

## 另请参阅

- [网络](../reference/networking.md)：`lingtp://` 和 `p2p_*` 使用这些原语。
- [Web 服务器与 HTTP](../reference/web.md)：会话、登录与 HTTPS。
