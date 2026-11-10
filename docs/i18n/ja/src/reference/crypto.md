# 暗号

Ling には暗号機能が組み込まれています。ハッシュ、認証付き暗号、パスワードハッシュ、
従来型とポスト量子の署名、ハイブリッドなポスト量子鍵交換、二要素認証コード、そして
BIP-39 リカバリーフレーズを含む Ling Wallet のプリミティブ（`lw_*`）です。これらの組み込み関数は、
`ling-crypto` クレートにある検証済みの Rust 実装（RustCrypto、dalek、BLAKE3、`ml-kem`、
`ml-dsa`、Argon2）の上に載った、文字列ベースの薄い層です。

必要なインストールプロファイルは次のとおりです。

| 組み込み関数 | プロファイル |
|---|---|
| ハッシュ、`crypto_seal`/`crypto_open`、署名、`knot_*`、`password_*` | `web` または `privacy`（どちらも暗号の基盤を含みます） |
| `totp_secret`、`totp_now` | `web` |
| `lw_*` | `privacy` |

既定のインストール（`full`）にはすべて含まれています。プロファイルが欠けている場合、その
組み込み関数を呼ぶと、再インストールに必要なプロファイル名を示すメッセージで失敗します。

## 約束事

- **16 進数で入力し、16 進数で出力します。** 鍵、ノンス、暗号文、ダイジェスト、署名は
  小文字の 16 進文字列です。32 バイトの鍵は 16 進数で 64 文字です。
- **ハンドル。** `ed25519_keygen`、`mldsa_keygen`、`mldsa87_keygen`、`knot_keygen` は、
  インタプリタ内の鍵テーブルを指す小さな数値を返します。`lw_*` 系は代わりに 32 文字の
  16 進ハンドルを返します。秘密鍵のバイト列があなたのコードに渡ることはありません。
- **結果。** 不正な入力で失敗しうる組み込み関数は結果を返します。表示すると `Ok(...)` または
  `Err(message)` のように見えます。`match r { ok(v) => ..., bad(e) => ... }` で取り出します。
- **綴り間違いは黙って無視されます。** 未知の関数は `()` に評価されます。呼び出しが
  「何もしない」ときは、綴りを確認してください。

## セキュリティの指針

1. **目的を果たす最も高水準の組み込み関数を使ってください。** 手持ちの鍵で暗号化するなら
   `crypto_seal`、パスワードを保存するなら `password_hash` です。ハッシュと XOR で独自方式を
   作らないでください。
2. **鍵は本物のエントロピーで 32 バイト必要です。** `crypto_seal`、`crypto_open`、
   `*_from_seed` 系の組み込み関数は 16 進数を寛容に読みます。短い鍵はゼロで埋められ、長い鍵は
   切り詰められ、不正な 16 進のペアは捨てられます。打ち間違いはエラーではなく弱い鍵になります。
   鍵は `random_hex(32)` で作ってください。パスフレーズを鍵にするには `lw_password_key`
   （Argon2id）を使い、一度ハッシュするだけで済ませないでください。
3. **`password_hash` はログイン用、`lw_password_key` は鍵用です。**
4. **ノンスを再利用しないでください。** `crypto_seal` は呼び出しごとに新しいランダムなノンスを
   選びます。`lw_aead_seal` はノンスをあなたから受け取るので、メッセージごとに `random_hex(24)`
   を使ってください。
5. **秘密にゲーム用の乱数を使わないでください。** `rand` はゲームプレイ用です。
   オペレーティングシステムの安全な乱数生成器を読む `random_hex` か `lw_random` を使ってください。
6. **文字列は消去されません。** Ling の変数に入れた 16 進の秘密は、その文字列が生きている間
   残ります。API がハンドルを提供している場合は秘密をハンドルに保持し、`lw_*` の鍵は
   `lw_key_drop` で破棄してください。
7. **比較には検証用の組み込み関数を使ってください。** 文字列の `==` は定数時間ではありません。
   `ed25519_verify`、`mldsa87_verify`、`password_verify`、`totp_verify` を使ってください。
