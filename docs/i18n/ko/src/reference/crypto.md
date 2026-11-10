# 암호화

Ling에는 암호 기능이 내장되어 있습니다. 해시, 인증 암호화, 비밀번호 해시, 고전 서명과
양자 내성 서명, 하이브리드 양자 내성 키 교환, 2단계 인증 코드, 그리고 BIP-39 복구 문구를
지원하는 Ling Wallet 기본 연산(`lw_*`)이 있습니다. 이 내장 함수들은 `ling-crypto` 크레이트에
들어 있는 검증된 Rust 구현(RustCrypto, dalek, BLAKE3, `ml-kem`, `ml-dsa`, Argon2) 위에
문자열 기반의 얇은 층을 얹은 것입니다.

필요한 설치 프로필은 다음과 같습니다.

| 내장 함수 | 프로필 |
|---|---|
| 해시, `crypto_seal`/`crypto_open`, 서명, `knot_*`, `password_*` | `web` 또는 `privacy`(둘 다 암호 구성 요소를 포함) |
| `totp_secret`, `totp_now` | `web` |
| `lw_*` | `privacy` |

기본 설치(`full`)에는 모두 포함되어 있습니다. 프로필이 빠져 있으면 해당 내장 함수를 호출할 때
어떤 프로필로 다시 설치해야 하는지 알려 주는 메시지와 함께 실패합니다.

## 규칙

- **입력도 출력도 16진수입니다.** 키, 논스, 암호문, 다이제스트, 서명은 소문자 16진수
  문자열입니다. 32바이트 키는 16진수 64자입니다.
- **핸들.** `ed25519_keygen`, `mldsa_keygen`, `mldsa87_keygen`, `knot_keygen`은 인터프리터 안의
  키 테이블을 가리키는 작은 숫자를 반환합니다. `lw_*` 계열은 대신 32자 16진수 핸들을 반환합니다.
  비밀 키 바이트는 절대 여러분의 코드에 노출되지 않습니다.
- **결과.** 잘못된 입력으로 실패할 수 있는 내장 함수는 결과를 반환합니다. 출력하면
  `Ok(...)` 또는 `Err(message)`처럼 보입니다. `match r { ok(v) => ..., bad(e) => ... }`로 꺼내십시오.
- **철자가 틀린 이름은 조용히 넘어갑니다.** 알 수 없는 함수는 `()`로 평가됩니다. 호출이
  "아무 일도 하지 않는다면" 철자를 확인하십시오.

## 보안 지침

1. **목적에 맞는 가장 높은 수준의 내장 함수를 쓰십시오.** 가지고 있는 키로 암호화하려면
   `crypto_seal`을, 비밀번호를 저장하려면 `password_hash`를 쓰십시오. 해시와 XOR로 직접
   방식을 만들지 마십시오.
2. **키는 진짜 엔트로피 32바이트여야 합니다.** `crypto_seal`, `crypto_open`과
   `*_from_seed` 내장 함수는 16진수를 관대하게 읽습니다. 짧은 키는 0으로 채우고, 긴 키는 자르며,
   잘못된 16진수 쌍은 버립니다. 오타는 오류가 아니라 약한 키가 됩니다. 키는 `random_hex(32)`로
   만드십시오. 암호 문구를 키로 바꿀 때는 해시 한 번이 아니라 `lw_password_key`(Argon2id)를
   쓰십시오.
3. **`password_hash`는 로그인용, `lw_password_key`는 키용입니다.**
4. **논스를 재사용하지 마십시오.** `crypto_seal`은 호출할 때마다 새 무작위 논스를 고릅니다.
   `lw_aead_seal`은 논스를 여러분에게서 받으므로 메시지마다 `random_hex(24)`를 쓰십시오.
5. **비밀에 게임용 난수를 쓰지 마십시오.** `rand`는 게임 진행용입니다. 운영 체제의 안전한
   생성기를 읽는 `random_hex`나 `lw_random`을 쓰십시오.
6. **문자열은 지워지지 않습니다.** Ling 변수에 든 16진수 비밀은 그 문자열이 살아 있는 동안
   남아 있습니다. API가 핸들을 제공하면 비밀을 핸들에 두고, `lw_*` 키는 `lw_key_drop`으로
   폐기하십시오.
