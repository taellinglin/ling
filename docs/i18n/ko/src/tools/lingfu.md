# lingfu

`lingfu`(灵符, "영부")는 Ling의 프로젝트 관리자입니다. 프로젝트를 만들고, 프로젝트와 테스트를
실행하고, 실행 파일을 빌드하고, 패키지 레지스트리와 통신하며, 프로젝트 전체를 다른 인간 언어로
다시 쓸 수도 있습니다. 실행과 빌드에는 `ling`을 호출하는데, 먼저 자기 옆에서 찾고 없으면
`PATH`에서 찾습니다.

```text
lingfu <command> [arguments]
lingfu --help | -h | help
lingfu --version | -V | version
```

`lingfu`는 현재 폴더와 상위 폴더에서 매니페스트를 찾아 프로젝트를 정합니다: `Ling.toml`,
`灵符.toml`, `霊符.toml`, `영부.toml`, `ลิงฟู.toml`.

## 명령 한눈에 보기

| 명령 | 별칭 | 용도 |
|---|---|---|
| `new <name>` | `create` | 프로젝트 폴더를 만듭니다. |
| `init` | `initialize` | 현재 폴더에 프로젝트를 만듭니다. |
| `run` | `execute`, `start` | 프로젝트를 실행합니다. |
| `test` | `verify` | 프로젝트의 테스트를 실행합니다. |
| `build` | `compile`, `make` | 네이티브 실행 파일을 빌드합니다. |
| `clean` | `purge`, `clear` | 빌드 결과물을 지웁니다. |
| `tree` | `structure`, `ls` | 프로젝트 트리를 출력합니다. |
| `manifest` | `show`, `info` | 매니페스트를 출력합니다. |
| `normalize <lang>` | `norm`, `localize` | 프로젝트를 다른 언어로 다시 씁니다. |
| `add <package>` | `install`, `include` | 레지스트리에서 패키지를 설치합니다. |
| `search <query>` | `find`, `seek` | 레지스트리를 검색합니다. |
| `publish` | `release`, `share` | 프로젝트를 배포합니다. |
| `login <api-key>` | `sign-in`, `auth` | 레지스트리 자격 증명을 저장합니다. |
| `logout` | `sign-out`, `deauth` | 자격 증명을 지웁니다. |
| `doc` | `docs`, `document` | 문서를 빌드합니다. |
| `bench`, `update` | | `cargo bench` / `cargo update`를 실행합니다. |
| `wizard` | `guide`, `create-wizard` | 대화형 프로젝트 생성기. |
| `seed` | | 프로젝트의 무결성 지문. |
| `ast`, `convert` | | `ling ast` / `ling convert`로 넘깁니다. |
| `check`, `fmt`, `translate` | | 아직 사용하지 마십시오(아래 참고). |

공통 옵션: `--lang`/`-l <code>`(스캐폴딩 언어: `en`, `zh`, `ja`, `ko`, `th`, `ru`),
`--type`/`-t <kind>`, `--root-lang <code>`, `--crates`, `--path <dir>`, `--dry-run`/`-n`.

각 명령에는 다른 언어 이름도 있습니다. 예를 들어 `test`는 중국어로 `试`, 태국어로 `ทดสอบ`입니다.
이런 이름은 실행 파일 자체의 이름이 현지화된 이름(`灵符.exe`, `ลิงฟู` 등)일 때만 동작합니다.
`lingfu`라는 이름으로 실행할 때는 영어 명령을 쓰십시오.

### `lingfu new`와 `lingfu init`

```text
lingfu new <name> [--type bin|lib|game|ui|ai|crypto|web|polyglot] [--lang en|zh|ja|ko|th|ru]
```

매니페스트, 시작 프로그램이 든 소스 폴더, 테스트 폴더, 어휘 폴더, `README.md`, `.gitignore`,
`ling.ignore`를 담은 `<name>/`을 만듭니다. `lingfu init`은 같은 일을 현재 폴더에서 하며, 폴더
이름을 프로젝트 이름으로 씁니다.

`--lang`은 생성되는 파일의 언어**와** 폴더 이름을 정합니다.

