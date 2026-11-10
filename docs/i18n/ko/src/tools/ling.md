# ling 명령

`ling`은 Ling 프로그램을 실행하고, 빌드하고, 그리고, 변환합니다. 이 페이지에는 모든 하위 명령과
옵션이 정리되어 있습니다. 가벼운 소개가 필요하다면 [프로그램 실행](../getting-started/running.md)부터
읽으십시오.

```text
ling run [--interp|--wasm|--O0|--O1|--O2|--O3] <file.ling>
ling <file.ling>                       (shorthand for: ling run <file.ling>)
ling build <file.ling|dir> [--out dir] [--platform list] [--icon file] [--pack] [--aot]
ling ast [path] [--technical|--artwork|--ling|--all] [--out dir]
ling visualize <file.ling>
ling convert <asset> [-o out.ling] [--no-compression]
```

| 하위 명령 | 용도 |
|---|---|
| `run` | 프로그램을 실행합니다(기본값은 JIT). |
| `build` | 파일이나 프로젝트를 하나 이상의 플랫폼용 배포물로 컴파일합니다. |
| `ast` | 파일이나 프로젝트의 구문 트리를 SVG 파일로 그립니다. |
| `visualize` | 파일의 구문 트리를 SVG 하나로 표준 출력에 씁니다. |
| `convert` | 에셋(모델, 소리, MIDI, SVG)을 가져올 수 있는 `.ling` 파일로 바꿉니다. |

인수 없이 `ling`을 실행하거나 알 수 없는 인수를 주면 사용법 안내를 출력하고 상태 0으로
끝납니다. 첫 줄이 버전입니다. `--version`이나 `--help` 옵션은 없고, `check`, `fmt`,
`translate`, `test` 하위 명령도 없습니다.

소스 파일의 확장자는 `.ling`, `.灵`, `.霊`, `.령`, `.ลิง` 중 아무것이나 쓸 수 있습니다.

### `ling run`

파일을 파싱하고 시작점을 찾아 실행합니다.

| 옵션 | 효과 |
|---|---|
| *(없음)* | 기본값인 Cranelift JIT. JIT가 컴파일하지 못하면 프로그램 전체를 인터프리터로 실행합니다. |
| `--interp` | 트리 순회 인터프리터. 의미상의 기준이며 오류 메시지가 가장 자세합니다. |
| `--wasm`(또는 `--web`) | 프로그램을 WebAssembly로 빌드해 로컬에서 제공하고 브라우저를 엽니다. `ling` 옆에 `lingc`가 있어야 합니다. |
| `--O0` `--O1` `--O2` `--O3` | JIT 최적화 수준(기본값 `--O3`). 아주 큰 프로그램의 컴파일이 느리면 낮추십시오. |

옵션은 파일 이름 앞이나 뒤에 올 수 있습니다. Ling 확장자를 가진 첫 인수가 프로그램입니다. 그
뒤의 인수는 프로그램이 `get_args()`와 `cli_arg(name, default)`로 읽을 수 있게 남겨 둡니다.

알아 둘 점:

- `http_serve`나 `lingtp_serve`(또는 태국어 `เว็บเสิร์ฟ` 같은 별칭)를 호출하는 프로그램은
  자동으로 인터프리터에서 실행됩니다. JIT가 아직 클로저 인수를 지원하지 않기 때문입니다.
  줄임 형태 `ling file.ling`은 이 검사를 하지 않으므로 서버는 `ling run`으로 실행하십시오.
- JIT에서는 알 수 없는 함수가 조용히 `()`를 돌려줍니다. 인터프리터는 `unknown function`을
  보고합니다. [알려진 제한 사항](../language/limits.md)을 참고하십시오.
- 시작점이 없는 파일은 라이브러리로 취급되어 `error[runtime]: no entry point`로 실패합니다.
- 다른 언어로 작성된 소스 파일이면 `ling`은 실행 전에 표준 오류에 `[detected language: ...]`를
  출력합니다.
- 파일이 없으면 `[ling] error: file does not exist: <path>`를 출력합니다. 구문 오류는
  `error[parse]: ...`, 실행 오류는 트레이스백이 붙은 `error[runtime]: ...`입니다. 모두 상태 1로
  끝납니다.

### `ling build`

프로그램이나 프로젝트를 배포물로 컴파일합니다.

| 옵션 | 의미 |
|---|---|
| `<target>` | `.ling` 파일이나 디렉터리(기본값 `.`). 디렉터리는 매니페스트(`灵符.toml`, `ลิงฟู.toml`, `Ling.toml` 순)로 읽습니다. 매니페스트가 없으면 `main.ling`, `start.ling`, `启.灵`, `เริ่ม.ลิง`, `src` 폴더, 또는 아무 `.ling` 파일을 찾습니다. |
| `--out <dir>` | 출력 폴더, 기본값 `dist`. 플랫폼마다 `dist/web`, `dist/windows` 같은 하위 폴더에 들어갑니다. |
| `--platform <list>` | 쉼표로 구분한 대상(아래 표). `all`은 `win,lin,mac,web`을 뜻합니다. |
| `--icon <file>` | 앱 아이콘(`.svg`, `.png`, `.jpg`, `.bmp`, `.ico`). 매니페스트의 `icon`보다 우선합니다. |
| `--pack` | `[includes]` 리소스와 프로그램을 실행 파일에 넣어 독립된 파일 하나로 만듭니다. |
| `--aot` | 인터프리터를 넣는 대신 미리 기계어로 컴파일합니다. |

