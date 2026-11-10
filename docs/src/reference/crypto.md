# Cryptography

Ling has cryptography built in: hashes, authenticated encryption, password hashing,
classical and post-quantum signatures, a hybrid post-quantum key exchange, two-factor
codes, and the Ling Wallet primitives (`lw_*`) with BIP-39 recovery phrases. The builtins
are a thin, string-based layer over vetted Rust implementations (RustCrypto, dalek,
BLAKE3, `ml-kem`, `ml-dsa`, Argon2) in the `ling-crypto` crate.

Which install profile you need:

| Builtins | Profile |
|---|---|
| Hashes, `crypto_seal`/`crypto_open`, signatures, `knot_*`, `password_*` | `web` or `privacy` (both carry the crypto building block) |
| `totp_secret`, `totp_now` | `web` |
| `lw_*` | `privacy` |

The default install (`full`) includes all of them. If a profile is missing, calling one
of its builtins fails with a message that names the profile to reinstall with.

## Conventions

- **Hex in, hex out.** Keys, nonces, ciphertexts, digests and signatures are lowercase hex
  strings. A 32-byte key is 64 hex characters.
- **Handles.** `ed25519_keygen`, `mldsa_keygen`, `mldsa87_keygen` and `knot_keygen` return a
  small number that points into a key table inside the interpreter. The `lw_*` family
  returns a 32-character hex handle instead. Secret key bytes never reach your code.
- **Results.** Builtins that can fail on bad input return a result. Printed, it looks like
  `Ok(...)` or `Err(message)`. Unwrap it with `match r { ok(v) => ..., bad(e) => ... }`.
- **Misspelt names are silent.** An unknown function evaluates to `()`. If a call "does
  nothing", check the spelling.

## Security guidance

1. **Use the highest-level builtin that does the job.** To encrypt with a key you hold, use
   `crypto_seal`. To store passwords, use `password_hash`. Don't build your own scheme from
   hashes and XOR.
2. **Keys must be 32 bytes of real entropy.** `crypto_seal`, `crypto_open` and the
   `*_from_seed` builtins read hex leniently: a short key is zero-padded, a long one is
   cut, and invalid hex pairs are dropped. A typo gives you a weak key, not an error.
   Make keys with `random_hex(32)`. Turn a passphrase into a key with `lw_password_key`
   (Argon2id), never with a single hash.
3. **`password_hash` is for logins, `lw_password_key` is for keys.**
4. **Never reuse a nonce.** `crypto_seal` picks a fresh random nonce every call. `lw_aead_seal`
   takes the nonce from you: use `random_hex(24)` for each message.
5. **Don't use the game RNG for secrets.** `rand` is for gameplay. Use `random_hex` or
   `lw_random`, which read the operating system's secure generator.
6. **Strings are not wiped.** A hex secret in a Ling variable lives as long as that string.
   Keep secrets in handles where the API offers them, and drop `lw_*` keys with `lw_key_drop`.
7. **Compare with the verify builtins.** `==` on strings is not constant time. Use
   `ed25519_verify`, `mldsa87_verify`, `password_verify` and `totp_verify`.
8. **`crypto_hash` is not plain SHA3-256.** It adds a Ling-specific label, so it won't match
   other tools. Use `sha256_hex` or `hash_hex` when you need to interoperate.
9. **Treat the Ling-specific designs as unaudited.** The geometric layer (`knot_points`,
   `holo_points`, `crypto_hash`), the DICE-42 cascade inside `lingtp://` and the LRS-87 ring
   signature are original constructions over standard primitives. LRS-87 is marked
   experimental. For high-value systems, use the standard primitives and get your own review.
10. **Post-quantum here is hybrid.** The KEM builtins combine X25519 with ML-KEM, so the
    secret stays safe if either half breaks. A tampered KEM ciphertext gives a different
    secret instead of an error, so always use the secret to authenticate something.