| 언어 | 매니페스트 | 소스 | 시작 파일 | 테스트 | 빌드 |
|---|---|---|---|---|---|
| 영어 | `Ling.toml` | `src` | `start.ling` | `tests` | `build` |
| 중국어 | `灵符.toml` | `灵源` | `启.灵` | `灵镜` | `灵碑` |
| 일본어 | `霊符.toml` | `霊源` | `啓.霊` | `霊鏡` | `霊碑` |
| 한국어 | `영부.toml` | `령원` | `시작.령` | `령경` | `령비` |
| 태국어 | `ลิงฟู.toml` | `ต้นกำเนิด` | `เริ่ม.ลิง` | `ทดสอบ` | `สร้าง` |

매니페스트에는 `[spirit]`(`name`, `kind`, `version`, 선택적으로 `entry`), `[author]`,
`[realm]`(`language`, `level`, `source`), `[root]`(`core` 의존성), `[depends]` 표가 있습니다.
예시는 [프로그램 실행](../getting-started/running.md#lingtoml)을 참고하십시오.

### `lingfu run`

프로젝트의 시작 프로그램을 `ling run`으로 실행합니다. 시작 프로그램은 다음 중 처음으로 존재하는
파일입니다: 매니페스트의 `entry`, `<src>/<시작 파일>`, `<root>/<시작 파일>`, `<src>/main.ling`,
`<root>/main.ling`. 추가 인수는 `ling run`에 넘깁니다.

```text
$ lingfu run
Running ...\demo\src\start.ling...
🌊 Welcome to demo Spirit Realm 🌊
Spirit Level: 铁
```

### `lingfu test`

테스트 폴더 아래의 모든 `.ling` 파일을 `ling run`으로 하나씩 실행하고, 처음 실패한 곳에서
멈춥니다. 테스트는 시작점이 있는 평범한 프로그램입니다(`실행` 블록을 바인딩하는 `바인드`는 모두
시작점으로 인정됩니다).

```text
$ lingfu test
Testing Ling project...
  test: ...\demo\tests\test.ling
✨ All mirrors are clear ✨
🔮 Ling tests passed for demo
✓ All tests passed!
```

테스트는 JIT에서 실행되는데, JIT는 알 수 없는 함수에서 멈추지 않으므로 망가진 테스트도
"통과"할 수 있습니다. 테스트마다 PASS나 FAIL을 분명히 출력하게 하거나, 직접
`ling run --interp`로 실행하십시오. 테스트 폴더 안의 보조 파일도 테스트로 취급됩니다.

### `lingfu build`

`ling build <project> --aot`를 호출하고 `--out`, `--platform`, `--icon`, `--pack`을 넘깁니다.
`--no-aot`는 대신 인터프리터를 넣고, `--path <dir>`는 프로젝트 폴더를 지정합니다. 매니페스트
옆에 `Cargo.toml`이 있으면 대신 `cargo build`를 실행합니다.

첫 빌드 전에 `[spirit]` 아래에 `entry = "src/start.ling"`을 추가하십시오. 그렇지 않으면
`ling build`가 `main.ling`을 찾다가 `entry '...' not found`로 멈춥니다. 빌드에는 Rust 툴체인이
필요합니다. [ling 명령](ling.md#ling-build)을 참고하십시오.

### `lingfu clean`, `tree`, `manifest`

- `clean`은 빌드 폴더와 잠금 폴더(`build`와 `.lock`, 또는 현지화된 이름)를 지우고, Rust
  크레이트가 있으면 `cargo clean`을 실행합니다.
- `tree`는 프로젝트의 폴더 트리를 출력합니다.
- `manifest`는 매니페스트의 경로와 내용을 출력합니다.

### `lingfu normalize`

```text
lingfu normalize <lang> [--dry-run|-n] [--content-only] [--files-only]
```

현재 폴더의 프로젝트 전체를 하나의 인간 언어로 다시 씁니다. 모든 소스 파일의 키워드와 내장 함수
이름을 번역하고, 소스 파일과 표준 폴더의 이름을 바꾸고, 가져오기 경로도 맞게 고칩니다.
`<lang>`은 `en`, `zh`, `ja`, `ko`, `th` 같은 코드입니다(`korean`, `한국어` 같은 이름도 됩니다).
생략하면 메뉴가 나옵니다.

| 옵션 | 효과 |
|---|---|
| `--dry-run`, `-n` | 아무것도 바꾸지 않고 변경 사항만 나열합니다. |
| `--content-only` | 파일 내용만 다시 쓰고 이름은 그대로 둡니다. |
| `--files-only` | 파일과 폴더 이름만 바꾸고 내용은 그대로 둡니다. |

```text
$ lingfu normalize zh --dry-run
Normalizing to Chinese  [dry-run]
  ~ lexicons\en.ling
  ~ src\start.ling
  ~ tests\test.ling
  mv tests\test.ling → tests\测试.ling
  mv build → 构建
  mv src → 源码
  mv tests → 测试

Dry run complete — no files changed.
  ✓ 3 file(s) rewritten
  ~ 1 file(s) renamed
  ↕ 3 folder(s) renamed
```

> **경고:** `normalize`는 폴더 안의 **모든** Ling 파일을 그 자리에서 다시 씁니다. 먼저
> 프로젝트를 커밋(또는 복사)하고 시험 실행부터 하십시오.

식별자, 주석, 문자열 안의 텍스트는 그대로 두고 키워드와 내장 함수만 바꿉니다. 또한
`normalize`는 자체 폴더 이름(중국어의 경우 `源码`, `测试`, `构建`)을 쓰므로, `lingfu new`가
만드는 이름과 다릅니다.

### `lingfu check`, `fmt`, `translate`

이 명령들은 존재하지만 `ling`에 없는 하위 명령(`ling check`, `ling fmt`,
`ling <from> <to> <file>`)을 호출합니다. `check`와 `fmt`는 아무것도 하지 않고 성공을 보고하며,
`translate`는 `ling`의 사용법 안내만 출력합니다. 프로젝트의 언어를 바꾸려면
`lingfu normalize`를 쓰십시오.

### 레지스트리 명령

레지스트리는 기본적으로 `https://fu.ling-lang.org`입니다. 다른 레지스트리를 쓰려면
`LINGFU_REGISTRY`를 설정하십시오.

| 명령 | 하는 일 |
|---|---|
| `login <api-key>` | 키를 `~/.lingfu/credentials.toml`에 저장합니다. |
| `logout` | 저장된 자격 증명을 지웁니다. |
| `publish` | 프로젝트를 `.tar.gz`로 묶어(`ling.ignore`에 적힌 파일은 제외) 올립니다. `login`이 필요합니다. |
| `search <query>` | 레지스트리를 검색합니다. `--crates`를 붙이면 대신 crates.io를 검색합니다. |
| `add <package>` | 패키지를 `./ling_modules/<name>/`에 내려받고, 가져올 때 쓸 `use` 줄을 출력합니다. `--crates`를 붙이면 `cargo add`를 실행합니다. |

### `lingfu doc`, `bench`, `update`

`doc`은 `docs/book.toml`이나 `book/book.toml`이 있으면 `mdbook build`를, 없으면
`cargo doc --no-deps`를 실행합니다. `bench`와 `update`는 `cargo bench`와 `cargo update`를
실행하며, Rust 크레이트가 들어 있는 프로젝트에서만 쓸모가 있습니다.

### `lingfu seed`

git 프로젝트의 암호화된 무결성 지문을 만듭니다. 추적되는 모든 파일의 BLAKE3 머클 루트를 재생
가능한 `seed.wav`로 씁니다. `--check`는 프로젝트를 저장된 지문과 비교합니다. git 저장소가
필요합니다.

### `lingfu wizard`

대화형 질문(언어, 프로젝트 종류, 이름 등)을 거쳐 마지막에 `new`처럼 프로젝트를 만듭니다.
터미널에서 입력을 받으므로 스크립트로 자동화할 수 없습니다.

## 종료 상태

`lingfu`는 성공하면 0을 돌려줍니다. 실패하면 `Error: <message>`(예: `build failed`,
`tests failed`)를 출력하고 1을 돌려줍니다.
