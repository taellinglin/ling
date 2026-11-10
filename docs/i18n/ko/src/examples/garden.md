# 정원

소스: [`examples/audiovisual/Garden.ling`](https://github.com/taellinglin/ling/blob/master/examples/audiovisual/Garden.ling)

탁 트인 야외 정원입니다. 땅과 하늘, 가운데의 연못, 앞쪽의 나무 두 그루, 꽃밭, 뒤쪽의 나무숲, 나무 헛간이 있고 벽이나
천장은 없습니다. 모두 벡터 지오메트리(`vtex_*`)로 그리며 도형 종류마다 고유한 음이 있어서, 보기만 하는 게 아니라 듣는
장면이기도 합니다.

저장소 루트에서 실행합니다.

```sh
ling run examples/audiovisual/Garden.ling
```

## 장면

| 부분 | 파일 안의 함수 |
|---|---|
| 잔디, 돌길, 꽃밭 구역 | `draw_ground` |
| 해, 흘러가는 구름, 지평선의 빛 | `draw_sky` |
| 나선형 줄기와 꽃 수관을 가진 나무 | `draw_tree`, `draw_trees` |
| 여기저기 흩어진 꽃밭 | `draw_flowers` |
| 연꽃이 떠 있고 분수와 물결이 있는 연못 | `draw_pond` |
| 문, 창문, 지붕이 있는 나무 헛간 | `draw_shed` |
| 이전 프레임에서 만든 희미한 "잔상" 두 개 | `draw_echo` |

도형은 `vtex_flower`, `vtex_lotus`, `vtex_chakra`, `vtex_rings`, `vtex_grid`, `vtex_spiral`, `vtex_star`,
`vtex_yantra`, `vtex_hyperbolic_uv`, `vtex_tessellated`로 그립니다. 작은 해시 함수 `sm2_hash` 덕분에 땅 구역과
헛간 판자마다 세부 묘사의 양이 다릅니다.

## 소리

톤 성부 여섯 개가 G장조 오음 음계(G A B D E)를 씁니다. 각 성부는 `audio_tone`으로 장면 속 자기 위치(연못, 헛간,
하늘 주변)에 놓이고, `audio_listener`가 카메라를 따라갑니다.

## 카메라

카메라는 연못 주위를 느리고 자유로운 궤도로 떠다니며 늘 정원 가운데를 바라봅니다. 색은 약 2초마다 전체 색상환을
한 바퀴 돕니다.

## 해 볼 만한 것

- `orbit_h`와 `orbit_v`를 바꿔 카메라를 더 가깝게 또는 멀리 두어 보세요.
- `PX`와 `PZ`로 연못을, `SX`와 `SZ`로 헛간을 옮겨 보세요.
- 오음 음계 주파수 `PG3`부터 `PE4`까지 바꿔 정원의 조를 바꿔 보세요.