## Hashes and random bytes

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `sha256_hex(text)` | `SHA256哈希` | — | — | — | SHA-256 as 64 hex characters |
| `hash_hex(text)` | — | — | — | `แฮชสตริง` | BLAKE3 as 64 hex characters |
| `file_hash(path)` | — | — | — | `แฮชไฟล์` | BLAKE3 of a file's raw bytes |
| `crypto_hash(text)` | `几何哈希` | `幾何ハッシュ` | `기하해시` | `แฮชเข้ารหัส` | Ling's labelled SHA3-256 |
| `shake_hex(text, len)` | `SHAKE哈希` | — | — | — | SHAKE-256, `len` bytes of output |
| `hash_str(text)` | — | — | — | `แฮช` | Fast non-cryptographic hash, 0 to 1 |
| `hash_int(text, n)` | — | — | — | `แฮชจำนวน` | Fast non-cryptographic hash, 0 to `n - 1` |
| `random_hex(n)` | — | — | `무작위16진수` | — | `n` secure random bytes as hex |

### `sha256_hex`, `hash_hex`, `file_hash`

`sha256_hex` matches every other SHA-256 tool, including the browser's
`crypto.subtle.digest`. `hash_hex` is BLAKE3: fast and collision resistant. `file_hash`
hashes a file's raw bytes with BLAKE3 (binary safe) and returns `""` if the file can't be
read. None of them is a password hash.

```ling
bind start = do {
    print(sha256_hex("abc"))
    print(hash_hex("abc"))
    write_file("note.txt", "hello file")
    print(file_hash("note.txt") == hash_hex("hello file"))
    remove_file("note.txt")
}
```

```output
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85
true
```

### `crypto_hash`, `shake_hex`

`crypto_hash(text)` computes `SHA3-256("ling-geo-holo-hash-v1" + text)`. It is the digest
that `knot_points` draws. `shake_hex(text, len)` squeezes `len` bytes (default 32) from
SHAKE-256.

```ling
bind start = do {
    print(crypto_hash("abc"))
    print(shake_hex("abc", 16))
}
```

```output
0f5ae6a06fa73d3e18615b33ee9a9efafdb1a25a7bbf5f3ca9c4d7f1ac4ddfb9
483366601360a8771c6863080cc4114d
```

### `hash_str`, `hash_int`

FNV-1a hashes for seeds, colours and bucketing. They are deterministic and **not secure**:
never use them on anything an attacker controls. `hash_int`'s `n` defaults to 100.

### `random_hex(n)`

Returns `n` bytes (default 32, clamped to 1–256) from the operating system's secure random
generator, as `2 * n` hex characters. Use it for keys, nonces and session ids.

```ling
bind start = do {
    bind key = random_hex(32)
    print(len(key))
}
```

```output
64
```

## Sealing messages

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `crypto_seal(key_hex, text)` | `封印` | `封印する` | `봉인` | `ผนึก` | Encrypt and authenticate with XChaCha20-Poly1305 |
| `crypto_open(key_hex, sealed_hex)` | `解封` | `封印解除` | `봉인해제` | `เปิดผนึก` | Verify and decrypt |

`crypto_seal` returns `nonce (24 bytes) + ciphertext + tag (16 bytes)` as hex. Sealing the same
text twice gives different output because each call draws a new nonce. `crypto_open` returns
the text, or `Err("decryption/auth failed")` if the key is wrong or one bit changed. The
plaintext is decoded as UTF-8, so encode binary data first (for example with `base64_encode`).

```ling
bind start = do {
    bind key = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
    bind sealed = crypto_seal(key, "meet at noon")
    print(crypto_open(key, sealed))
    print(crypto_open("ff" + key, sealed))
}
```

```output
meet at noon
Err(decryption/auth failed)
```

## Passwords

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `password_hash(password)` | — | — | `비밀번호해시` | — | Argon2id hash as a PHC string |
| `password_verify(password, stored)` | — | — | `비밀번호확인` | — | Check a password against a stored hash |