7. **비교는 검증 내장 함수로 하십시오.** 문자열 `==`는 상수 시간이 아닙니다.
   `ed25519_verify`, `mldsa87_verify`, `password_verify`, `totp_verify`를 쓰십시오.
8. **`crypto_hash`는 일반 SHA3-256이 아닙니다.** Ling 고유의 레이블을 덧붙이므로 다른 도구와
   일치하지 않습니다. 상호 운용이 필요하면 `sha256_hex`나 `hash_hex`를 쓰십시오.
9. **Ling 고유 설계는 감사받지 않은 것으로 취급하십시오.** 기하 계층(`knot_points`,
   `holo_points`, `crypto_hash`), `lingtp://` 안의 DICE-42 캐스케이드, LRS-87 링 서명은 표준
   기본 연산 위에 만든 독자적인 구성입니다. LRS-87은 실험적이라고 표시되어 있습니다. 가치가 큰
   시스템에서는 표준 기본 연산을 쓰고 별도의 검토를 받으십시오.
10. **여기서 양자 내성은 하이브리드입니다.** KEM 내장 함수는 X25519와 ML-KEM을 결합하므로
    한쪽이 깨져도 비밀은 안전합니다. 변조된 KEM 암호문은 오류 대신 다른 비밀을 내놓으므로,
    비밀은 항상 무언가를 인증하는 데 사용하십시오.

## 해시와 무작위 바이트

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `sha256_hex(text)` | `SHA256哈希` | — | — | — | SHA-256을 16진수 64자로 |
| `hash_hex(text)` | — | — | — | `แฮชสตริง` | BLAKE3를 16진수 64자로 |
| `file_hash(path)` | — | — | — | `แฮชไฟล์` | 파일 원시 바이트의 BLAKE3 |
| `crypto_hash(text)` | `几何哈希` | `幾何ハッシュ` | `기하해시` | `แฮชเข้ารหัส` | Ling의 레이블 붙은 SHA3-256 |
| `shake_hex(text, len)` | `SHAKE哈希` | — | — | — | SHAKE-256, 출력 `len`바이트 |
| `hash_str(text)` | — | — | — | `แฮช` | 빠른 비암호 해시, 0~1 |
| `hash_int(text, n)` | — | — | — | `แฮชจำนวน` | 빠른 비암호 해시, 0~`n - 1` |
| `random_hex(n)` | — | — | `무작위16진수` | — | 안전한 무작위 `n`바이트를 16진수로 |

### `sha256_hex`, `hash_hex`, `file_hash`

`sha256_hex`는 브라우저의 `crypto.subtle.digest`를 포함한 다른 모든 SHA-256 도구와 결과가
같습니다. `hash_hex`는 BLAKE3로, 빠르고 충돌에 강합니다. `file_hash`는 파일의 원시 바이트를
BLAKE3로 해시하며(바이너리 안전), 파일을 읽을 수 없으면 `""`를 반환합니다. 셋 다 비밀번호
해시가 아닙니다.

```ling
바인드 시작 = 실행 {
    출력(sha256_hex("abc"))
    출력(hash_hex("abc"))
    write_file("note.txt", "안녕 파일")
    출력(file_hash("note.txt") == hash_hex("안녕 파일"))
    remove_file("note.txt")
}
```

```output
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85
true
```

### `crypto_hash`, `shake_hex`

`crypto_hash(text)`는 `SHA3-256("ling-geo-holo-hash-v1" + text)`를 계산합니다. `knot_points`가
그리는 다이제스트가 바로 이것입니다. `shake_hex(text, len)`는 SHAKE-256에서 `len`바이트(기본값
32)를 뽑아냅니다.

```ling
바인드 시작 = 실행 {
    출력(기하해시("abc"))
    출력(shake_hex("abc", 16))
}
```

```output
0f5ae6a06fa73d3e18615b33ee9a9efafdb1a25a7bbf5f3ca9c4d7f1ac4ddfb9
483366601360a8771c6863080cc4114d
```

### `hash_str`, `hash_int`

