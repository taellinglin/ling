# Cryptography & Security

Ling provides integrated access to modern cryptography via the `ling-crypto` crate, with abstractions available in English, Chinese, Japanese, Korean, and Thai.

## Overview

All crypto functions are multilingual. Use them in any supported language:

```ling
bind hash = blake3_hash("data")           # English
令 hash = blake3_哈希("data")              # Chinese
bind hash = blake3_ハッシュ("data")        # Japanese
```

## Hashing

Cryptographic hash functions are **deterministic** and **one-way**: the same input always produces the same hash, but recovering the input from the hash is computationally infeasible.

### BLAKE3

```ling
bind hash_result = blake3_hash(data: string) -> string
```

**Multilingual names:**
- English: `blake3_hash`
- Chinese: `blake3_哈希`, `blake3_散列`
- Japanese: `blake3_ハッシュ`
- Korean: `blake3_해시`
- Thai: `blake3_แฮช`

Fast, cryptographic hash function. Output: 32 bytes (256-bit hexadecimal string).

**Example:**
```ling
bind data = "The quick brown fox"
bind digest = blake3_hash(data)
print(digest)  # → c3ab... (64-char hex)
```

**Use cases:**
- File integrity verification
- Blockchain transaction hashing
- Password salting (with Argon2id)
- Merkle trees

### SHA3-256 & SHA3-512

```ling
bind hash_256 = sha3_256_hash(data: string) -> string
bind hash_512 = sha3_512_hash(data: string) -> string
```

NIST-standardized SHA-3 family. Slightly slower than BLAKE3 but widely supported.

- **SHA3-256**: 32-byte (256-bit) output
- **SHA3-512**: 64-byte (512-bit) output

### SHAKE-256 (Extendable-Output)

```ling
bind output = shake256_xof(data: string, length: number) -> string
```

**Multilingual names:**
- English: `shake256_xof`
- Chinese: `shake256_可扩展输出`
- Japanese: `shake256_拡張出力`
- Korean: `shake256_확장출력`
- Thai: `shake256_ส่วนขยายผลลัพธ์`

Produces variable-length output (XOF = Extendable-Output Function). Useful when you need arbitrary-length hash outputs.

**Example:**
```ling
bind stream = shake256_xof("seed", 256)  # 256 bytes of pseudorandom output
```

## Encryption (AEAD)

Authenticated Encryption with Associated Data (AEAD) combines encryption with built-in authentication. If the ciphertext or key is wrong, decryption **fails and returns error**.

### AES-256-GCM

```ling
bind encrypted = aes_gcm_encrypt(plaintext: string, key: string) -> string
bind plaintext = aes_gcm_decrypt(ciphertext_with_nonce: string, key: string) -> string
```

**Multilingual names:**
- English: `aes_gcm_encrypt`, `aes_gcm_decrypt`
- Chinese: `aes_gcm_加密`, `aes_gcm_解密`
- Japanese: `aes_gcm_暗号化`, `aes_gcm_復号化`
- Korean: `aes_gcm_암호화`, `aes_gcm_복호화`
- Thai: `aes_gcm_เข้ารหัส`, `aes_gcm_ถอดรหัส`

Industry-standard AEAD cipher. Fast and secure. Key must be 32 bytes (256-bit).

**Example:**
```ling
bind key = blake3_hash("my password")  # 32 bytes
bind plaintext = "Secret message"
bind encrypted = aes_gcm_encrypt(plaintext, key)
bind recovered = aes_gcm_decrypt(encrypted, key)
# → "Secret message"
```

**Return format:**
- `encrypt()` returns: `nonce(12 bytes) + ciphertext + tag(16 bytes)`
- `decrypt()` recovers plaintext if tag matches, else error

### XChaCha20-Poly1305

```ling
bind encrypted = chacha20_encrypt(plaintext: string, key: string) -> string
bind plaintext = chacha20_decrypt(ciphertext_with_nonce: string, key: string) -> string
```

Modern, ChaCha20 stream cipher with Poly1305 authentication. Better for high-speed scenarios and long nonces.

**Use cases:**
- Disk encryption
- TLS connections
- Streaming data protection

## Digital Signatures (Ed25519)