`password_hash` uses Argon2id with 64 MiB of memory, 3 passes, 4 lanes and a fresh random
salt, and returns a string like `$argon2id$v=19$m=65536,t=3,p=4$...`. Store the whole string.
Each call takes tens of milliseconds by design. `password_verify` reads the parameters from
the stored string and returns `true` or `false` (a malformed string gives `false`).

```ling
bind start = do {
    bind stored = password_hash("correct horse")
    print(substr(stored, 0, 30))
    print(password_verify("correct horse", stored))
    print(password_verify("wrong", stored))
}
```

```output
$argon2id$v=19$m=65536,t=3,p=4
true
false
```

## Signatures

Three families share one shape: make a keypair (random or from a 32-byte seed), read the
public key, sign a string, verify with the public key.

| Family | Prefix | Public key | Signature | Notes |
|---|---|---|---|---|
| Ed25519 | `ed25519_` | 32 bytes | 64 bytes | Classical, fast, deterministic |
| ML-DSA-65 (FIPS 204) | `mldsa_` | 1952 bytes | 3309 bytes | Post-quantum, NIST category 3 |
| ML-DSA-87 (FIPS 204) | `mldsa87_` | 2592 bytes | 4627 bytes | Post-quantum, NIST category 5; used by `lingtp://` |

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `ed25519_keygen()` | `生成签名密钥` | `署名鍵生成` | `생성서명키` | `สร้างกุญแจลายเซ็น` | Random keypair handle |
| `ed25519_keygen_from_seed(seed_hex)` | `从种子生成签名密钥` | `シードから署名鍵生成` | `씨앗에서생성서명키` | `สร้างกุญแจลายเซ็นจากเมล็ด` | Keypair from a 32-byte seed |
| `ed25519_public(handle)` | `签名公钥` | `署名公開鍵` | `서명공개키` | `กุญแจสาธารณะลายเซ็น` | Public key as hex |
| `ed25519_sign(handle, message)` | `签名` | `署名する` | `서명하다` | `เซ็นชื่อ` | Signature as hex |
| `ed25519_verify(public_hex, message, sig_hex)` | `验证签名` | `署名検証` | `서명확인` | `ยืนยันลายเซ็น` | `true` if valid |
| `mldsa_keygen()` | `生成量子签名密钥` | `量子署名鍵生成` | `양자서명키생성` | `สร้างกุญแจลายเซ็นควอนตัม` | Random ML-DSA-65 keypair |
| `mldsa_keygen_from_seed(seed_hex)` | `从种子生成量子签名密钥` | `シードから量子署名鍵生成` | `씨앗에서양자서명키생성` | `สร้างกุญแจลายเซ็นควอนตัมจากเมล็ด` | ML-DSA-65 keypair from a seed |
| `mldsa_public(handle)` | `量子签名公钥` | `量子署名公開鍵` | `양자서명공개키` | `กุญแจสาธารณะลายเซ็นควอนตัม` | Public key as hex |
| `mldsa_sign(handle, message)` | `量子签名` | `量子署名する` | `양자서명하다` | `เซ็นชื่อควอนตัม` | Signature as hex |
| `mldsa_verify(public_hex, message, sig_hex)` | `验证量子签名` | `量子署名検証` | `양자서명확인` | `ยืนยันลายเซ็นควอนตัม` | `true` if valid |
| `mldsa87_keygen()` | — | — | — | — | Random ML-DSA-87 keypair |
| `mldsa87_keygen_from_seed(seed_hex)` | — | — | — | — | ML-DSA-87 keypair from a seed |
| `mldsa87_public(handle)` | — | — | — | — | Public key as hex |
| `mldsa87_sign(handle, message)` | — | — | — | — | Signature as hex |
| `mldsa87_verify(public_hex, message, sig_hex)` | — | — | — | — | `true` if valid |