시드, 색상, 버킷 나누기에 쓰는 FNV-1a 해시입니다. 결정적이며 **안전하지 않습니다**. 공격자가
조작할 수 있는 값에는 절대 쓰지 마십시오. `hash_int`의 `n` 기본값은 100입니다.

### `random_hex(n)`

운영 체제의 안전한 난수 생성기에서 `n`바이트(기본값 32, 1~256으로 제한)를 받아 16진수
`2 * n`자로 반환합니다. 키, 논스, 세션 ID에 쓰십시오.

```ling
바인드 시작 = 실행 {
    바인드 키 = random_hex(32)
    출력(길이(키))
}
```

```output
64
```

## 메시지 봉인

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `crypto_seal(key_hex, text)` | `封印` | `封印する` | `봉인` | `ผนึก` | XChaCha20-Poly1305로 암호화하고 인증 |
| `crypto_open(key_hex, sealed_hex)` | `解封` | `封印解除` | `봉인해제` | `เปิดผนึก` | 검증하고 복호화 |

`crypto_seal`은 `논스(24바이트) + 암호문 + 태그(16바이트)`를 16진수로 반환합니다. 호출마다 새
논스를 뽑으므로 같은 텍스트를 두 번 봉인해도 결과가 다릅니다. `crypto_open`은 텍스트를
반환하거나, 키가 틀렸거나 한 비트라도 바뀌었으면 `Err("decryption/auth failed")`를 반환합니다.
평문은 UTF-8로 디코딩되므로 바이너리 데이터는 먼저 인코딩하십시오(예: `base64_encode`).

```ling
바인드 시작 = 실행 {
    바인드 키 = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
    바인드 봉인됨 = 봉인(키, "정오에 만나요")
    출력(봉인해제(키, 봉인됨))
    출력(봉인해제("ff" + 키, 봉인됨))
}
```

```output
정오에 만나요
Err(decryption/auth failed)
```

## 비밀번호

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `password_hash(password)` | — | — | `비밀번호해시` | — | PHC 문자열 형태의 Argon2id 해시 |
| `password_verify(password, stored)` | — | — | `비밀번호확인` | — | 저장된 해시와 비밀번호 대조 |

`password_hash`는 메모리 64 MiB, 3회 반복, 4레인, 새 무작위 솔트로 Argon2id를 수행하고
`$argon2id$v=19$m=65536,t=3,p=4$...` 같은 문자열을 반환합니다. 문자열 전체를 저장하십시오.
호출마다 의도적으로 수십 밀리초가 걸립니다. `password_verify`는 저장된 문자열에서 매개변수를
읽어 `true` 또는 `false`를 반환합니다(형식이 잘못된 문자열은 `false`).

```ling
바인드 시작 = 실행 {
    바인드 저장값 = 비밀번호해시("올바른 말")
    출력(substr(저장값, 0, 30))
    출력(비밀번호확인("올바른 말", 저장값))
    출력(비밀번호확인("틀림", 저장값))
}
```

```output
$argon2id$v=19$m=65536,t=3,p=4
true
false
```

## 서명

세 계열은 같은 형태를 공유합니다. 키 쌍을 만들고(무작위 또는 32바이트 시드에서), 공개 키를
읽고, 문자열에 서명하고, 공개 키로 검증합니다.