8. **`crypto_hash` は素の SHA3-256 ではありません。** Ling 固有のラベルを付け加えるので、
   他のツールとは一致しません。相互運用が必要なら `sha256_hex` か `hash_hex` を使ってください。
9. **Ling 固有の設計は未監査として扱ってください。** 幾何的な層（`knot_points`、
   `holo_points`、`crypto_hash`）、`lingtp://` 内部の DICE-42 カスケード、LRS-87 リング署名は、
   標準的なプリミティブの上に作られた独自の構成です。LRS-87 は実験的と明記されています。
   重要度の高いシステムでは標準のプリミティブを使い、独自にレビューを受けてください。
10. **ここでのポスト量子はハイブリッドです。** KEM の組み込み関数は X25519 と ML-KEM を組み合わせる
    ので、どちらか一方が破られても秘密は安全です。改ざんされた KEM 暗号文はエラーではなく
    別の秘密を生むので、秘密は必ず何かの認証に使ってください。

## ハッシュとランダムバイト

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `sha256_hex(text)` | `SHA256哈希` | — | — | — | SHA-256 を 16 進 64 文字で返す |
| `hash_hex(text)` | — | — | — | `แฮชสตริง` | BLAKE3 を 16 進 64 文字で返す |
| `file_hash(path)` | — | — | — | `แฮชไฟล์` | ファイルの生バイトの BLAKE3 |
| `crypto_hash(text)` | `几何哈希` | `幾何ハッシュ` | `기하해시` | `แฮชเข้ารหัส` | Ling のラベル付き SHA3-256 |
| `shake_hex(text, len)` | `SHAKE哈希` | — | — | — | SHAKE-256、出力は `len` バイト |
| `hash_str(text)` | — | — | — | `แฮช` | 高速な非暗号ハッシュ、0〜1 |
| `hash_int(text, n)` | — | — | — | `แฮชจำนวน` | 高速な非暗号ハッシュ、0〜`n - 1` |
| `random_hex(n)` | — | — | `무작위16진수` | — | 安全なランダムバイト `n` 個を 16 進で返す |

### `sha256_hex`、`hash_hex`、`file_hash`

`sha256_hex` は、ブラウザの `crypto.subtle.digest` を含む他のあらゆる SHA-256 ツールと
一致します。`hash_hex` は BLAKE3 で、高速かつ衝突耐性があります。`file_hash` はファイルの生バイトを
BLAKE3 でハッシュし（バイナリでも安全）、ファイルを読めなければ `""` を返します。どれも
パスワードハッシュではありません。

```ling
束縛 開始 = 実行 {
    印刷(sha256_hex("abc"))
    印刷(hash_hex("abc"))
    write_file("note.txt", "hello file")
    印刷(file_hash("note.txt") == hash_hex("hello file"))
    remove_file("note.txt")
}
```

```output
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85
true
```

### `crypto_hash`、`shake_hex`

`crypto_hash(text)` は `SHA3-256("ling-geo-holo-hash-v1" + text)` を計算します。これは
`knot_points` が描くダイジェストです。`shake_hex(text, len)` は SHAKE-256 から `len` バイト
（既定は 32）を取り出します。

```ling
束縛 開始 = 実行 {
    印刷(幾何ハッシュ("abc"))
    印刷(shake_hex("abc", 16))
}
```

```output
0f5ae6a06fa73d3e18615b33ee9a9efafdb1a25a7bbf5f3ca9c4d7f1ac4ddfb9
483366601360a8771c6863080cc4114d
```

### `hash_str`、`hash_int`

シード、色、振り分けに使う FNV-1a ハッシュです。決定的ですが **安全ではありません**。
攻撃者が操作できるものには決して使わないでください。`hash_int` の `n` の既定値は 100 です。

### `random_hex(n)`

