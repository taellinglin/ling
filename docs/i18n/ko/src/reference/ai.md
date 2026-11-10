# AI와 언어 모델

Ling의 기계 지능은 세 가지 크기로 제공됩니다. 모두 여러분의 컴퓨터에서 실행되며, 호스팅된
서비스로 보내는 것은 없습니다.

| 계열 | 하는 일 | 필요한 것 |
|---|---|---|
| `nn_*` | 루프에서 학습시키는 작은 순전파 신경망 | `ai` 프로필 |
| `bt_*` | 게임 캐릭터용 행동 트리 | `ai` 프로필 |
| `dialog_*`(학습) | 캐릭터의 말투를 배우는 작은 언어 모델 | `ai` 프로필 |
| `stt_*` | Whisper를 이용한 음성 인식 | `ai` 프로필과 Whisper 모델 |
| `llm_*` | 로컬 GGUF 모델(Qwen3, GLM-4)과 대화 | 선택 기능 `llm`과 모델 파일 |
| `vision_*` | 이미지에 대한 질문(Moondream2) | 선택 기능 `vision`과 모델 파일 |

기본 설치(`full`)에는 `ai` 프로필이 포함됩니다. `llm`과 `vision`은 `full`에 들어 있지 않으므로
명시적으로 설치하십시오. 예: `cargo install ling-lang --features llm`(NVIDIA GPU라면 `llm-cuda`).
이 기능이 없는 빌드에서는 `llm_*`와 `vision_*` 호출이 `()`로 평가됩니다.

다른 곳에서 이름을 보더라도 존재하지 않는 것이 두 가지 있습니다. `ai_chat`이나 `ai_embed` 내장
함수는 없고, 호스팅된 모델 API용 클라이언트도 없습니다. 원격 모델을 호출하려면
[웹 서버와 HTTP](../reference/web.md)의 `http_post_async`를 쓰고 응답은 직접 해석하십시오.

## 규칙

- **핸들은 숫자입니다.** `nn_new`, `bt_build`, `dialog_new`, `llm_load`, `vision_load`는 작은 숫자를
  반환합니다. 불러오기에 실패하면 `-1`을 반환합니다. 다른 함수에 잘못된 핸들을 주면 중립적인 값
  (`[]`, `0`, `""` 또는 `false`)을 반환하며 절대 멈추지 않습니다.
- **숫자 리스트**(예: `[0.5, 1]`)는 내부에서 32비트 부동소수점으로 변환됩니다. 숫자가 아닌 값은
  건너뜁니다.
- **긴 작업은 폴링합니다.** `stt_start`, `llm_generate_start`, `vision_analyze_start`는 작업 ID를 바로
  반환하고 백그라운드 스레드에서 일합니다. `sleep(ms)`를 넣은 루프에서 확인하십시오.

## 신경망: `nn_*`

밀집 순전파 신경망입니다. 입력 개수를 정하고, 층을 추가한 뒤, 예제마다 `nn_train`을 한 번씩
호출합니다. 학습은 제곱 오차 손실을 쓰는 단순한 확률적 경사 하강법으로, 매 프레임 돌려도 될
만큼 가볍습니다. `nn_new`에 주는 시드 덕분에 결과를 재현할 수 있습니다.

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `nn_new(n_inputs, seed)` | `建神经网` | `ニューラル作成` | `신경망생성` | `สร้างโครงข่าย` | 숫자 `n_inputs`개를 받는 빈 신경망 |
| `nn_dense(net, units, activation)` | `密集层` | `密層追加` | `밀집층` | `ชั้นหนาแน่น` | 완전 연결층 추가 |
| `nn_forward(net, inputs)` | `神经前向` | `順伝播` | `순전파` | `ส่งต่อโครงข่าย` | 신경망 실행. 리스트를 반환 |
| `nn_train(net, inputs, targets, rate)` | `训练网` | `ニューラル学習` | `신경망학습` | `ฝึกโครงข่าย` | 학습 한 단계. 손실을 반환 |
| `nn_save(net, path)` | `保存网` | `網保存` | `신경망저장` | `บันทึกโครงข่าย` | `LNN1` 형식으로 저장 |
| `nn_load(path)` | `载入网` | `網読込` | `신경망불러오기` | `โหลดโครงข่าย` | 저장된 신경망 불러오기, 실패하면 `-1` |

