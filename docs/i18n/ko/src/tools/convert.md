# 에셋 변환

`ling convert`는 외부 에셋(3D 모델, 소리, MIDI 곡, SVG 그림)을 독립된 `.ling` 파일로 바꿉니다.
프로그램에서 그 파일을 `사용`으로 가져오면 Ling의 내장 함수로 에셋을 다시 만들어 냅니다. 실행할
때 로더나 원본 파일이 필요 없습니다.

```sh
ling convert model.glb                  # model.ling 작성 (압축, 무손실)
ling convert sfx.wav -o sfx.ling        # 출력 경로 지정
ling convert logo.svg --no-compression  # 읽고 비교하기 쉬운 일반 배열
lingfu convert song.mid                 # lingfu는 ling convert로 넘깁니다
```

| 옵션 | 의미 |
|---|---|
| `-o <out.ling>`(또는 `--out`) | 출력 경로. 기본값은 입력 파일 이름에 `.ling`을 붙인 것입니다. |
| `--no-compression` | 숫자를 압축 블롭 대신 일반 리스트로 씁니다. |

## 형식별 결과

| 입력 | 생성되는 바인딩 | 프로필 |
|---|---|---|
| `.gltf`, `.glb` | 메시마다 `<name>_<mesh>_pos`, `_nrm`, `_uv`, `_idx`와 와이어프레임 함수 `draw_<name>_<mesh>(ox, oy, oz, scale)`. `draw_<name>(ox, oy, oz, scale)`은 모델 전체를 그립니다. 노드 이름과 변환은 주석으로 남습니다. | `game` |
| `.wav`, `.ogg`, `.flac`, `.mp3` | `<name>_pcm`(모노 샘플), `<name>_rate`, `<name>_dur` | `audio` |
| `.mid`, `.midi` | `<name>_notes`(음표마다 `[time, dur, midi, vel, channel]`이 이어진 평평한 리스트)와 `<name>_dur` | `audio` |
| `.svg` | `<name>_xy`(점), `<name>_lens`(폴리라인마다 점 개수), `draw_<name>(ox, oy, scale)` | 모두 |
| `.blend` | Blender로 glTF로 내보낸 뒤 `.glb`처럼 변환합니다. Blender가 `PATH`에 있거나 `$BLENDER`가 설정되어 있어야 합니다. | `game` |

`<name>`은 입력 파일 이름을 올바른 식별자로 다듬은 것입니다. SVG에서는 `<path>` 데이터(`M`,
`L`, `H`, `V`, `C`, `Q`, `Z`)와 `<line>`, `<rect>`, `<polyline>`, `<polygon>` 요소가
폴리라인으로 바뀝니다. 그 밖의 확장자는 `unsupported extension`으로 실패합니다.

## 압축과 일반 형식

기본적으로 긴 숫자 리스트는 zlib으로 압축하고 base64로 인코딩한 텍스트로 손실 없이 저장되며, 두
내장 함수로 풀어냅니다.

- `blob_f32("...")`는 숫자 리스트(리틀 엔디언 32비트 부동소수점)로 풀립니다.
- `blob_i32("...")`는 정수 리스트(32비트 정수)로 풀리며, 인덱스와 길이에 쓰입니다.

`--no-compression`은 같은 숫자를 리터럴 리스트 `[ ... ]`로 씁니다. 파일은 커지지만 읽기
쉽습니다. 숫자가 여덟 개 이하인 리스트는 항상 일반 형식으로 씁니다.

## 예: SVG

`<rect>` 하나와 `<path>` 하나가 든 작은 `logo.svg`가 있다고 합시다.

```text
$ ling convert logo.svg
[convert] logo.svg → logo.ling  (1 KB, deflate+base64 lossless)
```

생성된 `logo.ling`은 이렇게 시작합니다.

```ling-module
# 2 polylines, 7 points
bind logo_xy = blob_f32("eNpdibENAAAIgzjFV/RmDzTRwaYDAwCRLF2Huvf/A/L0CM0=")
bind logo_lens = [2, 5]
```

그 뒤에 `draw_logo(ox, oy, scale)` 함수가 이어집니다. 프로그램에서는 이렇게 씁니다.

```ling-gui
사용 "logo.ling"

바인드 시작 = 실행 {
    창열기(400, 400, "로고")
    동안 창열림() {
        채우기(0, 0, 0)
        색설정(255, 255, 255)
        draw_logo(100, 100, 2.0)
        표시()
    }
}
```

## 참고

- 오디오는 원본 샘플링 속도의 모노 다운믹스로 저장됩니다. 생성된 파일에는 샘플만 들어 있으며,
  재생하거나 분석하는 일은 프로그램이 맡습니다.
- 생성된 코드는 함수에 태국어 키워드(`ฟังก์ชัน`)를, 바인딩에는 `bind`를 씁니다. 정상입니다.
  모든 언어는 어느 파일에서나 동작합니다.
- 모델의 그리기 함수는 `draw_line_3d`로 와이어프레임을 그립니다. 채워지고 텍스처가 입혀진
  메시가 필요하면 [3D 도형과 메시](../reference/shapes.md)를 참고하십시오.
