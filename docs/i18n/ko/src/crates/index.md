# 크레이트 개요

Ling은 Rust로 작성되었고 여러 크레이트로 이루어진 워크스페이스로 나뉘어 있습니다. 주 크레이트인
`ling-lang`에는 컴파일러 프런트엔드, 인터프리터, JIT 드라이버, `ling` 명령이 들어 있고, 아래의
크레이트들이 언어의 구성 요소와 내장 함수 라이브러리를 제공합니다. 이 크레이트들을 여러분의 Rust
코드에서 쓰고 싶거나, 어떤 크레이트가 어떤 내장 함수를 뒷받침하는지 알고 싶을 때만 이 페이지가
필요합니다.

아래 버전은 이 글을 쓸 때 저장소에 있던 버전입니다. 대부분의 크레이트는 Ling의 `2030.x` 번호를
따르며, `ling-http`와 `ling-web`은 자체 번호를 씁니다.

## 주 크레이트

| 크레이트 | 버전 | 설명 |
|---|---|---|
| `ling-lang` | 2030.1.53 | 언어 자체: 렉서, 파서, 인터프리터, Cranelift JIT와 AOT, 내장 함수. 라이브러리 이름은 `ling`이고, 실행 파일은 `ling`, `lingc`, `lx`, `ling-repl`입니다. |
| `lingfu`(`crates/ling-fu`) | 2030.1.19 | 프로젝트 관리자: `lingfu new`, `run`, `build`, `normalize` 등. [lingfu](../tools/lingfu.md)를 참고하십시오. |

## 컴파일러

| 크레이트 | 버전 | 설명 |
|---|---|---|
| `ling-core` | 2030.0.9 | 핵심 타입, 오류, 컴파일러 기반 구조. |
| `ling-lex` | 2030.0.8 | 어휘 분석과 다국어 토큰화. |
| `ling-ast` | 2030.0.8 | 추상 구문 트리 타입. |
| `ling-polyglot` | 2030.0.8 | 16개 이상의 인간 언어로 키워드와 내장 함수 이름을 제공하는 어휘 시스템. |
| `ling-mir` | 2030.0.12 | 코드 생성기를 위한 중간 수준 중간 표현. |
| `ling-codegen` | 2030.0.13 | 코드 생성 백엔드(바이트코드, WASM, 네이티브). |
| `ling-macro` | 2030.0.8 | Ling 생태계의 절차적 매크로. |
| `ling-runtime` | 2030.0.8 | 런타임 지원: 가비지 컬렉터, 할당자, 표준 라이브러리. |
| `ling-wasm` | 2030.0.16 | `ling build --platform web`이 쓰는 WebGL2 WebAssembly 런타임. |

## 그래픽, UI, 입력

| 크레이트 | 버전 | 설명 |
|---|---|---|
| `ling-graphics` | 2030.0.13 | 3D/4D 렌더링, 기하, 애니메이션, 글꼴 도구. |
| `ling-gl` | 2030.0.10 | 투영된 장면을 위한 GPU 래스터라이저(wgpu: Vulkan, Metal, DX12; wasm에서는 WebGL2). |
| `ling-gpu` | 2030.0.10 | GPU 연산(cudarc를 통한 CUDA, CPU 대체 경로 포함). |
| `ling-ui` | 2030.0.9 | 보존 모드 위젯과 플렉스 레이아웃(`ui_*`). |
| `ling-animation` | 2030.0.8 | 통합 애니메이션 시스템 Anima: 하나의 리그로 어떤 대상이든. |
| `ling-input` | 2030.0.9 | Sensorium: 게임패드, 조이스틱, 햅틱, 모션, XR 컨트롤러, 터치 컨트롤. |
| `ling-paint` | 2030.0.1 | LingAnime 페인트 엔진: 레이어, 브러시, 벡터, 하프톤, 2.5D 리그, 펜 태블릿(`art_*`). |
| `ling-icon` | 2030.0.9 | SVG/PNG 아이콘을 다중 해상도 `.ico`로 래스터화해 Windows 실행 파일에 넣습니다(`ling build --icon`). |

## 게임과 물리

