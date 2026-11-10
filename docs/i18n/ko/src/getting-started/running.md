# 프로그램 실행

파일 하나는 `ling run`으로, 프로젝트 전체는 `lingfu run`으로 실행합니다. 이 페이지에서는 두
방법을 보여 주고, Ling이 코드를 실행하는 두 가지 백엔드와 프로그램을 배포 가능한 형태로 만드는
방법을 설명합니다.

## 파일 실행

```sh
ling run hello.ling
ling hello.ling          # 같은 동작의 줄임 형태
```

`ling run`은 파일을 파싱하고 시작점(`바인드 시작 = 실행 { ... }`)을 찾아 실행합니다. 소스
파일의 확장자는 `.ling`, `.灵`, `.霊`, `.령`, `.ลิง` 중 아무것이나 쓸 수 있습니다.

파일 이름 뒤의 인수는 명령줄에 남아 프로그램이 읽을 수 있습니다. `get_args()`는 명령줄
전체를 돌려주고, `cli_arg("port", "8080")`은 `--port 99` 같은 옵션을 읽으며 없으면
`"8080"`을 돌려줍니다.

## JIT와 인터프리터

Ling에는 두 가지 백엔드가 있습니다.

| 명령 | 백엔드 | 용도 |
|---|---|---|
| `ling run file.ling` | Cranelift JIT(기본값) | 속도. 먼저 프로그램을 기계어로 컴파일합니다. |
| `ling run --interp file.ling` | 트리 순회 인터프리터 | 진단. Ling 동작의 기준입니다. |

JIT가 프로그램을 컴파일하지 못하면 `ling run`은 프로그램 전체를 인터프리터로 실행합니다. 아주
큰 프로그램의 컴파일이 너무 오래 걸리면 `--O0`, `--O1`, `--O2`, `--O3`(기본값 `--O3`)으로
JIT 최적화 수준을 낮출 수 있습니다.

### 프로그램이 "아무것도 안 할" 때는 `--interp`

JIT에서는 존재하지 않는 함수를 호출해도 아무 값도 돌려주지 않고 프로그램이 계속 진행됩니다.
다음 프로그램에는 `draw_rectt`라는 오타가 있습니다.

```ling
바인드 시작 = 실행 {
    출력("이전")
    draw_rectt(1, 2)
    출력("이후")
}
```
```output
이전
이후
```

인터프리터는 멈추고 무엇이 잘못되었는지 알려 줍니다.

```text
$ ling run --interp typo.ling
[detected language: Korean (한국어)]
이전
error[runtime]: unknown function 'draw_rectt'
 --> typo.ling
  traceback (deepest call last):
    0: start
  hint: check the spelling, or `use` the module that defines it
```

무언가가 무시되는 것 같으면 언제든 `--interp`로 다시 실행해 보십시오. 두 백엔드의 다른 차이는
[알려진 제한 사항](../language/limits.md)에 정리되어 있습니다.

### 서버는 인터프리터에서 실행됩니다

`http_serve`나 `lingtp_serve`를 호출하는 프로그램은 `--interp`가 없어도 항상 인터프리터에서
실행됩니다. 요청 핸들러는 내장 함수에 넘기는 클로저인데, JIT는 아직 이를 지원하지 않습니다.
웹 서비스는 대부분의 시간을 네트워크를 기다리며 보내므로 체감할 만한 손해는 없습니다.
[작은 웹 서비스](../tutorials/web-service.md)를 참고하십시오.

### 브라우저에서 실행

`ling run --wasm file.ling`(또는 `--web`)은 프로그램을 WebAssembly로 빌드하고, 로컬 웹 서버로
제공한 뒤 브라우저를 엽니다. 첫 빌드는 런타임을 컴파일하므로 1분 정도 걸릴 수 있습니다.
`ling` 옆에 `lingc` 실행 파일이 있어야 합니다.

## lingfu로 프로젝트 관리

파일 하나보다 큰 프로그램이라면 `lingfu`로 프로젝트를 만드십시오.