### 만들기와 학습

- `nn_new(n_inputs, seed)`: 둘 다 기본값은 1입니다.
- `nn_dense(net, units, activation)`: 활성화 함수는 `"relu"`(기본값), `"tanh"`, `"sigmoid"`이며,
  그 밖의 값은 선형입니다. 마지막 층의 너비가 출력 크기가 됩니다.
- `nn_forward(net, inputs)`: 짧은 입력 리스트는 0으로 채우고 긴 리스트는 자릅니다.
- `nn_train(net, inputs, targets, rate)`: `rate` 기본값은 0.01입니다. 갱신 전의 손실을 반환합니다.
  데이터 전체를 여러 회차 반복하십시오.

이 신경망은 XOR을 배웁니다.

```ling
바인드 시작 = 실행 {
    바인드 망 = 신경망생성(2, 7)
    밀집층(망, 8, "tanh")
    밀집층(망, 1, "sigmoid")
    위해 회차 안에 0..2000 {
        신경망학습(망, [0, 0], [0], 0.5)
        신경망학습(망, [0, 1], [1], 0.5)
        신경망학습(망, [1, 0], [1], 0.5)
        신경망학습(망, [1, 1], [0], 0.5)
    }
    출력(순전파(망, [0, 1])[0] > 0.9)
    출력(순전파(망, [1, 1])[0] < 0.1)
}
```

```output
true
true
```

### 저장

`nn_save(net, path)`는 작은 리틀 엔디언 파일(기본값 `"model.lnn"`)을 쓰고 `true`를 반환합니다.
`nn_load(path)`는 새 핸들을 반환하며, 파일이 없거나 올바르지 않으면 `-1`을 반환합니다. 이 형식에는
체크섬도 서명도 없으므로 신뢰할 수 없는 곳의 `.lnn` 파일은 불러오지 마십시오.

## 행동 트리: `bt_*`

행동 트리는 게임 캐릭터의 의사 결정 계층입니다. 작은 텍스트 언어로 작성하고, 숫자 사실(체력,
거리, 탄약)을 담은 **블랙보드**에 대해 틱합니다. 틱 한 번은 선택된 행동의 이름을 반환하며,
그것을 어떻게 수행할지는 여러분의 프로그램이 정합니다.

```text
selector {                  # 하나가 성공할 때까지 자식을 차례로 시도
    sequence {              # 모든 자식이 성공해야 함
        ?enemy_visible > 0  # 블랙보드 사실에 대한 조건
        ?ammo >= 1
        !attack             # 행동
    }
    !patrol                 # 대체 행동
}
```

복합 노드는 `selector`, `sequence`, `parallel`(모든 자식이 성공하면 성공), `not <child>`입니다.
조건은 `?key OP value` 형태이며 `>`, `<`, `>=`, `<=`, `==`, `!=`를 씁니다. 한 번도 설정하지 않은
키는 `0`으로 읽힙니다. 주석은 `#`로 시작합니다. 키와 행동 이름에는 한국어도 쓸 수 있습니다.

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `bt_build(spec)` | `建行为树` | `行動木構築` | `행동트리구성` | `สร้างต้นไม้พฤติกรรม` | 트리 컴파일, 문법 오류면 `-1` |
| `bt_set(tree, key, value)` | `设事实` | `事実設定` | `사실설정` | `ตั้งข้อเท็จจริง` | 블랙보드 사실 설정 |
| `bt_tick(tree)` | `行为树滴答` | `行動木更新` | `행동트리틱` | `เดินต้นไม้พฤติกรรม` | 한 번 실행. 행동 이름 또는 `""` 반환 |
| `bt_status(tree)` | `行为树状态` | `行動木状態` | `행동트리상태` | `สถานะต้นไม้พฤติกรรม` | 마지막 결과: `0` 실패, `1` 성공, `2` 실행 중 |

