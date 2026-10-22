# One-time setup of the speech-to-text bridge used by ling-lang's stt_* builtins (and LingStation > Transcribe Vocals).
# Creates %LOCALAPPDATA%\LingStation\stt\venv with faster-whisper. Nothing is installed into your normal Python.
#   powershell -File stt\setup_windows.ps1 [-Python python] [-Model small]
param([string]$Python = "python", [string]$Model = "")
$ErrorActionPreference = "Stop"
$dir = Join-Path $env:LOCALAPPDATA "LingStation\stt"
New-Item -ItemType Directory -Force $dir | Out-Null
& $Python -m venv (Join-Path $dir "venv")
$py = Join-Path $dir "venv\Scripts\python.exe"
& $py -m pip install --upgrade pip
# ctranslate2 4.8 and onnxruntime 1.23 crashed on one test machine's CPU; these versions are known to work
& $py -m pip install --only-binary :all: faster-whisper "ctranslate2==4.5.0" "onnxruntime==1.19.2"
if ($Model -ne "") {
  & $py -m pip install huggingface_hub
  & $py -c "from huggingface_hub import snapshot_download; snapshot_download('Systran/faster-whisper-$Model')"
}
Write-Host "done: $py"
Write-Host "models found:"
& $py (Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) "ling_stt.py") models
