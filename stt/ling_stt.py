"""Speech to text for ling-lang, through faster-whisper, running entirely on this machine.

Usage (what the `stt_*` builtins of ling run):
    python ling_stt.py models
    python ling_stt.py transcribe <audio file> [--model small] [--lang auto] [--words 1] [--device auto] [--task transcribe|translate]

Everything is printed as one JSON object per line:
    {"type": "models", "models": [{"name": "small", "path": "...", "size_mb": 460}, ...]}
    {"type": "info", "message": "...", "device": "cuda", "model": "small"}
    {"type": "progress", "done": 12.3, "total": 200.0}
    {"type": "segment", "start": 1.2, "end": 3.4, "text": "hello world", "avg_logprob": -0.2, "words": [[1.2, 1.6, "hello", 0.98], ...]}
    {"type": "done", "language": "en", "language_probability": 0.99, "duration": 200.0}
    {"type": "error", "message": "..."}
No network is used: models are read from the local Hugging Face cache (or a folder given with --model), so a model has to be downloaded once
(for instance `huggingface-cli download Systran/faster-whisper-small`) before it can be used.
"""
import json
import os
import sys


def out(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def hf_roots():
    roots = []
    for var in ("HF_HUB_CACHE", "HUGGINGFACE_HUB_CACHE"):
        if os.environ.get(var):
            roots.append(os.environ[var])
    if os.environ.get("HF_HOME"):
        roots.append(os.path.join(os.environ["HF_HOME"], "hub"))
    roots.append(os.path.join(os.path.expanduser("~"), ".cache", "huggingface", "hub"))
    roots.append(os.path.join(os.environ.get("LOCALAPPDATA", ""), "LingStation", "stt", "models"))
    return [r for r in roots if r and os.path.isdir(r)]


def folder_size_mb(p):
    total = 0
    for dp, _dn, fn in os.walk(p):
        for f in fn:
            try:
                total += os.path.getsize(os.path.join(dp, f))
            except OSError:
                pass
    return int(total / 1048576)


def find_models():
    """name -> (path, size): every faster-whisper model snapshot in the Hugging Face caches"""
    found = {}
    for root in hf_roots():
        for d in sorted(os.listdir(root)):
            if not d.startswith("models--") or "faster-whisper" not in d.lower() and "whisper" not in d.lower():
                continue
            snaps = os.path.join(root, d, "snapshots")
            if not os.path.isdir(snaps):
                continue
            for s in sorted(os.listdir(snaps)):
                p = os.path.join(snaps, s)
                if os.path.isfile(os.path.join(p, "model.bin")):
                    name = d.split("--", 2)[-1]
                    for prefix in ("faster-whisper-", "whisper-"):
                        if name.startswith(prefix):
                            name = name[len(prefix):]
                    found[name] = (p, folder_size_mb(p))
    return found


def prepare_cuda():
    """Make the CUDA 12 / cuDNN 9 libraries findable: a PyTorch install next to the base Python ships them."""
    cands = []
    if os.environ.get("LING_STT_CUDA_DIR"):
        cands.append(os.environ["LING_STT_CUDA_DIR"])
    for base in {sys.base_prefix, sys.prefix}:
        cands.append(os.path.join(base, "Lib", "site-packages", "torch", "lib"))
        cands.append(os.path.join(base, "lib", "site-packages", "torch", "lib"))
    cands += [p for p in os.environ.get("PATH", "").split(os.pathsep) if p]
    for d in cands:
        if os.path.isfile(os.path.join(d, "cudnn_ops64_9.dll")) and os.path.isfile(os.path.join(d, "cublas64_12.dll")):
            try:
                os.add_dll_directory(d)
            except Exception:  # noqa
                pass
            os.environ["PATH"] = d + os.pathsep + os.environ.get("PATH", "")
            return d
    return None


def cuda_works(model_path):
    """CUDA failures can abort the whole process, so the test runs in a child process (cached for a day)."""
    import subprocess, time
    cache = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".cuda_probe.json")
    try:
        c = json.load(open(cache))
        if time.time() - c.get("t", 0) < 86400 and c.get("dll") == (prepare_cuda() or ""):
            return c.get("ok", False)
    except Exception:  # noqa
        pass
    try:
        ok = subprocess.run([sys.executable, os.path.abspath(__file__), "cuda-probe", model_path], capture_output=True, timeout=180).returncode == 0
    except Exception:  # noqa
        ok = False
    try:
        json.dump({"t": time.time(), "ok": ok, "dll": prepare_cuda() or ""}, open(cache, "w"))
    except Exception:  # noqa
        pass
    return ok