オペレーティングシステムの安全な乱数生成器から `n` バイト（既定は 32、1〜256 に制限）を取り、
`2 * n` 文字の 16 進数で返します。鍵、ノンス、セッション ID に使ってください。

```ling
束縛 開始 = 実行 {
    束縛 鍵 = random_hex(32)
    印刷(長さ(鍵))
}
```

```output
64
```

## メッセージの封印

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `crypto_seal(key_hex, text)` | `封印` | `封印する` | `봉인` | `ผนึก` | XChaCha20-Poly1305 で暗号化と認証を行う |
| `crypto_open(key_hex, sealed_hex)` | `解封` | `封印解除` | `봉인해제` | `เปิดผนึก` | 検証して復号する |

`crypto_seal` は `ノンス（24 バイト）+ 暗号文 + タグ（16 バイト）` を 16 進数で返します。
呼び出しごとに新しいノンスを引くので、同じテキストを 2 回封印しても出力は異なります。
`crypto_open` はテキストを返し、鍵が違うか 1 ビットでも変わっていれば
`Err("decryption/auth failed")` を返します。平文は UTF-8 として復号されるので、バイナリデータは
先にエンコードしてください（たとえば `base64_encode` で）。

```ling
束縛 開始 = 実行 {
    束縛 鍵 = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
    束縛 封印済み = 封印する(鍵, "正午に集合")
    印刷(封印解除(鍵, 封印済み))
    印刷(封印解除("ff" + 鍵, 封印済み))
}
```

```output
正午に集合
Err(decryption/auth failed)
```

## パスワード

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `password_hash(password)` | — | — | `비밀번호해시` | — | Argon2id ハッシュを PHC 文字列で返す |
| `password_verify(password, stored)` | — | — | `비밀번호확인` | — | 保存済みハッシュとパスワードを照合する |

`password_hash` は 64 MiB のメモリ、3 パス、4 レーン、新しいランダムなソルトで Argon2id を使い、
`$argon2id$v=19$m=65536,t=3,p=4$...` のような文字列を返します。文字列全体を保存してください。
1 回の呼び出しには意図的に数十ミリ秒かかります。`password_verify` は保存された文字列から
パラメータを読み、`true` か `false` を返します（不正な文字列は `false`）。

```ling
束縛 開始 = 実行 {
    束縛 保存値 = password_hash("correct horse")
    印刷(substr(保存値, 0, 30))
    印刷(password_verify("correct horse", 保存値))
    印刷(password_verify("間違い", 保存値))
}
```

```output
$argon2id$v=19$m=65536,t=3,p=4
true
false
```

## 署名

3 つの系統は同じ形をしています。鍵ペアを作り（ランダム、または 32 バイトのシードから）、
公開鍵を読み、文字列に署名し、公開鍵で検証します。

