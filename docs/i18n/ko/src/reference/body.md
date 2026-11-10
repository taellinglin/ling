# 음향 바디

`body_*` 내장 함수는 소리로 사람 모양의 피부를 키워 내는 툴킷 `ling-body`를 다룹니다. 바디 골격(매개변수로 만드는 T 포즈 인체 또는 가져온 모델)을 복셀화해 부호 거리장으로 만듭니다. 바디에 놓은 음원이 그 안팎에 압력파를 일으키고, 얇은 탄성 막이 그 파동에 반응하며, 막의 표면을 물샐틈없는(watertight) 삼각형 메시로 뽑아냅니다.

이 레시피는 WAV 파일을 오갑니다. 내보낸 WAV는 소리로 재생되면서 매개변수와 메시도 함께 담고 있어, 다시 불러오면 같은 바디가 됩니다. 다른 오디오 파일도 분석해 음원 매개변수로 바꿀 수 있습니다.

알아 둘 것:

- 툴킷은 프로그램마다 하나이며, 모든 `body_*` 호출이 그것을 다룹니다.
- 실패할 수 있는 작업은 성공하면 `1`, 실패하면 `0`을 돌려주고, 메시지를 남깁니다. 메시지는 `body_message()`로 읽습니다.
- 값은 숫자, 문자열, 평평한 리스트로 오갑니다.
- 단위는 미터입니다. 메시는 Y축이 위이고, 인체는 +Z를 바라보며, 왼쪽이 +X입니다.
- `body_draw`와 `body_draw_creases`를 뺀 모든 것은 창 없이 실행됩니다.
- 이 내장 함수들에는 현지화된 이름이 없습니다.

## 첫 번째 바디

매개변수 몇 개를 정하고, 음원을 추가하고, 시뮬레이션을 조금씩 나눠 돌린 다음, 메시를 읽고 내보냅니다.

```ling
바인드 시작 = 실행 {
    body_param_set("sr", 8000)
    body_param_set("steps", 60)
    body_param_set("seal", 1)
    body_src_add(0.0, 1.0, 0.0, 0, 220)
    출력("단계: " + str(body_sim_begin()))

    바인드 진행 = [0, 1]
    동안 진행[0] < 진행[1] {
        바인드 진행 = body_sim_step(40)
    }
    출력("완료: " + str(body_sim_done()))
    출력("메시: " + str(body_mesh_update()) + " " + body_message())
    바인드 개수 = body_mesh_counts()
    출력("watertight: " + str(개수[2]))
    출력(body_grid_info())
    출력("내보냄: " + str(body_export_mesh("body.obj")) + " " + body_message())
}
```

```output
단계: 60
완료: 1
메시: 1 mesh updated
watertight: 1
109x107x75 @ 41.3 mm
내보냄: 1 10008 triangles → body.obj
```

`body_message()`의 메시지는 영어로 나옵니다.

## 요약

| 내장 함수 | 용도 |
|---|---|
| `body_message()` | 마지막 작업의 메시지 |
| `body_grid_info()` | `"109x107x75 @ 41.3 mm"` 같은 격자 크기와 간격 |
| `body_notes()` | 마지막 청취 패스나 WAV 분석에서 나온 메모 |
| `body_param_get(name)` / `body_param_set(name, v)` | 매개변수 읽기와 설정 |
| `body_param_reset()` | 모든 매개변수를 되돌리고 음원을 모두 제거 |
| `body_param_count()`, `body_param_name(i)`, `body_param_group(i)` | 매개변수 목록 |
| `body_param_min(i)`, `body_param_max(i)`, `body_param_default(i)` | 매개변수의 범위와 기본값 |
| `body_src_add(x, y, z, wave, freq)` | 위치에 음원 추가 |
| `body_src_at_joint(joint, wave, freq)` | 골격 관절에 음원 추가 |
| `body_src_count()`, `body_src_name(i)`, `body_src_del(i)`, `body_src_clear()` | 음원 관리 |
| `body_src_get(i, field)` / `body_src_set(i, field, v)` | 음원 필드 읽기와 변경 |
| `body_joint_count()`, `body_joint_name(i)`, `body_joint_pos(i)`, `body_joint_parent(i)` | 골격의 관절 |
| `body_model_load(path)`, `body_model_use(h)`, `body_model_active()` | 가져온 모델을 바디로 사용 |
| `body_model_part_count(h)`, `body_model_part_name(h, i)`, `body_model_part_tris(h, i)` | 모델 부품 살펴보기 |
| `body_model_part_enabled(h, i)`, `body_model_part_set(h, i, on)` | 부품 포함 또는 제외 |
| `body_preview()` | 시뮬레이션 없이 복셀화해 기본 형태 보기 |
| `body_sim_begin()`, `body_sim_step(ms)` | 시뮬레이션 시작과 진행 |
| `body_sim_active()`, `body_sim_done()` | 시뮬레이션 상태 |
| `body_mesh_update()` | 막을 작업 메시로 추출 |
| `body_displacement()`, `body_slice(axis, frac)`, `body_mic_tail(n)` | 시뮬레이션 살펴보기 |
| `body_mesh_counts()`, `body_mesh_bounds()` | 메시 통계 |
| `body_mesh_positions()`, `body_mesh_normals()`, `body_mesh_indices()` | 메시 원본 데이터 |
| `body_crease_detect()`, `body_crease_apply()`, `body_detail_reset()` | 음장에서 주름 새기기 |
| `body_crease_count()`, `body_crease_segments()`, `body_crease_clear()`, `body_crease_export(path)` | 주름 곡선 다루기 |
| `body_listen_begin()`, `body_listen_step(ms)`, `body_listen_done()`, `body_listen_finish(path)` | 바디의 음향 특징 녹음 |
| `body_listen_spectrum()` | 음향 특징의 로그 간격 48개 대역 |
| `body_export_mesh(path)`, `body_export_wav(path)`, `body_import_wav(path, analyze_only)` | 파일 |
| `body_project_save(path)`, `body_project_load(path)` | 레시피를 JSON으로 저장하고 불러오기 |
| `body_draw(mode, yaw, x, y, z, scale, extra)`, `body_draw_creases(yaw, x, y, z, scale)` | 창에 그리기 |

