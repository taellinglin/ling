# 설치

Ling은 명령 한 줄로 설치할 수 있습니다. 설치하면 두 프로그램이 생깁니다. `ling`은 Ling
프로그램을 실행하고 빌드하며, `lingfu`는 프로젝트 관리자입니다.

## 한 줄 설치

```sh
# macOS / Linux
curl --proto '=https' --tlsv1.2 -sSf https://ling-lang.org/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://ling-lang.org/install.ps1 | iex
```

Rust 툴체인이 없으면 스크립트가 먼저 설치합니다([rustup](https://rustup.rs)과 같은 방식).
그다음 crates.io에서 `ling`과 `lingfu`를 빌드합니다.

## Cargo로 설치

이미 Rust가 있다면 crates.io에서 두 프로그램을 바로 설치할 수 있습니다.

```sh
cargo install ling-lang lingfu
```

두 프로그램은 같은 폴더(`~/.cargo/bin`)에 설치됩니다. `lingfu`는 먼저 자기 옆에서 `ling`을
찾고, 없으면 `PATH`에서 찾습니다.

## 소스에서 빌드

```sh
git clone https://github.com/taellinglin/ling
cd ling
cargo build --release
# 실행 파일은 target/release/에 있습니다 (ling, 그리고 lingc, lx, ling-repl)
```

같은 소스에서 `lingfu`를 빌드하려면 `cargo build --release -p lingfu`를 실행합니다.

## 설치 확인

인수 없이 `ling`을 실행합니다. 사용법 안내의 첫 줄이 버전입니다.

```sh
ling
# ling 2030.1.53 — The Omniglot Systems Language
# Usage:
#   ling run <file.ling>   ...
lingfu --version
# lingfu v2030.1.19
```

`ling`에는 `--version` 옵션이 없습니다. 버전은 이 사용법 안내로 확인합니다.

## 설치 프로필

`cargo install ling-lang`은 모든 기능(`full` 프로필)을 설치합니다. 더 적게 빌드하려면 기본값을
끄고 필요한 프로필만 고릅니다.

```sh
cargo install ling-lang --no-default-features --features core        # 컴파일러와 인터프리터만
cargo install ling-lang --no-default-features --features web         # + HTTP 서버/클라이언트, SQLite
cargo install ling-lang --no-default-features --features game,audio  # 소리가 있는 게임 키트
```

| 프로필 | 추가되는 기능 |
|---|---|
| `core` | 렉서, 파서, Cranelift JIT와 AOT, 인터프리터, 소프트웨어 창과 그리기(`draw_*`, 글꼴, 이미지, 메시). 모든 프로필에 포함됩니다. |
| `web` | `http_*` 서버와 클라이언트([웹 서버와 HTTP](../reference/web.md)), `db_*` SQLite([데이터베이스 (SQLite)](../reference/database.md)), 정적 파일, TOTP, zip/tar, 동영상 재생, 프로세스, 해시와 `password_hash`. |
| `privacy` | 양자 내성 신원, `lingtp://`, P2P `p2p_*`, Ling 지갑 `lw_*`, `ssh_*`, 그리고 `web`과 `ui_*` 위젯. |
| `game` | 물리(연체, 강체, 액체, glTF), GPU 래스터, 애니메이션, 대화, 게임패드, 음향 바디, `ui_*` 위젯. |
| `ai` | 신경망(`nn_*`), 행동 트리(`bt_*`), 학습형 대화와 음성 인식(`stt_*`). |
| `audio` | 사운드 엔진, 음악 디코딩·합성·MIDI, 마이크, LingStationX DAW. |
| `os` | `ling build --platform kernel`, `rpi`, `lingos`, `lingos-rpi`(LingOS 이미지). |
| `full` | 위의 전부(기본값). LingAnime 페인트 엔진인 `art`(`art_*`)도 포함합니다. |

빌드에서 빠진 프로필의 내장 함수를 호출하면 프로그램은 오류를 내고 멈춥니다. 오류 메시지에
어떤 프로필로 다시 설치해야 하는지 나옵니다. 무거운 추가 기능은 어떤 프로필 위에서든 따로
켜야 합니다: `llm`, `llm-cuda`, `vision`, `cuda`, `gpu-cuda`, `gpu`, `llvm`, `emu`.

## 요구 사항

- Rust 1.85 이상(설치 스크립트가 알아서 설치합니다).
- Linux: 오디오에 `libasound2-dev`가 필요합니다.
- macOS와 Windows: 추가로 필요한 것이 없습니다.
- 데스크톱 대상으로 `ling build`를 하면 작은 Rust 프로젝트를 컴파일하므로 Rust 툴체인이
  필요합니다. [ling 명령](../tools/ling.md)을 참고하십시오.

## 다음 단계

- 첫 프로그램 작성: [헬로 월드](hello.md).
- 명령 익히기: [프로그램 실행](running.md).
- [Ling 북](../books.md)을 읽어 보십시오. 초급, 중급, 고급, 그리고 완전한 레퍼런스로 이루어진
  무료 PDF 네 권입니다.