| 系統 | 接頭辞 | 公開鍵 | 署名 | 備考 |
|---|---|---|---|---|
| Ed25519 | `ed25519_` | 32 バイト | 64 バイト | 従来型、高速、決定的 |
| ML-DSA-65 (FIPS 204) | `mldsa_` | 1952 バイト | 3309 バイト | ポスト量子、NIST カテゴリ 3 |
| ML-DSA-87 (FIPS 204) | `mldsa87_` | 2592 バイト | 4627 バイト | ポスト量子、NIST カテゴリ 5。`lingtp://` が使用 |

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `ed25519_keygen()` | `生成签名密钥` | `署名鍵生成` | `생성서명키` | `สร้างกุญแจลายเซ็น` | ランダムな鍵ペアのハンドル |
| `ed25519_keygen_from_seed(seed_hex)` | `从种子生成签名密钥` | `シードから署名鍵生成` | `씨앗에서생성서명키` | `สร้างกุญแจลายเซ็นจากเมล็ด` | 32 バイトのシードから鍵ペア |
| `ed25519_public(handle)` | `签名公钥` | `署名公開鍵` | `서명공개키` | `กุญแจสาธารณะลายเซ็น` | 公開鍵を 16 進で返す |
| `ed25519_sign(handle, message)` | `签名` | `署名する` | `서명하다` | `เซ็นชื่อ` | 署名を 16 進で返す |
| `ed25519_verify(public_hex, message, sig_hex)` | `验证签名` | `署名検証` | `서명확인` | `ยืนยันลายเซ็น` | 有効なら `true` |
| `mldsa_keygen()` | `生成量子签名密钥` | `量子署名鍵生成` | `양자서명키생성` | `สร้างกุญแจลายเซ็นควอนตัม` | ランダムな ML-DSA-65 鍵ペア |
| `mldsa_keygen_from_seed(seed_hex)` | `从种子生成量子签名密钥` | `シードから量子署名鍵生成` | `씨앗에서양자서명키생성` | `สร้างกุญแจลายเซ็นควอนตัมจากเมล็ด` | シードから ML-DSA-65 鍵ペア |
| `mldsa_public(handle)` | `量子签名公钥` | `量子署名公開鍵` | `양자서명공개키` | `กุญแจสาธารณะลายเซ็นควอนตัม` | 公開鍵を 16 進で返す |
| `mldsa_sign(handle, message)` | `量子签名` | `量子署名する` | `양자서명하다` | `เซ็นชื่อควอนตัม` | 署名を 16 進で返す |
| `mldsa_verify(public_hex, message, sig_hex)` | `验证量子签名` | `量子署名検証` | `양자서명확인` | `ยืนยันลายเซ็นควอนตัม` | 有効なら `true` |
| `mldsa87_keygen()` | — | — | — | — | ランダムな ML-DSA-87 鍵ペア |
| `mldsa87_keygen_from_seed(seed_hex)` | — | — | — | — | シードから ML-DSA-87 鍵ペア |
| `mldsa87_public(handle)` | — | — | — | — | 公開鍵を 16 進で返す |
| `mldsa87_sign(handle, message)` | — | — | — | — | 署名を 16 進で返す |
| `mldsa87_verify(public_hex, message, sig_hex)` | — | — | — | — | 有効なら `true` |

### 署名の使い方

シードを使えば、16 進 64 文字だけを保存しておき、再起動後に同じ鍵ペアを作り直せます。シードは
秘密鍵と同じように守ってください。未知のハンドルを渡すと `*_public` と `*_sign` は `""` を
返します。検証用の組み込み関数は、不正な署名や形式の誤った入力に対して `false` を返します。
Ed25519 の署名は決定的で、同じ鍵とメッセージからは常に同じ署名が得られます。

ライセンスファイルやリリースマニフェストのように長く使うものには、Ed25519 **と** ML-DSA-87 の
両方で署名し、両方が検証できたときだけ受け入れてください。

```ling
束縛 開始 = 実行 {
    束縛 シード = "0101010101010101010101010101010101010101010101010101010101010101"
    束縛 古典鍵 = ed25519_keygen_from_seed(シード)
    束縛 量子鍵 = mldsa87_keygen_from_seed(シード)
    束縛 文書 = "ライセンス: acme、有効期限 2031-01-01"
    束縛 署名甲 = ed25519_sign(古典鍵, 文書)
    束縛 署名乙 = mldsa87_sign(量子鍵, 文書)
    印刷(長さ(ed25519_public(古典鍵)))
    印刷(ed25519_verify(ed25519_public(古典鍵), 文書, 署名甲) && mldsa87_verify(mldsa87_public(量子鍵), 文書, 署名乙))
    印刷(mldsa87_verify(mldsa87_public(量子鍵), 文書 + "!", 署名乙))
}
```

```output
64
true
false
```