| 플랫폼 | 별칭 | 결과물 |
|---|---|---|
| `web` | | `lingc`가 빌드하는 WebGL/WebAssembly 번들 |
| `win` | `windows` | Windows 실행 파일 |
| `lin` | `linux` | Linux 실행 파일 |
| `mac` | `macos`, `darwin` | macOS(Intel) 실행 파일 |
| `macarm` | `mac-arm`, `mac-arm64`, `macos-arm64`, `apple-silicon` | macOS(Apple Silicon) 실행 파일 |
| `pios` | `pi`, `raspios`, `linux-arm64`, `linarm`, `arm64` | Raspberry Pi OS / 64비트 ARM Linux |
| `kernel` | `bare` | LingOS 베어메탈 커널 이미지(`os` 프로필) |
| `rpi` | `raspberrypi` | 라즈베리 파이용 LingOS 이미지(`os` 프로필) |
| `lingos` | `app`, `apps` | LingOS 사용자 영역 앱(`os` 프로필) |
| `lingos-rpi` | `apps-rpi`, `app-rpi` | 라즈베리 파이용 LingOS 사용자 영역 앱(`os` 프로필) |

`--platform`이 없으면 매니페스트의 `targets`를 쓰고, 그것도 없으면 현재 플랫폼과 `web`을
빌드합니다. 목록을 매니페스트에서 가져온 경우, 현재 컴퓨터에서 빌드할 수 없는(그리고 `cross`가
설치되지 않은) 데스크톱 대상은 안내와 함께 건너뜁니다. `--platform`을 직접 지정하면 대신
실패합니다.

네이티브 대상은 작은 Rust 프로젝트를 생성해 컴파일하므로 **Rust 툴체인이 설치되어 있어야
하고**, 첫 빌드는 몇 분 걸릴 수 있습니다.

#### 매니페스트 키

`ling build`는 매니페스트를 완전한 TOML 파서가 아니라 줄 단위로 읽습니다. 모든 설정은 한 줄에
`key = value` 형태로 쓰고, 배열도 한 줄에 쓰십시오.

| 키 | 의미 |
|---|---|
| `name` | 프로젝트 이름. 실행 파일 이름으로 쓰입니다. |
| `version` | 버전 문자열. 기본값 `0.1.0`. |
| `entry` | 매니페스트 기준의 시작 프로그램. 기본값 `main.ling`. |
| `kind` | `bin`(기본값), `web`, `game`, `ui`, `ai`, `crypto`, `lib`, `polyglot`, `kernel`. |
| `icon` | 아이콘 파일. |
| `targets` | 기본 플랫폼. 예: `targets = ["win", "lin", "web"]`. |
| `pack` | `pack = true`는 `--pack`과 같습니다. |
| `includes` | 리소스 글롭, 또는 한 줄에 따옴표로 감싼 패턴 하나씩 쓰는 `[includes]` 섹션. `*`는 폴더 안에서, `**`는 폴더를 넘어 일치합니다. |
| `ling-features` | 실행 파일에서 켤 `ling`의 Cargo 기능. 예를 들어 `db_*`를 쓰는 프로그램은 `["web"]`. |

```toml
# Ling.toml
[project]
name = "my-game"
version = "1.0.0"
entry = "main.ling"
icon = "images/logo.svg"
targets = ["win", "lin", "web"]
pack = true

[includes]
"/music/*.wav"
"/font/*.otf"
"/levels/**/*.json"
```

포함된 파일은 같은 구조로 실행 파일 옆에 복사되거나, `--pack`을 쓰면 안에 들어갑니다. 어느
쪽이든 `read_file` 같은 파일 내장 함수는 똑같이 동작합니다.

### `ling ast`

파일이나 프로젝트 전체의 구문 트리를 SVG로 그립니다. 디렉터리는 재귀적으로 탐색하며(숨김 폴더,
`target`, `dist`, `node_modules`, `AST`, `灵碑`는 건너뜀) 하나의 프로그램으로 취급합니다.

| 옵션 | 의미 |
|---|---|
| `[path]` | 파일이나 디렉터리, 기본값 `.`. |
| `--technical`, `--artwork`, `--ling` | 그리기 스타일을 하나 이상 고릅니다. 아무것도 고르지 않으면 세 가지를 모두 만듭니다. |
| `--all` | 세 가지 스타일 모두. |
| `--out <dir>` | 출력 폴더, 기본값 `AST`. |

```text
$ ling ast hello.ling
ling ast: 'hello' — 1 file(s), 0 fn(s)
  ✓ AST\hello.technical.svg
  ✓ AST\hello.artwork.svg
  ✓ AST\hello.ling.svg
```

### `ling visualize`

파일의 구문 트리를 SVG 그림 하나로 표준 출력에 씁니다. 파일로 리디렉션하십시오.

```sh
ling visualize hello.ling > hello.svg
```

파일을 파싱만 하므로 프로그램이 문법적으로 올바른지 빠르게 확인하는 데에도 쓸 수 있습니다.

### `ling convert`

에셋을 `사용`으로 가져올 수 있는 `.ling` 파일로 바꿉니다. [에셋 변환](convert.md)을
참고하십시오.

```sh
ling convert model.glb -o model.ling
```

## 종료 상태

`ling`은 성공하면 0, 오류를 보고하면 1을 돌려줍니다.