def cmd_models():
    m = find_models()
    out({"type": "models", "models": [{"name": k, "path": v[0], "size_mb": v[1]} for k, v in sorted(m.items())]})


def resolve_model(name):
    if os.path.isdir(name) and os.path.isfile(os.path.join(name, "model.bin")):
        return name
    m = find_models()
    if name in m:
        return m[name][0]
    # a short name that is part of a longer one ("large" -> "large-v3")
    for k in sorted(m, reverse=True):
        if k.startswith(name):
            return m[k][0]
    return None


def cmd_cuda_probe(path):
    """exit code 0 when this model runs on the GPU (run in a child process: a missing CUDA library can abort the process)"""
    prepare_cuda()
    import numpy as np
    from faster_whisper import WhisperModel
    m = WhisperModel(path, device="cuda", compute_type="float16", local_files_only=True)
    list(m.transcribe(np.zeros(16000, dtype="float32"), language="en")[0])
    return 0


def cmd_transcribe(argv):
    opts = {"model": "small", "lang": "auto", "words": "1", "device": "auto", "task": "transcribe", "vad": "1"}
    audio = None
    i = 0
    while i < len(argv):
        a = argv[i]
        if a.startswith("--") and i + 1 < len(argv):
            opts[a[2:]] = argv[i + 1]
            i += 2
        else:
            audio = a
            i += 1
    if not audio or not os.path.isfile(audio):
        out({"type": "error", "message": "audio file not found: %s" % audio})
        return 1
    path = resolve_model(opts["model"])
    if path is None:
        names = ", ".join(sorted(find_models())) or "none found"
        out({"type": "error", "message": "model '%s' is not in the local cache (available: %s). Download one once, e.g. huggingface-cli download Systran/faster-whisper-%s" % (opts["model"], names, opts["model"])})
        return 1
    try:
        from faster_whisper import WhisperModel
    except Exception as e:  # noqa
        out({"type": "error", "message": "faster-whisper is not installed in this Python: %s" % e})
        return 1
    want = opts["device"]
    cuda = False
    if want in ("auto", "cuda"):
        try:
            prepare_cuda()
            import ctranslate2
            cuda = ctranslate2.get_cuda_device_count() > 0 and cuda_works(path)
        except Exception:  # noqa
            cuda = False
    plans = []
    if want in ("auto", "cuda") and cuda:
        plans.append(("cuda", "float16"))
    plans.append(("cpu", "int8"))
    model = None
    used = None
    last = None
    for dev, ctype in plans:
        try:
            model = WhisperModel(path, device=dev, compute_type=ctype, local_files_only=True)
            used = dev
            break
        except Exception as e:  # noqa
            last = e
            model = None
    if model is None:
        out({"type": "error", "message": "could not load the model: %s" % last})
        return 1
    out({"type": "info", "message": "loaded %s on %s" % (opts["model"], used), "device": used, "model": opts["model"]})
    lang = None if opts["lang"] in ("auto", "", "none") else opts["lang"]
    try:
        segments, info = model.transcribe(
            audio,
            language=lang,
            task=opts["task"],
            word_timestamps=opts["words"] == "1",
            vad_filter=opts["vad"] == "1",
            beam_size=5,
            condition_on_previous_text=False,
        )
        total = float(info.duration or 0.0)
        for seg in segments:
            obj = {"type": "segment", "start": round(seg.start, 3), "end": round(seg.end, 3), "text": seg.text.strip(), "avg_logprob": round(seg.avg_logprob, 3)}
            if seg.words:
                obj["words"] = [[round(w.start, 3), round(w.end, 3), w.word.strip(), round(w.probability, 3)] for w in seg.words]
            out(obj)
            out({"type": "progress", "done": round(seg.end, 2), "total": round(total, 2)})
        out({"type": "done", "language": info.language, "language_probability": round(info.language_probability, 3), "duration": round(total, 2)})
    except Exception as e:  # noqa
        out({"type": "error", "message": "transcription failed: %s" % e})
        return 1
    return 0


def main():
    if len(sys.argv) < 2:
        out({"type": "error", "message": "usage: ling_stt.py models | transcribe <audio> [options]"})
        return 2
    if sys.argv[1] == "models":
        cmd_models()
        return 0
    if sys.argv[1] == "cuda-probe" and len(sys.argv) > 2:
        return cmd_cuda_probe(sys.argv[2])
    if sys.argv[1] == "transcribe":
        return cmd_transcribe(sys.argv[2:])
    out({"type": "error", "message": "unknown command %s" % sys.argv[1]})
    return 2


if __name__ == "__main__":
    sys.exit(main())