```ling
바인드 시작 = 실행 {
    바인드 두뇌 = 행동트리구성("selector { sequence { ?적_보임 > 0 ?탄약 >= 1 !공격 } sequence { ?적_보임 > 0 !도망 } !순찰 }")
    출력(행동트리틱(두뇌))
    사실설정(두뇌, "적_보임", 1)
    사실설정(두뇌, "탄약", 5)
    출력(행동트리틱(두뇌))
    사실설정(두뇌, "탄약", 0)
    출력(행동트리틱(두뇌))
}
```

```output
순찰
공격
도망
```

## 대화 학습: `dialog_*`

이 내장 함수들은 캐릭터에게 아주 작은 신경망 언어 모델을 줍니다. 학습한 대사의 **리듬과
어휘**를 배워 그 말투로 새 대사를 씁니다. 크기가 아주 작아서(킬로바이트 단위, CPU 코어 하나)
말뭉치가 작으면 구절을 그대로 반복합니다. `dialog_show` 계열의 대화 *상자*는 게임 프로필의 별개
기능입니다.

`dialog_learn`으로 한 줄씩 넣거나, Ling Dialog 형식(`.lingdialog`) 파일을 통째로 넣으십시오. 이
형식에서 `= label`은 대화를 시작하고 `speaker: text`는 발화 하나입니다.

```text
= welcome
bram: 어서 오게 나그네 신발 좀 말리게
hero: 길에서 들은 소식 있어요
bram: 북쪽 고개에 산적이 나온다네
```

학습에는 각 발화의 텍스트만 쓰입니다. 텍스트는 소문자로 바뀌고 단어로 나뉩니다.

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `dialog_new(context, embed, hidden, seed)` | `建对话模型` | `対話モデル作成` | `대화모델생성` | `สร้างโมเดลสนทนา` | 학습 전 모델(기본값 3, 32, 64, 1) |
| `dialog_learn(model, text)` | `对话学习` | `対話学習` | `대화학습` | `เรียนรู้สนทนา` | 학습 텍스트 한 줄 추가 |
| `dialog_load(model, path)` | `对话载入` | `対話読込` | `대화불러오기` | `โหลดชุดสนทนา` | `.lingdialog` 파일 추가. 추가된 대화 수 반환 |
| `dialog_train(model, epochs, rate)` | `对话训练` | `対話訓練` | `대화훈련` | `ฝึกสนทนา` | 학습(기본값 20, 0.1). 마지막 손실 반환 |
| `dialog_say(model, prompt, max_tokens, temperature)` | `对话生成` | `対話生成` | `대화생성` | `พูดสนทนา` | 대사 생성(기본값 24, 0.8) |
| `dialog_save(model, path)` | `对话存模` | `対話モデル保存` | `대화모델저장` | `บันทึกโมเดลสนทนา` | 학습된 모델 저장 |
| `dialog_load_model(path)` | `对话载模` | `対話モデル読込` | `대화모델불러오기` | `โหลดโมเดลสนทนา` | 저장된 모델 불러오기, 실패하면 `-1` |

`context`는 모델이 앞의 단어를 몇 개 보는지입니다. `temperature`가 `0` 이하면 항상 가장 가능성
높은 단어를 고르고, 값이 높을수록 다양해집니다. 시드, 말뭉치, 인수가 같으면 매번 같은 결과가
나옵니다. 학습 후에 텍스트를 추가했다면 다시 학습하십시오.

```ling
바인드 시작 = 실행 {
    바인드 상인 = 대화모델생성(3, 16, 32, 1)
    대화학습(상인, "어서 오게 나그네 불가에서 신발을 말리게")
    대화학습(상인, "칼은 날카롭게 지갑은 꼭 쥐고 다니게 나그네")
    출력(대화훈련(상인, 40, 0.1) < 2)
    출력(길이(대화생성(상인, "어서 오게 나그네", 8, 0.7)) > 0)
}
```

```output
true
true
```

## 음성 인식: `stt_*`

