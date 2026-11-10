# 도장 — 신전 방

소스: [`examples/3d-rooms/ling-dao-chamber.ling`](https://github.com/taellinglin/ling/blob/master/examples/3d-rooms/ling-dao-chamber.ling)

벡터 지오메트리(`vtex_*`)만으로 그린 고요한 중국·일본풍 신전입니다. 픽셀 채우기는 전혀 쓰지 않습니다. 모든 것이 금색,
비취색, 진홍색으로 천천히 움직이고, 배경에서는 오음 음계의 종소리가 부드럽게 울립니다. 프로그램은 주로 태국어 키워드와
내장 함수로 쓰였고, 중국어 이름도 몇 개 섞여 있습니다.

저장소 루트에서 실행합니다.

```sh
ling run examples/3d-rooms/ling-dao-chamber.ling
```

## 방에 있는 것

| 부분 | 사용한 내장 함수 |
|---|---|
| 안쪽 끝의 도리이 | `vtex_torii` |
| 도리이 뒤의 오층탑 | `vtex_pagoda` |
| 제단 만다라: 톱니 고리, 법륜, 연꽃, 고리 | `vtex_spiked_cog`, `vtex_chakra`, `vtex_lotus`, `vtex_rings` |
| 얀트라 테두리가 있는 돌 타일 바닥 | `vtex_grid`, `vtex_yantra` |
| 천장의 별 모양 등롱 | `vtex_star`, `vtex_rings` |
| 양쪽 벽의 수묵 패널 | `vtex_letter_rain`, `vtex_lotus` |
| 네 모서리의 기둥 | `vtex_rings`, `vtex_spiked_cog`, `vtex_lotus` |

## 소리

다섯 음이 중국 오음 음계 宫 商 角 徵 羽(C4, D4, E4, G4, A4)를 이루고, 희미한 저음이 하나 더 있습니다. 각 음은
`audio_tone`으로 3D 공간에 놓이고 `audio_listener`가 카메라를 따라가므로, 시점이 흔들리면 종소리도 함께 움직입니다.

## 카메라와 조명

카메라는 느린 진자처럼 양쪽으로 약 30도씩 흔들리며 늘 제단을 바라봅니다. 조명 다섯 개(`add_light`)가 천장의 금색 등,
제단의 따뜻한 빛, 기둥의 비취색과 진홍색 포인트, 도리이의 등을 만듭니다.

## 해 볼 만한 것

- 파일 맨 위의 색상 기준값 `GOLD`, `JADE`, `CRIMSON`을 바꿔 보세요.
- `orbit_spd`를 키워 카메라를 더 빠르게 움직여 보세요.
- `init_audio`의 주파수를 바꿔 다른 음계를 써 보세요.

여기서 쓴 내장 함수는 [3D 도형과 메시](../reference/shapes.md)와 [오디오](../reference/audio.md)를 보세요.