## ポスト量子鍵交換

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `knot_keygen()` | `生成密钥` | `鍵生成` | `키생성` | `สร้างกุญแจปม` | ハイブリッド X25519 + ML-KEM-768 鍵ペア |
| `knot_public(handle)` | `公钥` | `公開鍵` | `공개키` | `กุญแจสาธารณะปม` | 公開鍵（1216 バイト）を 16 進で返す |
| `knot_encapsulate(public_hex)` | `封装密钥` | `カプセル化` | `캡슐화` | `ห่อกุญแจปม` | `[ciphertext_hex, secret_hex]` |
| `knot_decapsulate(handle, ciphertext_hex)` | `解封装密钥` | `カプセル解除` | `캡슐해제` | `แกะกุญแจปม` | 同じ 32 バイトの秘密 |

`hybrid_keygen`、`hybrid_public`、`hybrid_encapsulate`、`hybrid_decapsulate` は英語の同義語です。
流れは次のとおりです。受信者が鍵ペアを作って公開鍵を公開し、送信者がそれに対してカプセル化して
秘密を手元に残し、暗号文を送ります。受信者がカプセルを解除すると同じ秘密が得られます。
この秘密を `crypto_seal` の鍵として使ってください。

`knot_keygen` の鍵はシードから作ることも書き出すこともできず、プロセスが続く間だけ存在します。
永続的なポスト量子 KEM の身元には `lw_kem_keypair` を使ってください。

```ling
束縛 開始 = 実行 {
    束縛 ボブ = 鍵生成()
    束縛 カプセル = カプセル化(公開鍵(ボブ))
    束縛 ボブの秘密 = カプセル解除(ボブ, カプセル[0])
    印刷(カプセル[1] == ボブの秘密)
    印刷(封印解除(ボブの秘密, 封印する(カプセル[1], "こんにちは、ボブ")))
}
```

```output
true
こんにちは、ボブ
```

## 視覚的なフィンガープリント

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `knot_points(text)` | `结点坐标` | `結び目点` | `매듭점` | `จุดปม` | トーラス結び目フィンガープリントの 3D 点 256 個 |
| `knot_label(text)` | `结点标签` | `結び目ラベル` | `매듭라벨` | `ป้ายปม` | その結び目の短い名前 |
| `holo_points(text)` | `全息点` | `ホログラム点` | `홀로그램점` | `จุดโฮโลแกรม` | ホログラム分割の 4D 点（装飾用） |
| `holo_fragment_count(text)` | `全息碎片数` | `ホログラム断片数` | `홀로그램조각수` | `จำนวนชิ้นโฮโลแกรม` | 文字列の断片数 |

これらはダイジェストを、人が目で見比べられる形に変えます。`knot_points` の連続する 3 つ組を
`draw_line_3d` で描いてください。それ自体が暗号強度を加えるわけではありません。

```ling
束縛 開始 = 実行 {
    印刷(長さ(結び目点("abc")))
    印刷(結び目ラベル("abc"))
}
```

```output
768
knot-17_12-v5.2
```

## ワンタイムコード（TOTP）

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `totp_secret()` | — | — | — | `โทเทนลับ` | 新しいランダムな Base32 シークレット（160 ビット） |
| `totp_uri(secret, account, issuer)` | — | — | — | `โทเทนยูอาร์ไอ` | 認証アプリ用の `otpauth://` リンク |
| `totp_verify(secret, code)` | — | — | — | `โทเทนตรวจ` | 6 桁のコードを確認する |
| `totp_now(secret)` | — | — | — | `โทเทนตอนนี้` | 今有効なコード |

ここでの TOTP は SHA-1、6 桁、30 秒ステップの RFC 6238 で、Google Authenticator や Authy と
一致します。`totp_verify` は前後 1 ステップまで受け入れ、再利用は追跡しません。最後に受け入れた
コードの時間ステップは自分で保存してください。`totp_uri` は URL エンコードを行わないので、
アカウント名と発行者名に `&` や空白を入れないでください。URI は `lw_qr` で QR コードにして表示します。

```ling
束縛 開始 = 実行 {
    印刷(totp_uri("JBSWY3DPEHPK3PXP", "alice@example.com", "Acme"))
    束縛 シークレット = totp_secret()
    印刷(長さ(シークレット))
    印刷(totp_verify(シークレット, totp_now(シークレット)))
}
```