`stt_*`는 Whisper로 오디오 파일을 로컬에서 받아 적습니다. `llm` 기능이 있고 모델이 설치되어
있으면 내장 Whisper 엔진을 쓰고, 그렇지 않으면 Python에서
[faster-whisper](https://github.com/SYSTRAN/faster-whisper)를 실행합니다. Python 경로는 한 번만
설정하면 됩니다.

```sh
python -m venv venv
venv/bin/pip install faster-whisper
huggingface-cli download Systran/faster-whisper-small
```

`LING_STT_PYTHON`을 그 Python으로 지정하거나, Ling의 기본 폴더(Windows는
`%LOCALAPPDATA%\LingStation\stt\venv`, 그 외는 `~/.local/share/lingstation/stt/venv`)에 venv를
만드십시오. 모델은 로컬 Hugging Face 캐시에서 읽으며 네트워크는 쓰지 않습니다.

| 내장 함수 | 용도 |
|---|---|
| `stt_available()` | Whisper 엔진을 실행할 수 있으면 `true` |
| `stt_models()` | 설치된 모델을 평평한 리스트 `[name, size_mb, ...]`로 |
| `stt_start(audio, model, lang, words)` | 작업 시작. ID를 반환하고 시작할 수 없으면 `""` |
| `stt_poll(job)` | 0부터 1까지의 진행률 |
| `stt_done(job)` | 작업이 끝나면 `true` |
| `stt_error(job)` | `""`, 또는 실패한 이유 |
| `stt_info(job)` | `"loaded small on cuda"` 같은 상태 텍스트 |
| `stt_language(job)` | 감지된 언어 코드 |
| `stt_segments(job)` | 지금까지의 구간 `[start, end, text, ...]` |
| `stt_words(job)` | 지금까지의 단어 `[start, end, word, probability, ...]` |
| `stt_srt(job, max_chars)` | SubRip 자막. 큐마다 최대 `max_chars`자(기본값 42, `0` = 구간 전체) |
| `stt_align(job, lyrics)` | 붙여 넣은 텍스트의 각 줄을 들린 내용에 맞춰 시간 지정: `[start, end, line, ...]` |
| `stt_align_srt(job, lyrics)` | 같은 내용을 SubRip 자막으로 |
| `stt_cancel(job)` | 작업 중지 |

현지화된 별칭은 없습니다. `stt_start`는 모델 이름(기본값 `"small"`), 언어(기본값 `"auto"`, 또는
`"en"`, `"ja"` 같은 코드), 단어별 타임스탬프 기록 여부(기본값 `true`)를 받습니다. 시간 단위는
초입니다.

```ling-gui
바인드 시작 = 실행 {
    만약 !stt_available() {
        출력("Whisper 엔진이 설치되어 있지 않습니다")
    } 아니면 {
        바인드 작업 = stt_start("interview.wav", "small", "auto", 참)
        동안 !stt_done(작업) {
            출력(stt_poll(작업))
            잠(500)
        }
        만약 stt_error(작업) == "" {
            출력(stt_language(작업))
            write_file("interview.srt", stt_srt(작업, 42))
        }
    }
}
```

## 로컬 언어 모델: `llm_*`

`llm_*`는 `ling` 프로세스 안에서 채팅 모델을 실행합니다. 양자화된 **GGUF** 체크포인트와
`tokenizer.json`을 CPU나 CUDA GPU에 불러옵니다. 지원하는 아키텍처는 `qwen3`과 `glm4`입니다. 선택
기능 `llm`(GPU는 `llm-cuda`)이 필요합니다.

생성은 프로그램을 막지 않습니다. `llm_generate_start`는 작업 ID를 바로 반환하고, 여러분은
`llm_generate_poll`로 지금까지의 텍스트를 폴링합니다. 긴 생성이 모든 방문자를 멈추게 할 수 있는 웹
핸들러 안에서 특히 중요합니다.

대화 기록은 다음 형식의 문자열 하나이며, 사용자 차례로 끝나야 합니다.

```text
<|user|>
렌즈가 뭐예요?
<|assistant|>
빛을 굴절시키는 휜 유리 조각입니다.
<|user|>
거울은요?
```

| 내장 함수 | 용도 |
|---|---|
| `llm_load(gguf_path, tokenizer_path, device)` | 모델 불러오기. `device`가 음수면 CPU, 아니면 CUDA 번호. 핸들 또는 `-1` 반환 |
| `llm_generate_start(model, system_prompt, history, max_tokens, temperature, top_p)` | 응답 생성 시작(기본값 512, 0.7, 0.9). 작업 ID 반환 |
| `llm_generate_poll(job)` | 지금까지 생성된 텍스트 |
| `llm_generate_done(job)` | 끝나면 `true` |
| `llm_job_status(job)` | `"pending"` 또는 `"done"` |
| `llm_job_cancel(job)` | 작업을 완료로 표시(참고 사항 참조) |
| `llm_job_tool_name(job)`, `llm_job_provide_tool_result(job, text)` | 도구 호출용 예약. 아직 구현되지 않음 |

현지화된 별칭은 없습니다.

```ling-gui
바인드 시작 = 실행 {
    바인드 모델 = llm_load("models/Qwen3-0.6B-Q8_0.gguf", "models/tokenizer.json", 0 - 1)
    만약 모델 < 0 {
        출력("모델을 불러올 수 없습니다")
    } 아니면 {
        바인드 작업 = llm_generate_start(모델, "한 문장으로 답하세요.", "<|user|>\n하늘은 왜 파란가요?\n", 128, 0.7, 0.9)
        동안 !llm_generate_done(작업) {
            잠(250)
        }
        출력(llm_generate_poll(작업))
    }
}
```

참고:

- 불러오기는 모델 전체를 메모리에 읽으므로 몇 초가 걸릴 수 있습니다. 시작할 때 한 번만
  불러오십시오.
- 모델당 한 번에 하나의 생성만 실행됩니다.
- `llm_job_cancel`은 작업을 완료로 표시할 뿐입니다. 생성은 `max_tokens`에 이를 때까지
  백그라운드에서 계속되므로 `max_tokens`를 적당히 작게 두십시오.
- 검열 계층은 없습니다. 신뢰할 수 없는 사용자에게 보여 주기 전에 출력을 거르고, 모델 출력을 파일,
  SQL, 명령에 그대로 넘기지 마십시오.

## 이미지 이해: `vision_*`

`vision_*`는 양자화된 **Moondream2** 모델로 이미지에 대한 질문에 답합니다. `llm_*`처럼 한 번
불러온 뒤 백그라운드 작업을 폴링합니다. 선택 기능 `vision`(`llm` 포함)이 필요합니다. 문자
인식(OCR)은 하지 않습니다.

| 내장 함수 | 용도 |
|---|---|
| `vision_load(gguf_path, tokenizer_path, device)` | 모델 불러오기. 핸들 또는 `-1` 반환 |
| `vision_analyze_start(model, image_base64, question, max_tokens, temperature, top_p)` | 작업 시작(기본값 `"Describe this image."`, 256, 0.7, 0.9) |
| `vision_analyze_poll(job)` | 지금까지 생성된 텍스트 |
| `vision_analyze_done(job)` | 성공 여부와 관계없이 끝나면 `true` |
| `vision_analyze_error(job)` | `""`, 또는 실패한 이유 |

Ling 문자열에는 원시 바이트를 담을 수 없으므로, 이미지는 PNG나 JPEG 바이트를 **base64 문자열**로
넘깁니다. 웹 페이지에서는 `canvas.toDataURL()`을 전송하고 `data:image/png;base64,` 접두사를
떼어 내십시오.

```ling-gui
바인드 시작 = 실행 {
    바인드 모델 = vision_load("models/moondream2-q4.gguf", "models/moondream-tokenizer.json", 0 - 1)
    바인드 작업 = vision_analyze_start(모델, read_file("shot.b64"), "화면에 무엇이 있나요?", 128, 0.2, 0.9)
    동안 !vision_analyze_done(작업) {
        잠(250)
    }
    만약 vision_analyze_error(작업) == "" {
        출력(vision_analyze_poll(작업))
    }
}
```

## 함께 보기

- 모델을 막힘 없이 브라우저에 제공하는 방법은 [웹 서버와 HTTP](../reference/web.md)를 보십시오.
