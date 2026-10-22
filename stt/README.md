# Speech to text in ling-lang (faster-whisper, local)

`stt_*` builtins transcribe audio on this computer with [faster-whisper](https://github.com/SYSTRAN/faster-whisper): no network, no hosted service.
A child Python process (`ling_stt.py`, embedded in the `ling` binary) runs the model on the GPU (CUDA, found through a PyTorch install when there
is one) or the CPU; Ling jobs are polled like the `llm_*` ones.

```ling
bind start = do {
    bind job = stt_start("song vocals.wav", "small", "auto", true)   # audio, model, language, word timestamps
    while !stt_done(job) { println(round(stt_poll(job) * 100.0), "%") sleep(500.0) }
    println(stt_error(job), stt_language(job))
    println(stt_segments(job))    # [start, end, text, ...]
    println(stt_words(job))       # [start, end, word, probability, ...]
    write_file("vocals.srt", stt_srt(job, 42.0))   # cues of at most 42 characters, broken at sentences and pauses
}
```
Also `stt_available()`, `stt_models()` (name, size MB of every model in the Hugging Face cache), `stt_info(job)` ("loaded small on cuda"),
`stt_cancel(job)`. See `examples/stt_demo.ling`.

Setup: `powershell -File stt\setup_windows.ps1 -Model small` (creates `%LOCALAPPDATA%\LingStation\stt\venv`). Models are looked up in the
Hugging Face cache (`Systran/faster-whisper-small|medium|large-v3`, `mobiuslabsgmbh/faster-whisper-large-v3-turbo` ...) and never downloaded
by the bridge. `LING_STT_PYTHON` points at another interpreter, `LING_STT_CUDA_DIR` at a folder with cuDNN 9 / cuBLAS 12.

In LingStation: **File > Transcribe Vocals (SRT)...** writes `.srt` (and `.txt`) next to the audio and can fit the words to a MIDI clip's notes.