```output
otpauth://totp/Acme:alice@example.com?secret=JBSWY3DPEHPK3PXP&issuer=Acme&algorithm=SHA1&digits=6&period=30
32
true
```

## Ling Wallet のプリミティブ: `lw_*`

`lw_*` 系は Ling Wallet プロジェクトのプリミティブ群です。上の組み込み関数とは 3 つの点で
異なります。

- **入力が厳格です。** 16 進数は正しい形式で、ちょうど規定の長さでなければなりません。不正な
  入力は修復されず、`Err(reason)` が返ります。
- **秘密のハンドル。** 秘密を作る組み込み関数は `Ok(handle)` を返します。ハンドルは 32 文字の
  16 進数です。`lw_key_drop` は鍵を破棄し、そのメモリを消去します。
- **検証関数は騒ぎ立てません。** `lw_id_verify`、`lw_ring_verify`、`lw_pk_valid` は形式の
  誤った入力に対して `false` を返します。

どれにもローカライズされた別名はありません。

| 組み込み関数 | 用途 |
|---|---|
| `lw_h256(domain, parts)` | ドメイン分離した SHAKE-256、32 バイト出力。`parts` は 16 進文字列のリスト |
| `lw_xof(domain, parts, len)` | 同じく、出力は `len` バイト |
| `lw_random(n)` | 安全なランダムバイト `n` 個（最大 1 MiB）を 16 進で返す |
| `lw_aead_seal(key, nonce, aad, plaintext)` | 自分で選んだ 24 バイトのノンスと関連データを使う XChaCha20-Poly1305（すべて 16 進） |
| `lw_aead_open(key, nonce, aad, ciphertext)` | `Ok(plaintext_hex)` または `Err("authentication failed")` |
| `lw_password_key(password, salt, m_kib, t, p)` | Argon2id: パスフレーズと 16 バイトのソルトから 32 バイトの鍵 |
| `lw_kem_keypair(seed)` | 96 バイトのシードから X25519 + ML-KEM-1024 の鍵 |
| `lw_kem_public(handle)` | KEM 公開鍵（1600 バイト） |
| `lw_kem_encaps(public_hex, context_hex)` | コンテキストに結び付いた `Ok([ciphertext_hex, secret_hex])` |
| `lw_kem_decaps(handle, ciphertext_hex, context_hex)` | `Ok(secret_hex)`。コンテキストが違えば秘密も違う |
| `lw_id_keypair(seed)` | 32 バイトのシードから ML-DSA-87 の身元 |
| `lw_id_public(handle)` | 身元の公開鍵（2592 バイト） |
| `lw_id_sign(handle, context, message_hex)` | `"tx.v1"` のようなコンテキストラベルの下で署名 |
| `lw_id_verify(public_hex, context, message_hex, sig_hex)` | `true` または `false` |
| `lw_id_fingerprint(public_hex)` | `9ebc-56af-97f2-61af` のような短いフィンガープリント |
| `lw_key_drop(handle)` | 秘密のハンドルを破棄。存在していれば `true` |
| `lw_spend_keypair`、`lw_spend_public`、`lw_onetime_secret`、`lw_onetime_public`、`lw_onetime_public_of`、`lw_onetime_key_image` | LRS-87 リング署名用の鍵 |
| `lw_ring_sign(message_hex, ring, index, onetime_handle)` | リンク可能なリング署名（実験的） |
| `lw_ring_verify(message_hex, ring, key_image_hex, sig_hex)` | `true` または `false` |
| `lw_pk_valid(public_hex)` | 有効な 2944 バイトのリング公開鍵か |
| `lw_mnemonic_from(entropy_hex)` | 32 バイトのエントロピーを 24 語の BIP-39 英語フレーズ（リスト）に |
| `lw_mnemonic_to(phrase)` | `Ok(entropy_hex)` または `Err(reason)` |
| `lw_pow_hash(header_hex)` | メモリハードなプルーフ・オブ・ワークのハッシュ（16 MiB、意図的に低速） |
| `lw_pow_trace(header_hex, every)` | 同じく、可視化用の探索経路の点も返す |
| `lw_meets_difficulty(hash_hex, difficulty)` | ハッシュが難易度の目標を満たすか |
| `lw_qr(text)` | QR コードを `1`（暗）と `0`（明）の行のリストで返す |