| 계열 | 접두사 | 공개 키 | 서명 | 비고 |
|---|---|---|---|---|
| Ed25519 | `ed25519_` | 32바이트 | 64바이트 | 고전 방식, 빠름, 결정적 |
| ML-DSA-65 (FIPS 204) | `mldsa_` | 1952바이트 | 3309바이트 | 양자 내성, NIST 카테고리 3 |
| ML-DSA-87 (FIPS 204) | `mldsa87_` | 2592바이트 | 4627바이트 | 양자 내성, NIST 카테고리 5, `lingtp://`에서 사용 |

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `ed25519_keygen()` | `生成签名密钥` | `署名鍵生成` | `생성서명키` | `สร้างกุญแจลายเซ็น` | 무작위 키 쌍 핸들 |
| `ed25519_keygen_from_seed(seed_hex)` | `从种子生成签名密钥` | `シードから署名鍵生成` | `씨앗에서생성서명키` | `สร้างกุญแจลายเซ็นจากเมล็ด` | 32바이트 시드에서 키 쌍 생성 |
| `ed25519_public(handle)` | `签名公钥` | `署名公開鍵` | `서명공개키` | `กุญแจสาธารณะลายเซ็น` | 공개 키(16진수) |
| `ed25519_sign(handle, message)` | `签名` | `署名する` | `서명하다` | `เซ็นชื่อ` | 서명(16진수) |
| `ed25519_verify(public_hex, message, sig_hex)` | `验证签名` | `署名検証` | `서명확인` | `ยืนยันลายเซ็น` | 유효하면 `true` |
| `mldsa_keygen()` | `生成量子签名密钥` | `量子署名鍵生成` | `양자서명키생성` | `สร้างกุญแจลายเซ็นควอนตัม` | 무작위 ML-DSA-65 키 쌍 |
| `mldsa_keygen_from_seed(seed_hex)` | `从种子生成量子签名密钥` | `シードから量子署名鍵生成` | `씨앗에서양자서명키생성` | `สร้างกุญแจลายเซ็นควอนตัมจากเมล็ด` | 시드에서 ML-DSA-65 키 쌍 생성 |
| `mldsa_public(handle)` | `量子签名公钥` | `量子署名公開鍵` | `양자서명공개키` | `กุญแจสาธารณะลายเซ็นควอนตัม` | 공개 키(16진수) |
| `mldsa_sign(handle, message)` | `量子签名` | `量子署名する` | `양자서명하다` | `เซ็นชื่อควอนตัม` | 서명(16진수) |
| `mldsa_verify(public_hex, message, sig_hex)` | `验证量子签名` | `量子署名検証` | `양자서명확인` | `ยืนยันลายเซ็นควอนตัม` | 유효하면 `true` |
| `mldsa87_keygen()` | — | — | — | — | 무작위 ML-DSA-87 키 쌍 |
| `mldsa87_keygen_from_seed(seed_hex)` | — | — | — | — | 시드에서 ML-DSA-87 키 쌍 생성 |
| `mldsa87_public(handle)` | — | — | — | — | 공개 키(16진수) |
| `mldsa87_sign(handle, message)` | — | — | — | — | 서명(16진수) |
| `mldsa87_verify(public_hex, message, sig_hex)` | — | — | — | — | 유효하면 `true` |

### 서명 사용하기

시드를 쓰면 16진수 64자만 저장해 두었다가 재시작 후 같은 키 쌍을 다시 만들 수 있습니다. 시드는
비밀 키처럼 보호하십시오. 알 수 없는 핸들을 주면 `*_public`과 `*_sign`은 `""`를 반환합니다.
검증 내장 함수는 잘못된 서명이나 형식이 잘못된 입력에 대해 `false`를 반환합니다. Ed25519 서명은
결정적이어서 같은 키와 메시지는 항상 같은 서명을 만듭니다.

라이선스 파일이나 릴리스 매니페스트처럼 오래 쓰이는 결과물에는 Ed25519 **와** ML-DSA-87로 함께
서명하고, 둘 다 검증될 때만 받아들이십시오.

```ling
바인드 시작 = 실행 {
    바인드 시드 = "0101010101010101010101010101010101010101010101010101010101010101"
    바인드 고전키 = ed25519_keygen_from_seed(시드)
    바인드 양자키 = mldsa87_keygen_from_seed(시드)
    바인드 문서 = "라이선스: acme, 만료 2031-01-01"
    바인드 서명_가 = ed25519_sign(고전키, 문서)
    바인드 서명_나 = mldsa87_sign(양자키, 문서)
    출력(길이(ed25519_public(고전키)))
    출력(ed25519_verify(ed25519_public(고전키), 문서, 서명_가) && mldsa87_verify(mldsa87_public(양자키), 문서, 서명_나))
    출력(mldsa87_verify(mldsa87_public(양자키), 문서 + "!", 서명_나))
}
```

```output
64
true
false
```