### Using signatures

A seed lets you store only 64 hex characters and re-create the same keypair after a restart;
protect it like a private key. An unknown handle makes `*_public` and `*_sign` return `""`.
The verify builtins return `false` for a bad signature or any malformed input. Ed25519
signatures are deterministic: the same key and message always give the same signature.

For long-lived artifacts such as licence files or release manifests, sign with Ed25519
**and** ML-DSA-87, and accept the artifact only if both verify:

```ling
bind start = do {
    bind seed = "0101010101010101010101010101010101010101010101010101010101010101"
    bind ed = ed25519_keygen_from_seed(seed)
    bind pq = mldsa87_keygen_from_seed(seed)
    bind doc = "licence: acme, expires 2031-01-01"
    bind sig_a = ed25519_sign(ed, doc)
    bind sig_b = mldsa87_sign(pq, doc)
    print(len(ed25519_public(ed)))
    print(ed25519_verify(ed25519_public(ed), doc, sig_a) && mldsa87_verify(mldsa87_public(pq), doc, sig_b))
    print(mldsa87_verify(mldsa87_public(pq), doc + "!", sig_b))
}
```

```output
64
true
false
```

## Post-quantum key exchange

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `knot_keygen()` | `生成密钥` | `鍵生成` | `키생성` | `สร้างกุญแจปม` | Hybrid X25519 + ML-KEM-768 keypair |
| `knot_public(handle)` | `公钥` | `公開鍵` | `공개키` | `กุญแจสาธารณะปม` | Public key (1216 bytes) as hex |
| `knot_encapsulate(public_hex)` | `封装密钥` | `カプセル化` | `캡슐화` | `ห่อกุญแจปม` | `[ciphertext_hex, secret_hex]` |
| `knot_decapsulate(handle, ciphertext_hex)` | `解封装密钥` | `カプセル解除` | `캡슐해제` | `แกะกุญแจปม` | The same 32-byte secret |

`hybrid_keygen`, `hybrid_public`, `hybrid_encapsulate` and `hybrid_decapsulate` are English
synonyms. The flow: the receiver makes a keypair and publishes the public key; the sender
encapsulates to it, keeps the secret and sends the ciphertext; the receiver decapsulates and
gets the same secret. Use the secret as a `crypto_seal` key.

`knot_keygen` keys can't be seeded or exported: they live only as long as the process. For a
persistent post-quantum KEM identity, use `lw_kem_keypair`.

```ling
bind start = do {
    bind bob = knot_keygen()
    bind enc = knot_encapsulate(knot_public(bob))
    bind secret_bob = knot_decapsulate(bob, enc[0])
    print(enc[1] == secret_bob)
    print(crypto_open(secret_bob, crypto_seal(enc[1], "hello bob")))
}
```

```output
true
hello bob
```

## Visual fingerprints

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `knot_points(text)` | `结点坐标` | `結び目点` | `매듭점` | `จุดปม` | 256 3-D points of a torus-knot fingerprint |
| `knot_label(text)` | `结点标签` | `結び目ラベル` | `매듭라벨` | `ป้ายปม` | Short name of that knot |
| `holo_points(text)` | `全息点` | `ホログラム点` | `홀로그램점` | `จุดโฮโลแกรม` | 4-D points of a holographic split (decorative) |
| `holo_fragment_count(text)` | `全息碎片数` | `ホログラム断片数` | `홀로그램조각수` | `จำนวนชิ้นโฮโลแกรม` | Number of fragments for a string |

These turn a digest into something a person can compare by eye: draw consecutive triples
from `knot_points` with `draw_line_3d`. They add no cryptographic strength of their own.

```ling
bind start = do {
    print(len(knot_points("abc")))
    print(knot_label("abc"))
}
```

```output
768
knot-17_12-v5.2
```