Asymmetric cryptography: sign with your **secret key**, verify with your **public key**. Proves you created a message without revealing your secret.

### Ed25519 Signing

```ling
bind signature = ed25519_sign(message: string, secret_key: string) -> string
bind is_valid = ed25519_verify(message: string, public_key: string, signature: string) -> bool
bind public_key = ed25519_public_key(secret_key: string) -> string
```

**Multilingual names:**
- English: `ed25519_sign`, `ed25519_verify`, `ed25519_public_key`
- Chinese: `ed25519_签名`, `ed25519_验证`, `ed25519_公钥`
- Japanese: `ed25519_署名`, `ed25519_検証`, `ed25519_公開鍵`
- Korean: `ed25519_서명`, `ed25519_검증`, `ed25519_공개키`
- Thai: `ed25519_ลายเซ็น`, `ed25519_ยืนยัน`, `ed25519_คีย์สาธารณะ`

**Example:**
```ling
bind secret = blake3_hash("my secret")  # 32 bytes
bind public = ed25519_public_key(secret)

bind message = "I approve this transaction"
bind sig = ed25519_sign(message, secret)

# Anyone can verify with public key
若 ed25519_verify(message, public, sig) {
    print("Signature valid! Message is from the secret key holder.")
} 否则 {
    print("Forged or corrupted signature.")
}
```

**Use cases:**
- Code signing (release verification)
- Transaction signing (blockchain, payments)
- Message authentication (prove authorship)

## Key Derivation

Convert passwords or input key material into cryptographic keys.

### Argon2id (Password Hashing)

```ling
bind hash_string = argon2id_hash(password: string) -> string
bind is_match = argon2id_verify(password: string, hash_string: string) -> bool
```

**Multilingual names:**
- English: `argon2id_hash`, `argon2id_verify`
- Chinese: `argon2id_密码哈希`, `argon2id_密码验证`
- Japanese: `argon2id_パスワードハッシュ`, `argon2id_パスワード検証`
- Korean: `argon2id_암호해시`, `argon2id_암호검증`
- Thai: `argon2id_แฮชรหัสผ่าน`, `argon2id_ตรวจสอบรหัสผ่าน`

**Memory-hard** password hashing, resistant to GPU cracking. Salt is automatic (random, embedded in output).

**Example:**
```ling
bind password = "correct horse battery staple"
bind hashed = argon2id_hash(password)  # → "$argon2id$v=19$...", takes ~100ms

# Later, verify a login attempt
若 argon2id_verify(user_input, hashed) {
    print("Login successful")
} 否则 {
    print("Invalid password")
}
```

### HKDF-SHA3 (Key Derivation)

```ling
bind derived_key = hkdf_sha3(
    input_key_material: string,
    salt: string,
    info: string,
    output_len: number
) -> string
```

Extract-Expand key derivation. Derives `output_len` bytes from input key material.

**Example:**
```ling
bind ikm = "high-entropy random bytes"
bind salt = "public, non-secret salt"
bind context = "application-name-v1"
bind key_64 = hkdf_sha3(ikm, salt, context, 64)  # 64 bytes
```

## Post-Quantum: ML-KEM-768

NIST-standardized lattice-based key encapsulation, resistant to quantum computers.

```ling
bind (secret_key, public_key) = mlkem_keygen() -> (string, string)
bind (ciphertext, shared_secret) = mlkem_encapsulate(public_key: string) -> (string, string)
bind shared_secret = mlkem_decapsulate(secret_key: string, ciphertext: string) -> string
```

**Multilingual names:**
- English: `mlkem_keygen`, `mlkem_encapsulate`, `mlkem_decapsulate`
- Chinese: `mlkem_密钥生成`, `mlkem_封装`, `mlkem_解封装`
- Japanese: `mlkem_鍵生成`, `mlkem_カプセル化`, `mlkem_カプセル化解除`
- Korean: `mlkem_키생성`, `mlkem_캡슐화`, `mlkem_캡슐화해제`
- Thai: `mlkem_สร้างคีย์`, `mlkem_บรรจุ`, `mlkem_ถอดบรรจุ`

**Key sizes:**
- Secret key: 2400 bytes
- Public key: 1184 bytes
- Ciphertext: 1088 bytes
- Shared secret: 32 bytes