## 양자 내성 키 교환

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `knot_keygen()` | `生成密钥` | `鍵生成` | `키생성` | `สร้างกุญแจปม` | 하이브리드 X25519 + ML-KEM-768 키 쌍 |
| `knot_public(handle)` | `公钥` | `公開鍵` | `공개키` | `กุญแจสาธารณะปม` | 공개 키(1216바이트, 16진수) |
| `knot_encapsulate(public_hex)` | `封装密钥` | `カプセル化` | `캡슐화` | `ห่อกุญแจปม` | `[ciphertext_hex, secret_hex]` |
| `knot_decapsulate(handle, ciphertext_hex)` | `解封装密钥` | `カプセル解除` | `캡슐해제` | `แกะกุญแจปม` | 같은 32바이트 비밀 |

`hybrid_keygen`, `hybrid_public`, `hybrid_encapsulate`, `hybrid_decapsulate`는 영어 동의어입니다.
흐름은 이렇습니다. 수신자가 키 쌍을 만들어 공개 키를 공개하면, 송신자는 그 키로 캡슐화해 비밀을
보관하고 암호문을 보냅니다. 수신자는 캡슐을 풀어 같은 비밀을 얻습니다. 이 비밀을 `crypto_seal`
키로 쓰십시오.

`knot_keygen` 키는 시드로 만들거나 내보낼 수 없으며, 프로세스가 살아 있는 동안만 존재합니다.
영구적인 양자 내성 KEM 신원이 필요하면 `lw_kem_keypair`를 쓰십시오.

```ling
바인드 시작 = 실행 {
    바인드 밥 = 키생성()
    바인드 캡슐 = 캡슐화(공개키(밥))
    바인드 밥의_비밀 = 캡슐해제(밥, 캡슐[0])
    출력(캡슐[1] == 밥의_비밀)
    출력(봉인해제(밥의_비밀, 봉인(캡슐[1], "안녕 밥")))
}
```

```output
true
안녕 밥
```

## 시각적 지문

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `knot_points(text)` | `结点坐标` | `結び目点` | `매듭점` | `จุดปม` | 토러스 매듭 지문의 3D 점 256개 |
| `knot_label(text)` | `结点标签` | `結び目ラベル` | `매듭라벨` | `ป้ายปม` | 그 매듭의 짧은 이름 |
| `holo_points(text)` | `全息点` | `ホログラム点` | `홀로그램점` | `จุดโฮโลแกรม` | 홀로그램 분할의 4D 점(장식용) |
| `holo_fragment_count(text)` | `全息碎片数` | `ホログラム断片数` | `홀로그램조각수` | `จำนวนชิ้นโฮโลแกรม` | 문자열의 조각 수 |

이 함수들은 다이제스트를 사람이 눈으로 비교할 수 있는 모양으로 바꿉니다. `knot_points`의
연속된 세 값씩을 `draw_line_3d`로 그리십시오. 그 자체로는 암호 강도를 더하지 않습니다.

```ling
바인드 시작 = 실행 {
    출력(길이(매듭점("abc")))
    출력(매듭라벨("abc"))
}
```

```output
768
knot-17_12-v5.2
```

## 일회용 코드(TOTP)

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `totp_secret()` | — | — | — | `โทเทนลับ` | 새 무작위 Base32 비밀(160비트) |
| `totp_uri(secret, account, issuer)` | — | — | — | `โทเทนยูอาร์ไอ` | 인증 앱용 `otpauth://` 링크 |
| `totp_verify(secret, code)` | — | — | — | `โทเทนตรวจ` | 6자리 코드 확인 |
| `totp_now(secret)` | — | — | — | `โทเทนตอนนี้` | 지금 유효한 코드 |

여기의 TOTP는 SHA-1, 6자리, 30초 간격의 RFC 6238로, Google Authenticator 및 Authy와 맞습니다.
`totp_verify`는 앞뒤로 한 간격씩 허용하며 재사용을 추적하지 않습니다. 마지막으로 받아들인 코드의
시간 간격은 직접 저장하십시오. `totp_uri`는 URL 인코딩을 하지 않으므로 계정과 발급자에 `&`나
공백을 넣지 마십시오. URI는 `lw_qr`로 QR 코드로 보여 줄 수 있습니다.

```ling
바인드 시작 = 실행 {
    출력(totp_uri("JBSWY3DPEHPK3PXP", "alice@example.com", "Acme"))
    바인드 비밀 = totp_secret()
    출력(길이(비밀))
    출력(totp_verify(비밀, totp_now(비밀)))
}
```