## One-time codes (TOTP)

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `totp_secret()` | — | — | — | `โทเทนลับ` | New random Base32 secret (160 bits) |
| `totp_uri(secret, account, issuer)` | — | — | — | `โทเทนยูอาร์ไอ` | `otpauth://` link for authenticator apps |
| `totp_verify(secret, code)` | — | — | — | `โทเทนตรวจ` | Check a six-digit code |
| `totp_now(secret)` | — | — | — | `โทเทนตอนนี้` | The code valid right now |

TOTP here is RFC 6238 with SHA-1, six digits and 30-second steps, which matches Google
Authenticator and Authy. `totp_verify` accepts one step either side and doesn't track replay:
store the last accepted code's time step yourself. `totp_uri` doesn't URL-encode, so keep `&`
and spaces out of the account and issuer. Show the URI as a QR code with `lw_qr`.

```ling
bind start = do {
    print(totp_uri("JBSWY3DPEHPK3PXP", "alice@example.com", "Acme"))
    bind secret = totp_secret()
    print(len(secret))
    print(totp_verify(secret, totp_now(secret)))
}
```

```output
otpauth://totp/Acme:alice@example.com?secret=JBSWY3DPEHPK3PXP&issuer=Acme&algorithm=SHA1&digits=6&period=30
32
true
```

## Ling Wallet primitives: `lw_*`

The `lw_*` family is the primitive set of the Ling Wallet project. It differs from the
builtins above in three ways:

- **Strict input.** Hex must be well formed and exactly the documented length. Bad input
  returns `Err(reason)` instead of being repaired.
- **Secret handles.** Builtins that create secrets return `Ok(handle)`, a 32-character hex
  handle. `lw_key_drop` destroys the key and wipes its memory.
- **Verifiers never fail loudly.** `lw_id_verify`, `lw_ring_verify` and `lw_pk_valid` return
  `false` for any malformed input.

None of them has a localized alias.

| Builtin | Purpose |
|---|---|
| `lw_h256(domain, parts)` | Domain-separated SHAKE-256, 32-byte output; `parts` is a list of hex strings |
| `lw_xof(domain, parts, len)` | The same with `len` bytes of output |
| `lw_random(n)` | `n` secure random bytes (up to 1 MiB) as hex |
| `lw_aead_seal(key, nonce, aad, plaintext)` | XChaCha20-Poly1305 with your 24-byte nonce and associated data (all hex) |
| `lw_aead_open(key, nonce, aad, ciphertext)` | `Ok(plaintext_hex)` or `Err("authentication failed")` |
| `lw_password_key(password, salt, m_kib, t, p)` | Argon2id: a 32-byte key from a passphrase and a 16-byte salt |
| `lw_kem_keypair(seed)` | X25519 + ML-KEM-1024 key from a 96-byte seed |
| `lw_kem_public(handle)` | KEM public key (1600 bytes) |
| `lw_kem_encaps(public_hex, context_hex)` | `Ok([ciphertext_hex, secret_hex])`, bound to a context |
| `lw_kem_decaps(handle, ciphertext_hex, context_hex)` | `Ok(secret_hex)`; a different context gives a different secret |
| `lw_id_keypair(seed)` | ML-DSA-87 identity from a 32-byte seed |
| `lw_id_public(handle)` | Identity public key (2592 bytes) |
| `lw_id_sign(handle, context, message_hex)` | Signature under a context label such as `"tx.v1"` |
| `lw_id_verify(public_hex, context, message_hex, sig_hex)` | `true` or `false` |
| `lw_id_fingerprint(public_hex)` | Short fingerprint like `9ebc-56af-97f2-61af` |
| `lw_key_drop(handle)` | Destroy a secret handle; `true` if it existed |
| `lw_spend_keypair`, `lw_spend_public`, `lw_onetime_secret`, `lw_onetime_public`, `lw_onetime_public_of`, `lw_onetime_key_image` | Keys for LRS-87 ring signatures |
| `lw_ring_sign(message_hex, ring, index, onetime_handle)` | Linkable ring signature (experimental) |
| `lw_ring_verify(message_hex, ring, key_image_hex, sig_hex)` | `true` or `false` |
| `lw_pk_valid(public_hex)` | Is this a valid 2944-byte ring public key? |
| `lw_mnemonic_from(entropy_hex)` | 32 bytes of entropy as a 24-word BIP-39 English phrase (a list) |
| `lw_mnemonic_to(phrase)` | `Ok(entropy_hex)` or `Err(reason)` |
| `lw_pow_hash(header_hex)` | Memory-hard proof-of-work hash (16 MiB, slow by design) |
| `lw_pow_trace(header_hex, every)` | The same, plus points of the walk for visualisation |
| `lw_meets_difficulty(hash_hex, difficulty)` | Does a hash meet a difficulty target? |
| `lw_qr(text)` | QR code as a list of rows of `1` (dark) and `0` (light) |

