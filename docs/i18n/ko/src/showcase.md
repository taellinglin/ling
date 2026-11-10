# 쇼케이스

여기 소개하는 앱과 데모는 모두 Ling으로 작성되었고 Ling 런타임에서 실행됩니다. 앱 스크린샷은 웹사이트에서 볼 수
있습니다: [ling-lang.org/#apps](https://ling-lang.org/#apps).

## 앱

| 앱 | 소개 |
|---|---|
| **LingStationX** | 디지털 오디오 워크스테이션. 어레인저, 피아노 롤, 트래커, 세션 뷰, 노드 그래프 패처, 믹서, 그리고 Nova, Prism, Growl 신시사이저가 있습니다. MIDI 가져오기, 슬라이서, 타임 워프, 마스터링 체인도 지원합니다. |
| **LingDev** | 노드 트리, 타임라인, 카메라·대화·효과용 컷신 트랙을 갖춘 3D 게임 편집기. Dance Ling, Ling Hazard, Ling Breaker, Moko, Ling Agent, Soul Symphony 시리즈가 이것으로 만들어졌습니다. |
| **LingModeler** | 모델링, 리깅, 웨이트 페인팅, 포즈, 애니메이션에 더해 벡터 일러스트, 어니언 스킨을 쓰는 2D 애니메이션, 절차적 텍스처까지. x86-64와 Raspberry Pi의 LingOS 안에서도 실행됩니다. |
| **LingForFun** | 필드, 마을, 던전, 비행, 전투, 퀘스트가 있는 온라인 액션 RPG. 데스크톱 클라이언트, WASM 브라우저 클라이언트, 서버 클러스터를 모두 Ling으로 작성했으며 중국어 키워드를 씁니다. 개발 중입니다. |
| **Ling Arcade** | Ling으로 작성한 NES 에뮬레이터가 들어 있는 콘솔 게임 라이브러리. 6502 CPU, PPU, 매퍼, APU가 모두 Ling의 JIT에서 돌아가는 평범한 `.ling` 코드입니다. nestest 로그 전체를 통과합니다(명령어 8,991개, 사이클 단위로 정확). |
| **Ling Wallet** | LNG용 양자 내성 프라이버시 지갑이자 풀 노드와 채굴기. ML-DSA-87 신원과 ML-KEM-1024 + X25519 하이브리드 키 교환을 씁니다. 실험적이며 감사를 받지 않았습니다. |

## 커뮤니티 프로젝트

| 프로젝트 | 소개 |
|---|---|
| [**Soul Symphony**](https://github.com/taellinglin/soul-symphony-ling-edition) | RPG 아케이드 물리 게임. 구슬을 굴려 절차적으로 만든 9개 레벨을 지나고, 레벨마다 끝에서 보스와 싸웁니다. 아케이드 대전, 네트워크, 협동 플레이를 지원하며 5개 언어로 즐길 수 있습니다. |
| [**LingSoul**](https://github.com/taellinglin/LingSoul) | Ling의 와이어프레임 스타일로 그린 분위기 있는 영혼 균형 시뮬레이션. 게임 속 HUD에는 직접 만든 문자 체계를 씁니다. |

## 저장소의 데모

[Ling 저장소](https://github.com/taellinglin/ling)에는 바로 실행할 수 있는 예제가 많습니다. 먼저 이것들을 해 보세요.

| 데모 | 보여 주는 것 |
|---|---|
| `examples/music/hallway_runner.ling` | 음악에 반응하는 무한 러너. 동전이 공간 사운드로 멜로디를 연주합니다 |
| `examples/music/karaoke_hero.ling` | 보컬 MIDI로 만든 노래방 음정 막대와 실시간 마이크 튜너 |
| `examples/music/synth_jam.ling` | `.ling` 패치로 합성하는 GM 악기 |
| `examples/physics/bouncy_ball.ling` | 튈 때마다 찌그러지는 스프링-질량 소프트 바디 |
| `examples/physics/tumbling.ling` | 구르고 뒹구는 강체 |
| `examples/physics/liquid_marble.ling` | 회전하는 구, 원뿔, 돔 위의 물과 기름 시뮬레이션 |
| `examples/ui/hud_showcase.ling` | UI 툴킷: HUD, 미터, 컨트롤, 게임 위젯 |
| `examples/crypto/crypto_donut_physics.ling` | SHA3 해시가 모양을 정하는 통통 튀는 토러스 매듭과 반응하는 오디오 |

저장소 루트에서 하나를 실행해 보세요.

```sh
ling run examples/physics/liquid_marble.ling
```

이 문서의 [예제](examples/dao-chamber.md) 부분에서 3D 장면 네 개를 더 자세히 살펴봅니다.