```output
otpauth://totp/Acme:alice@example.com?secret=JBSWY3DPEHPK3PXP&issuer=Acme&algorithm=SHA1&digits=6&period=30
32
true
```

## Ling Wallet 기본 연산: `lw_*`

`lw_*` 계열은 Ling Wallet 프로젝트의 기본 연산 모음입니다. 위의 내장 함수와는 세 가지가
다릅니다.

- **엄격한 입력.** 16진수는 형식이 올바르고 문서에 적힌 길이와 정확히 같아야 합니다. 잘못된
  입력은 고쳐 주지 않고 `Err(reason)`을 반환합니다.
- **비밀 핸들.** 비밀을 만드는 내장 함수는 32자 16진수 핸들인 `Ok(handle)`을 반환합니다.
  `lw_key_drop`은 키를 파기하고 메모리를 지웁니다.
- **검증 함수는 요란하게 실패하지 않습니다.** `lw_id_verify`, `lw_ring_verify`, `lw_pk_valid`는
  형식이 잘못된 입력에 대해 `false`를 반환합니다.

현지화된 별칭은 없습니다.

| 내장 함수 | 용도 |
|---|---|
| `lw_h256(domain, parts)` | 도메인 분리 SHAKE-256, 32바이트 출력. `parts`는 16진수 문자열 리스트 |
| `lw_xof(domain, parts, len)` | 같은 방식으로 `len`바이트 출력 |
| `lw_random(n)` | 안전한 무작위 `n`바이트(최대 1 MiB)를 16진수로 |
| `lw_aead_seal(key, nonce, aad, plaintext)` | 직접 고른 24바이트 논스와 연관 데이터를 쓰는 XChaCha20-Poly1305(모두 16진수) |
| `lw_aead_open(key, nonce, aad, ciphertext)` | `Ok(plaintext_hex)` 또는 `Err("authentication failed")` |
| `lw_password_key(password, salt, m_kib, t, p)` | Argon2id: 암호 문구와 16바이트 솔트에서 32바이트 키 생성 |
| `lw_kem_keypair(seed)` | 96바이트 시드에서 X25519 + ML-KEM-1024 키 생성 |
| `lw_kem_public(handle)` | KEM 공개 키(1600바이트) |
| `lw_kem_encaps(public_hex, context_hex)` | 문맥에 묶인 `Ok([ciphertext_hex, secret_hex])` |
| `lw_kem_decaps(handle, ciphertext_hex, context_hex)` | `Ok(secret_hex)`. 문맥이 다르면 다른 비밀 |
| `lw_id_keypair(seed)` | 32바이트 시드에서 ML-DSA-87 신원 생성 |
| `lw_id_public(handle)` | 신원 공개 키(2592바이트) |
| `lw_id_sign(handle, context, message_hex)` | `"tx.v1"` 같은 문맥 레이블 아래의 서명 |
| `lw_id_verify(public_hex, context, message_hex, sig_hex)` | `true` 또는 `false` |
| `lw_id_fingerprint(public_hex)` | `9ebc-56af-97f2-61af` 같은 짧은 지문 |
| `lw_key_drop(handle)` | 비밀 핸들 파기. 존재했으면 `true` |
| `lw_spend_keypair`, `lw_spend_public`, `lw_onetime_secret`, `lw_onetime_public`, `lw_onetime_public_of`, `lw_onetime_key_image` | LRS-87 링 서명용 키 |
| `lw_ring_sign(message_hex, ring, index, onetime_handle)` | 연결 가능한 링 서명(실험적) |
| `lw_ring_verify(message_hex, ring, key_image_hex, sig_hex)` | `true` 또는 `false` |
| `lw_pk_valid(public_hex)` | 유효한 2944바이트 링 공개 키인가? |
| `lw_mnemonic_from(entropy_hex)` | 엔트로피 32바이트를 BIP-39 영어 24단어 문구(리스트)로 |
| `lw_mnemonic_to(phrase)` | `Ok(entropy_hex)` 또는 `Err(reason)` |
| `lw_pow_hash(header_hex)` | 메모리 집약적 작업 증명 해시(16 MiB, 의도적으로 느림) |
| `lw_pow_trace(header_hex, every)` | 같은 해시와 함께 시각화용 경로 점 |
| `lw_meets_difficulty(hash_hex, difficulty)` | 해시가 난이도 목표를 만족하는가? |
| `lw_qr(text)` | QR 코드를 `1`(어두움)과 `0`(밝음) 행의 리스트로 |