```sh
lingfu new demo
cd demo
lingfu run
```

```text
Running ...\demo\src\start.ling...
🌊 Welcome to demo Spirit Realm 🌊
Spirit Level: 铁
```

`lingfu new`는 다음 구조를 만듭니다.

```text
demo/
├── Ling.toml          매니페스트
├── README.md
├── .gitignore
├── ling.ignore        배포할 때 제외할 파일
├── lexicons/en.ling
├── src/start.ling     시작 프로그램
└── tests/test.ling
```

| 명령 | 하는 일 |
|---|---|
| `lingfu run` | `ling run`으로 시작 프로그램을 실행합니다. 추가 인수는 그대로 넘깁니다. |
| `lingfu test` | `tests/` 아래의 모든 `.ling` 파일을 하나씩 실행합니다. |
| `lingfu build` | `ling build --aot`로 네이티브 실행 파일을 빌드합니다. |
| `lingfu clean` | 빌드 결과물을 지웁니다. |

`lingfu new demo --lang ko`는 폴더와 파일 이름까지 한국어로 된 프로젝트를 만듭니다. 매니페스트는
`영부.toml`, 시작 프로그램은 `령원/시작.령`입니다. 모든 명령은 [lingfu](../tools/lingfu.md)에서
다룹니다.

## Ling.toml

`lingfu new`가 만드는 매니페스트는 다음과 같습니다.

```toml
[spirit]
name = "demo"
kind = "bin"
version = "2030.0.0"

[author]
ling-lin = { email = "you@example.com" }

[realm]
language = "en"
level    = "铁"
source   = "src"

[root]
core = { version = "2030.0", language = "en" }

[depends]
# Add dependencies: spirit-name = { version = "x.y.z" }
```

- `[spirit]`은 프로젝트 이름을 정합니다. 여기에 `entry = "src/start.ling"`을 추가할 수 있습니다.
- `[realm]`은 프로젝트의 언어와 소스 폴더를 정합니다.
- `[root]`와 `[depends]`에는 `core` 의존성과 여러분의 패키지를 적습니다.

`lingfu run`과 `lingfu test`는 `src/start.ling`을 스스로 찾습니다. 하지만 `ling build`(따라서
`lingfu build`도)는 매니페스트에 따로 적지 않으면 `main.ling`을 찾으므로, 빌드하기 전에
**`[spirit]` 아래에 `entry = "src/start.ling"`을 추가하십시오**.

## 프로그램 빌드

`ling build`는 파일이나 프로젝트 폴더를 배포할 수 있는 형태로 만듭니다.

```sh
ling build game.ling                      # 현재 플랫폼 + web, dist/에 출력
ling build . --platform win,web --out dist
ling build . --platform all --pack --icon logo.svg
```

| 옵션 | 의미 |
|---|---|
| `--out <dir>` | 출력 폴더(기본값 `dist`). 플랫폼마다 하위 폴더가 생깁니다. |
| `--platform <list>` | 쉼표로 구분: `web`, `win`, `lin`, `mac`, `macarm`, `pios`, `kernel`, `rpi`, `lingos`, `lingos-rpi`. `all`은 `win,lin,mac,web`을 뜻합니다. |
| `--icon <file>` | 앱 아이콘(`.svg`, `.png`, `.ico` 등). |
| `--pack` | 매니페스트의 `[includes]` 파일을 실행 파일에 넣습니다. |
| `--aot` | 인터프리터를 넣는 대신 미리 기계어로 컴파일합니다. |

`--platform`이 없으면 매니페스트의 `targets` 목록을 쓰고, 그것도 없으면 현재 플랫폼과 `web`을
빌드합니다. 데스크톱 대상은 작은 Rust 프로젝트를 생성해 컴파일하므로 Rust 툴체인이 설치되어
있어야 하고, 첫 빌드는 몇 분 걸릴 수 있습니다. `web` 대상은 `lingc` 실행 파일을 사용합니다.

전체 옵션과 매니페스트 키는 [ling 명령](../tools/ling.md)에 있습니다.