## 매개변수

매개변수는 이름이 붙은 숫자 47개로 된 고정 표입니다. 각각 그룹, 기본값, 범위가 있으며, 값을 설정하면 범위 안으로 잘립니다. `body_param_set`은 아는 이름이면 `1`, 아니면 `0`을 돌려줍니다. 목록용 내장 함수는 0부터 46까지의 인덱스나 이름을 받습니다.

| 그룹 | 이름(기본값) | 의미 |
|---|---|---|
| body | `height` (1.7), `build` (1), `arm_span` (1), `head_scale` (1) | 골격 비율. `height`는 가져온 모델의 크기도 바꿉니다 |
| body | `seal` (3) | 가져온 모델의 구멍을 막을 때 쓰는 복셀 반지름 |
| sim | `sr` (22050), `steps` (800), `margin` (16), `up` (2) | 샘플링 레이트, 시간 단계, 셀 단위 공기 여백, 막 격자 세분화 |
| medium | `c_air` (343), `c_body` (1), `rho_body` (4), `damping` (0.002), `sponge` (8) | 음속, 바디의 속도비와 밀도비, 손실, 흡음층 |
| mic | `mic_sep` (0.18), `mic_dist` (0.15) | 귀 간격과 얼굴 앞 거리 |
| membrane | `relief`, `coupling`, `threshold`, `tension`, `stiffness`, `mem_time`, `mem_every`, `reinit_every`, `band`, `drive_mode`, `drive_tau` | 피부가 파동을 따르는 방식 |
| mesh | `smooth` (3), `decimate` (0), `iso` (0) | 스무딩 횟수, 정점 병합 셀 크기(미터), 표면 레벨 오프셋 |
| audio | `wav_bits` (16), `wav_gain` (0.9), `wav_repeat` (4), `wav_gap` (0.25) | WAV 내보내기 |
| detail | `crease_mode`, `crease_levels`, `crease_tol`, `crease_min`, `detail_level`, `crease_depth`, `crease_width`, `noise_amp`, `noise_freq` | 주름 검출과 새기기 |
| listen | `sig_dur`, `sig_sr`, `sig_rho`, `sig_excite` | 청취 패스 |

```ling
바인드 시작 = 실행 {
    출력(body_param_count())
    출력(body_param_get("height"))
    출력(body_param_set("height", 5))
    출력(body_param_get("height"))
    출력(body_param_set("nope", 1))
    출력(body_joint_count())
    출력(body_joint_name(4))
}
```

```output
47
1.7
1
2.4
0
21
head
```

`height`는 2.4까지만 올라가므로, 5를 넣으면 2.4가 저장됩니다.

## 음원과 관절

음원은 공간에 놓인 가진원입니다. 필드는 `x`, `y`, `z`(미터), `wave`(`0` 리커 웨이블릿, `1` 사인 버스트, `2` 임펄스), `freq`(Hz), `amp`, `phase`, `start`, `dur`(초)입니다.

- `body_src_add(x, y, z, wave, freq)`는 음원을 하나 추가하고(기본값 0, 1, 0, wave 0, 220 Hz) 그 인덱스를 돌려줍니다.
- `body_src_at_joint(joint, wave, freq)`는 이름이 붙은 관절에 음원을 추가하고, 음원 이름을 관절 이름으로 붙입니다. 모르는 관절이면 `-1`을 돌려줍니다.

골격에는 관절이 21개 있습니다. `hips`, `spine`, `chest`, `neck`, `head`, 그리고 `shoulder`, `elbow`, `wrist`, `hand`, `hip`, `knee`, `ankle`, `toe`의 `l_` / `r_` 버전입니다. `body_joint_pos(i)`는 현재 바디 크기에 맞춘 `[x, y, z]`를, `body_joint_parent(i)`는 부모 인덱스(루트는 `-1`)를 돌려줍니다.

## 가져온 모델