**Example:**
```ling
bind (sk, pk) = mlkem_keygen()

# Alice encapsulates to Bob's public key
bind (ct, ss_alice) = mlkem_encapsulate(pk)

# Bob decapsulates the ciphertext
bind ss_bob = mlkem_decapsulate(sk, ct)

# Both now share the same 32-byte secret
print(ss_alice == ss_bob)  # → true
```

## Advanced: Zero-Knowledge & VRF

### Schnorr Proof of Knowledge

```ling
bind proof = schnorr_proof_of_knowledge(secret: string, message: string) -> string
```

**Multilingual names:**
- English: `schnorr_proof_of_knowledge`
- Chinese: `schnorr_零知识证明`
- Japanese: `schnorr_ゼロ知識証明`
- Korean: `schnorr_영지식증명`
- Thai: `schnorr_พิสูจน์ความรู้ศูนย์`

Prove you know a secret without revealing it. Used in authentication protocols.

### Verifiable Random Function (VRF)

```ling
bind (proof, output) = vrf_evaluate(secret_key: string, input: string) -> (string, string)
```

**Multilingual names:**
- English: `vrf_evaluate`
- Chinese: `vrf_可验证随机函数`
- Japanese: `vrf_検証可能ランダム関数`
- Korean: `vrf_검증가능난수함수`
- Thai: `vrf_ฟังก์ชั่นสุ่มที่ตรวจสอบได้`

Produces pseudorandom output that is **deterministic** (same input → same output) but **verifiable** (anyone can confirm the output is correct without knowing the secret).

**Use case:** Fair lottery systems, randomized load balancing.

## Shamir Secret Sharing

```ling
bind shares = shamir_split(secret: string, threshold: number, num_shares: number) -> list
bind secret = shamir_reconstruct(shares: list) -> string
```

Split a secret into `num_shares` shares; any `threshold` shares can recover the secret.

**Example:**
```ling
bind secret = "nuclear codes"
bind shares = shamir_split(secret, 3, 5)  # 5 shares, need 3 to recover

# Any 3 shares can recover the secret
bind recovered = shamir_reconstruct(take(shares, 3))
print(recovered == secret)  # → true
```

## Mandala Hash (Ling-specific)

Visual geometric patterns → cryptographic keys.

```ling
bind key = mandala_hash(
    n_rings: number,     # 1-255
    n_spokes: number,    # 1-255
    n_petals: number,    # 1-255
    twist: number,       # spiral twist factor
    seed: number         # entropy seed
) -> string
```

**Multilingual names:**
- English: `mandala_hash`
- Chinese: `mandala_哈希`
- Japanese: `mandala_ハッシュ`
- Korean: `mandala_해시`
- Thai: `mandala_แฮช`

**Property:** Same visual pattern → same key (deterministic). Different patterns → completely different keys (avalanche).

**Use case:** Ling-specific: derive cryptographic identities from visual geometric patterns (see `crypto_hologram.ling`).

## Security Best Practices

1. **Never reuse nonces** with the same key in AEAD encryption.
2. **Use salt** with password hashes (Argon2id handles this automatically).
3. **Verify signatures** before trusting data.
4. **Rotate keys** periodically.
5. **Keep secrets secret** — Ed25519 secret keys and ECDH ephemeral keys must be protected.
6. **Use AEAD** instead of unauthenticated encryption (prevents tampering).

## Multilingual Crypto Library

All examples in this section support all 5 languages. See `examples/crypto/crypto.ling` for a complete multilingual wrapper library.

```ling
# Same code, different languages:
bind h1 = blake3_hash("test")           # English
令 h2 = blake3_哈希("test")             # Chinese (Chinese)
bind h3 = blake3_ハッシュ("test")       # Japanese
```

## Further Reading

- **Visualization**: `examples/crypto/crypto_hologram.ling` — 4D holographic cryptography
- **Crypto Crate**: [ling-crypto on crates.io](https://crates.io/crates/ling-crypto)
- **Standards**:
  - BLAKE3: https://blake3.io/
  - SHA3: NIST FIPS 202
  - AES-GCM: NIST SP 800-38D
  - Ed25519: RFC 8032
  - Argon2id: RFC 9106
  - ML-KEM: FIPS 203 (NIST PQC)