In a restricted isolate, builtins that create or use secret handles need the `"keys"`
capability. The main thread has every capability.

### Identities

An identity signs under a context label; the verifier must use the same label.

```ling
fn u(r) { return match r { ok(v) => v, bad(e) => "ERR:" + e } }

bind start = do {
    bind id = u(lw_id_keypair("0101010101010101010101010101010101010101010101010101010101010101"))
    bind pk = lw_id_public(id)
    bind sig = lw_id_sign(id, "ctx", "68656c6c6f")
    print(lw_id_verify(pk, "ctx", "68656c6c6f", sig))
    print(lw_id_verify(pk, "other", "68656c6c6f", sig))
    print(lw_id_fingerprint(pk))
    print(lw_key_drop(id))
}
```

```output
true
false
9ebc-56af-97f2-61af
true
```

### Keys from passphrases

`lw_password_key` returns `Ok(key_hex)` or `Err("invalid Argon2id parameters")`. Store the salt
next to the ciphertext; it doesn't need to be secret. The Ling Wallet profile is
`m_kib = 262144` (256 MiB), `t = 3`, `p = 1`. The example uses tiny parameters so it runs
instantly; don't copy them.

```ling
fn u(r) { return match r { ok(v) => v, bad(e) => "ERR:" + e } }

bind start = do {
    bind salt = "00112233445566778899aabbccddeeff"
    print(u(lw_password_key("hunter2", salt, 1024, 1, 1)))
}
```

```output
a97cf9fed0c0703d0b60b7351990a06b086636d9fec2c5ef4d2b7a4bae25bb34
```

### Recovery phrases (BIP-39)

`lw_mnemonic_from` turns 32 bytes into 24 English words. `lw_mnemonic_to` takes the words
separated by spaces (any case) and checks the word count, the words and the checksum.

```ling
fn u(r) { return match r { ok(v) => v, bad(e) => "ERR:" + e } }

bind start = do {
    bind entropy = "1111111111111111111111111111111111111111111111111111111111111111"
    bind words = lw_mnemonic_from(entropy)
    print(len(words))
    print(words[0] + " " + words[1] + " " + words[23])
    print(u(lw_mnemonic_to(join(words, " "))) == entropy)
    print(u(lw_mnemonic_to("abandon abandon")))
}
```

```output
24
baby mass cake
true
ERR:a recovery phrase has exactly 24 words
```

### Ring signatures and proof of work

LRS-87 ring signatures (`lw_ring_sign`) are marked experimental in the source. Use them for
research and prototypes, not for assets you can't afford to lose. `lw_pow_hash` allocates a
16 MiB scratchpad and is slow on purpose. `lw_meets_difficulty` accepts the difficulty as a
number, or as a decimal string for values above 2^53.

## See also

- [Networking](../reference/networking.md): `lingtp://` and `p2p_*` use these primitives.
- [Web Server and HTTP](../reference/web.md): sessions, logins and HTTPS.