制限付きのアイソレートでは、秘密のハンドルを作ったり使ったりする組み込み関数に `"keys"`
ケイパビリティが必要です。メインスレッドはすべてのケイパビリティを持っています。

### 身元

身元はコンテキストラベルの下で署名します。検証する側も同じラベルを使う必要があります。

```ling
関数 取出(r) { 戻る 一致 r { ok(v) => v, bad(e) => "エラー:" + e } }

束縛 開始 = 実行 {
    束縛 身元 = 取出(lw_id_keypair("0101010101010101010101010101010101010101010101010101010101010101"))
    束縛 公開 = lw_id_public(身元)
    束縛 署名 = lw_id_sign(身元, "ctx", "68656c6c6f")
    印刷(lw_id_verify(公開, "ctx", "68656c6c6f", 署名))
    印刷(lw_id_verify(公開, "other", "68656c6c6f", 署名))
    印刷(lw_id_fingerprint(公開))
    印刷(lw_key_drop(身元))
}
```

```output
true
false
9ebc-56af-97f2-61af
true
```

### パスフレーズからの鍵

`lw_password_key` は `Ok(key_hex)` または `Err("invalid Argon2id parameters")` を返します。
ソルトは暗号文の隣に保存してください。秘密にする必要はありません。Ling Wallet のプロファイルは
`m_kib = 262144`（256 MiB）、`t = 3`、`p = 1` です。例はすぐに終わるよう極小のパラメータを
使っているので、真似しないでください。

```ling
関数 取出(r) { 戻る 一致 r { ok(v) => v, bad(e) => "エラー:" + e } }

束縛 開始 = 実行 {
    束縛 ソルト = "00112233445566778899aabbccddeeff"
    印刷(取出(lw_password_key("hunter2", ソルト, 1024, 1, 1)))
}
```

```output
a97cf9fed0c0703d0b60b7351990a06b086636d9fec2c5ef4d2b7a4bae25bb34
```

### リカバリーフレーズ（BIP-39）

`lw_mnemonic_from` は 32 バイトを 24 個の英単語にします。`lw_mnemonic_to` は空白で区切った
単語（大文字小文字は問いません）を受け取り、語数、単語、チェックサムを確認します。

```ling
関数 取出(r) { 戻る 一致 r { ok(v) => v, bad(e) => "エラー:" + e } }

束縛 開始 = 実行 {
    束縛 エントロピー = "1111111111111111111111111111111111111111111111111111111111111111"
    束縛 単語 = lw_mnemonic_from(エントロピー)
    印刷(長さ(単語))
    印刷(単語[0] + " " + 単語[1] + " " + 単語[23])
    印刷(取出(lw_mnemonic_to(join(単語, " "))) == エントロピー)
    印刷(取出(lw_mnemonic_to("abandon abandon")))
}
```

```output
24
baby mass cake
true
エラー:a recovery phrase has exactly 24 words
```

### リング署名とプルーフ・オブ・ワーク

LRS-87 リング署名（`lw_ring_sign`）はソースで実験的と明記されています。研究や試作に使い、
失うわけにはいかない資産には使わないでください。`lw_pow_hash` は 16 MiB の作業領域を確保し、
意図的に低速です。`lw_meets_difficulty` は難易度を数値で、または 2^53 を超える値なら 10 進の
文字列で受け取ります。

## 関連項目

- [ネットワーク](../reference/networking.md): `lingtp://` と `p2p_*` はこれらのプリミティブを使います。
- [Web サーバーと HTTP](../reference/web.md): セッション、ログイン、HTTPS。