| 크레이트 | 버전 | 설명 |
|---|---|---|
| `ling-game` | 2030.1.9 | 게임 엔진: ECS, 물리, 장면 관리. |
| `ling-physics` | 2030.1.18 | 물리, 월드 생성, 쌍곡 기하, glTF 불러오기. |
| `ling-body` | 2030.0.2 | 음향 바디: 인체형 SDF 골격, FDTD 파동 솔버, 막, 메시와 WAV 코덱. |
| `ling-emu` | 2030.0.1 | 콘솔 에뮬레이션 호스트(`emu_*`): SNES와 N64 코어. |

## 오디오와 음악

| 크레이트 | 버전 | 설명 |
|---|---|---|
| `ling-audio` | 2030.0.13 | 4D 위치 기반 오디오 합성과 WAV 배경 음악. |
| `ling-music` | 2030.0.9 | 음악 디코딩, BPM과 조성 분석, 합성, 리듬 게임과 노래방 도구. |
| `ling-mic` | 2030.0.12 | 마이크 입력과 실시간 오디오 캡처. |
| `ling-daw` | 2030.0.2 | LingStationX의 DAW 엔진: 신시사이저, 드럼, 이펙트, 믹서, 시퀀서. |
| `ling-osdaw` | 2030.0.2 | LingOS용 LingStation: `no_std` 트래커 시퀀서와 신시사이저. |

## 네트워크, 웹, 보안

| 크레이트 | 버전 | 설명 |
|---|---|---|
| `ling-http` | 0.1.8 | axum 기반 HTTPS/JSON/SQLite 프레임워크. Ling의 `http_serve`가 이 위에서 동작합니다. [ling-http와 ling-web](web.md)을 참고하십시오. |
| `ling-web` | 0.1.9 | `ling-http` 위의 Flask 스타일 계층: 세션, 플래시 메시지, 템플릿, 정적 파일, 블루프린트, CSRF. |
| `ling-net` | 2030.0.9 | 비동기 네트워킹: HTTP, WebSocket, QUIC, P2P. |
| `ling-crypto` | 2030.1.13 | 양자 내성 및 고전 암호: ML-KEM-768, 하이브리드 KEM, AES-GCM, XChaCha20, BLAKE3, SHA-3, Argon2id, Shamir, Schnorr 영지식 증명, VRF. `no_std` 지원. |

## AI

| 크레이트 | 버전 | 설명 |
|---|---|---|
| `ling-ai` | 2030.0.10 | 텐서, 신경망, LLM 추론. |
| `ling-llm` | 2030.0.2 | 운영 체제 없이 CPU에서 동작하는 Qwen3용 `no_std` GGUF 추론 엔진(LingOS용). |
| `ling-diffusion` | 2030.0.1 | `sd_*` 내장 함수를 뒷받침하는 프로세스 내 Stable Diffusion(SD 1.x / SDXL). |

## LingOS

| 크레이트 | 버전 | 설명 |
|---|---|---|
| `ling-kernel` | 2030.1.37 | 최소 커널 런타임: `no_std` CPU 내장 기능, VGA, 시리얼, 부팅. |
| `ling-user` | 2030.1.4 | LingOS에서 Ling 프로그램을 위한 사용자 영역 런타임. |

## 바인딩

| 크레이트 | 버전 | 설명 |
|---|---|---|
| `ling-py` | 2030.0.8 | `ling-audio`, `ling-physics`, `ling-net`의 Python 바인딩. |

## 프로필별 크레이트

[설치 프로필](../getting-started/install.md)에 따라 `ling`에 어떤 크레이트가 컴파일되어 들어가는지
정해집니다.

| 프로필 | 크레이트 |
|---|---|
| `web` | `ling-http`, `ling-crypto`, `ling-icon`, `lingfu` |
| `privacy` | `web`에 더해 `ling-net`, `ling-ui` |
| `game` | `ling-physics`, `ling-gpu`, `ling-game`, `ling-animation`, `ling-input`, `ling-body`, `ling-ui`, `ling-icon` |
| `ai` | `ling-ai`, `ling-music` |
| `audio` | `ling-audio`, `ling-music`, `ling-mic`, `ling-daw` |
| `art`(`full`에 포함) | `ling-paint` |
