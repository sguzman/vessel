#!/usr/bin/env fish

# Prefetch every network-fetched asset needed for the English WhisperX +
# pyannote path used by Vessel. Keep this separate from vessel update so
# transcription can run without depending on live network access.

if test (count $argv) -lt 1
    echo "usage: fish scripts/prefetch-whisperx-models.fish <sourcearium-root>" >&2
    exit 2
end

set -l sourcearium_root (realpath $argv[1])
if not test -f "$sourcearium_root/sourcearium.toml"
    echo "$sourcearium_root does not look like a Sourcearium root" >&2
    exit 2
end

if not set -q HF_TOKEN
    echo "HF_TOKEN is required for pyannote/speaker-diarization-community-1." >&2
    echo "Accept the model's Hugging Face user conditions, then export a read token for this shell." >&2
    exit 2
end

set -l tool_root (uv tool dir)
or exit $status
set -l python "$tool_root/whisperx/bin/python"

if not test -x "$python"
    echo "WhisperX uv tool environment not found at $python" >&2
    echo "Run fish scripts/provision-whisperx.fish first." >&2
    exit 2
end

set -l cache_root "$sourcearium_root/.cache/vessel/models"
set -l whisper_dir "$cache_root/whisperx"
set -l align_dir "$cache_root/whisperx"
set -l pyannote_dir "$cache_root/pyannote/speaker-diarization-community-1"

mkdir -p "$whisper_dir" "$align_dir" "$pyannote_dir"
or exit $status

echo "[prefetch] faster-whisper large-v3 -> $whisper_dir"
env HF_TOKEN="$HF_TOKEN" "$python" - "$whisper_dir" <<'PY'
import sys
from faster_whisper import download_model

target = sys.argv[1]
path = download_model("large-v3", cache_dir=target)
print(path)
PY
or exit $status

echo "[prefetch] English alignment model -> $align_dir"
"$python" - "$align_dir" <<'PY'
import gc
import sys
import torchaudio

target = sys.argv[1]
bundle = torchaudio.pipelines.WAV2VEC2_ASR_BASE_960H
model = bundle.get_model(dl_kwargs={"model_dir": target})
del model
gc.collect()
print(target)
PY
or exit $status

echo "[prefetch] NLTK punkt_tab sentence data"
"$python" - <<'PY'
import nltk

if not nltk.download("punkt_tab", quiet=False):
    raise SystemExit("failed to download NLTK punkt_tab")
print("punkt_tab ready")
PY
or exit $status

echo "[prefetch] pyannote speaker-diarization-community-1 -> $pyannote_dir"
env HF_TOKEN="$HF_TOKEN" "$python" - "$pyannote_dir" <<'PY'
import os
import sys
from huggingface_hub import snapshot_download

target = sys.argv[1]
path = snapshot_download(
    repo_id="pyannote/speaker-diarization-community-1",
    local_dir=target,
    token=os.environ["HF_TOKEN"],
)
print(path)
PY
or exit $status

echo
echo "Model assets ready."
echo "WhisperX model dir: $whisper_dir"
echo "Alignment cache dir: $align_dir"
echo "Pyannote model dir: $pyannote_dir"
echo
echo "Vessel should use:"
echo "  --asr-model-dir $whisper_dir"
echo "  --diarization-model $pyannote_dir"