제한된 아이솔레이트에서는 비밀 핸들을 만들거나 쓰는 내장 함수에 `"keys"` 권한이 필요합니다.
메인 스레드는 모든 권한을 가집니다.

### 신원

신원은 문맥 레이블 아래에서 서명합니다. 검증하는 쪽도 같은 레이블을 써야 합니다.

```ling
함수 풀기(r) { 반환 매치 r { 좋아(v) => v, 나쁨(e) => "오류:" + e } }

바인드 시작 = 실행 {
    바인드 신원 = 풀기(lw_id_keypair("0101010101010101010101010101010101010101010101010101010101010101"))
    바인드 공개 = lw_id_public(신원)
    바인드 서명 = lw_id_sign(신원, "문맥", "68656c6c6f")
    출력(lw_id_verify(공개, "문맥", "68656c6c6f", 서명))
    출력(lw_id_verify(공개, "다른문맥", "68656c6c6f", 서명))
    출력(lw_id_fingerprint(공개))
    출력(lw_key_drop(신원))
}
```

```output
true
false
9ebc-56af-97f2-61af
true
```

### 암호 문구에서 키 만들기

`lw_password_key`는 `Ok(key_hex)` 또는 `Err("invalid Argon2id parameters")`를 반환합니다. 솔트는
암호문 옆에 저장하십시오. 비밀일 필요는 없습니다. Ling Wallet 프로필은 `m_kib = 262144`(256 MiB),
`t = 3`, `p = 1`입니다. 예제는 바로 실행되도록 아주 작은 매개변수를 쓰니 그대로 따라 하지
마십시오.

```ling
함수 풀기(r) { 반환 매치 r { 좋아(v) => v, 나쁨(e) => "오류:" + e } }

바인드 시작 = 실행 {
    바인드 솔트 = "00112233445566778899aabbccddeeff"
    출력(풀기(lw_password_key("hunter2", 솔트, 1024, 1, 1)))
}
```

```output
a97cf9fed0c0703d0b60b7351990a06b086636d9fec2c5ef4d2b7a4bae25bb34
```

### 복구 문구(BIP-39)

`lw_mnemonic_from`은 32바이트를 영어 단어 24개로 바꿉니다. `lw_mnemonic_to`는 공백으로 구분된
단어(대소문자 무관)를 받아 단어 수, 단어, 체크섬을 확인합니다.

```ling
함수 풀기(r) { 반환 매치 r { 좋아(v) => v, 나쁨(e) => "오류:" + e } }

바인드 시작 = 실행 {
    바인드 엔트로피 = "1111111111111111111111111111111111111111111111111111111111111111"
    바인드 단어들 = lw_mnemonic_from(엔트로피)
    출력(길이(단어들))
    출력(단어들[0] + " " + 단어들[1] + " " + 단어들[23])
    출력(풀기(lw_mnemonic_to(join(단어들, " "))) == 엔트로피)
    출력(풀기(lw_mnemonic_to("abandon abandon")))
}
```

```output
24
baby mass cake
true
오류:a recovery phrase has exactly 24 words
```

### 링 서명과 작업 증명

LRS-87 링 서명(`lw_ring_sign`)은 소스에서 실험적이라고 표시되어 있습니다. 연구와 시제품에만 쓰고,
잃어서는 안 되는 자산에는 쓰지 마십시오. `lw_pow_hash`는 16 MiB 작업 공간을 할당하며 일부러
느리게 동작합니다. `lw_meets_difficulty`는 난이도를 숫자로 받거나, 2^53을 넘는 값이면 10진수
문자열로 받습니다.

## 함께 보기

- [네트워킹](../reference/networking.md): `lingtp://`와 `p2p_*`가 이 기본 연산을 사용합니다.
- [웹 서버와 HTTP](../reference/web.md): 세션, 로그인, HTTPS.