`body_model_load(path)`는 `.glb`, `.gltf`, `.lmesh`, `.obj` 파일을 불러와 핸들(또는 `-1`)을 돌려줍니다. 불러오기만 해서는 선택되지 않습니다. `body_model_use(h)`를 부르고, 골격으로 돌아가려면 음수 핸들을 넘기세요. glTF 파일은 지오메트리와 노드 변환만 읽으므로, 텍스처 확장이 필요한 파일도 불러올 수 있습니다. 꽃이나 상자 같은 장식 부품은 꺼진 상태로 시작합니다. 부품은 `body_model_part_set(h, i, on)`으로 켜고 끕니다.

## 시뮬레이션

시뮬레이션은 시간을 정해 조금씩 돌기 때문에, 창은 계속 갱신될 수 있습니다.

- `body_preview()`는 바디를 복셀화해 흔들리지 않은 모양을 시뮬레이션 없이 뽑아냅니다.
- `body_sim_begin()`은 시뮬레이션을 시작하고 전체 단계 수를 돌려줍니다. 실패하면 `-1`입니다.
- `body_sim_step(ms)`는 약 `ms`밀리초(기본값 30) 동안 일하고 `[완료한_단계, 전체]`를 돌려줍니다.
- `body_mesh_update()`는 지금 상태의 막을 뽑아냅니다. 시뮬레이션이 도는 중에 불러 피부가 자라는 모습을 볼 수 있습니다.
- `body_displacement()`는 `[안쪽_mm, 바깥쪽_mm]`을 돌려줍니다. `body_slice(axis, frac)`는 -1..1로 정규화한 압력장 단면 `[w, h, v0, v1, ...]`를 돌려줍니다. `body_mic_tail(n)`은 첫 번째 마이크의 마지막 샘플 `n`개를 돌려줍니다.

## 주름과 디테일

시뮬레이션이 끝난 뒤 `body_crease_detect()`는 메시 위에서 음장을 샘플링해 등고선을 따라가고, 3차 베지어 곡선으로 맞춥니다. `body_crease_apply()`는 메시를 세분화하고 곡선마다 부드러운 홈을 새기며, 음장으로 가중한 미세한 노이즈도 더합니다. `body_detail_reset()`은 매끈한 메시로 되돌립니다. `body_crease_export(path)`는 곡선을 `.json`으로, 또는 곡선 오브젝트를 만드는 Blender용 `.py` 스크립트로 씁니다.

## 청취

`body_listen_begin()`은 입 앞에서 임펄스를 쏘고 머리 옆의 마이크 두 개로 녹음합니다. 끝날 때까지 `body_listen_step(ms)`를 부르고(`[완료, 전체]`를 돌려줌), 그다음 `body_listen_finish(path)`가 바디의 소리를 WAV로 쓰고 `body_listen_spectrum()`과 `body_notes()`(가장 강한 공명)를 채웁니다.

## 파일

- `body_export_mesh(path)`는 확장자에 따라 OBJ, STL, GLB를 씁니다. GLB에는 T 포즈 스켈레톤과 스킨 가중치가 들어갑니다.
- `body_export_wav(path)`는 스테레오 마이크 신호를 쓰고, 전체 레시피와 압축한 메시를 담은 추가 청크 두 개를 붙입니다.
- `body_import_wav(path, analyze_only)`는 두 청크가 있으면 되살립니다. `analyze_only`가 1이거나 청크가 없으면 대신 오디오에서 음원을 추정하며, `body_notes()`가 각 추정을 설명합니다. WAV, FLAC, Ogg, MP3를 받습니다.
- `body_project_save(path)`와 `body_project_load(path)`는 매개변수, 음원, 선택한 바디를 JSON으로 보관합니다.

## 그리기

`body_draw(mode, yaw, x, y, z, scale, extra)`는 작업 메시를 펜 색으로 그리며, 렌더러의 y-아래 공간으로 뒤집습니다. 모드:

| 모드 | 모양 |
|---|---|
| 0 | 솔리드, 스무딩된 법선 |
| 1 | `extra`번째마다 삼각형을 그리는 와이어프레임 |
| 2 | 피부 변위로 물들임. `extra`는 최대 밀림(밀리미터, 기본값 10) |
| 3 | 표면 법선으로 색칠 |
| 4 | 조명 하나의 점토 셰이딩 |

모드 3과 4는 미세한 요철을 보기 쉽게 합니다. `body_draw_creases(yaw, x, y, z, scale)`는 검출한 주름 곡선을 위에 그립니다.

이 창은 바디가 자라는 모습을 보여 줍니다.

```ling-gui
바인드 시작 = 실행 {
    창열기(640, 480, "바디")
    body_param_set("steps", 400)
    body_src_at_joint("chest", 1, 180)
    body_sim_begin()
    바인드 방향 = 0.0
    동안 창열림() {
        만약 body_sim_done() == 0 {
            body_sim_step(20)
            body_mesh_update()
        }
        채우기(16, 16, 24)
        색설정(220, 200, 180)
        body_draw(2, 방향, 0, 1, 4, 1, 10)
        flush_3d()
        표시()
        바인드 방향 = 방향 + 0.01
    }
}
```

함께 보기: [물리](physics.md), [음악](music.md).
