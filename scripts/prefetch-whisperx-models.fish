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
printf '%s\n' \
    'import sys' \
    'from faster_whisper import download_model' \
    '' \
    'target = sys.argv[1]' \
    'path = download_model("large-v3", cache_dir=target)' \
    'print(path)' \
    | "$python" - "$whisper_dir"
or exit $status

echo "[prefetch] English alignment model -> $align_dir"
printf '%s\n' \
    'import gc' \
    'import sys' \
    'import torchaudio' \
    '' \
    'target = sys.argv[1]' \
    'bundle = torchaudio.pipelines.WAV2VEC2_ASR_BASE_960H' \
    'model = bundle.get_model(dl_kwargs={"model_dir": target})' \
    'del model' \
    'gc.collect()' \
    'print(target)' \
    | "$python" - "$align_dir"
or exit $status

echo "[prefetch] NLTK punkt_tab sentence data"
printf '%s\n' \
    'import nltk' \
    '' \
    'if not nltk.download("punkt_tab", quiet=False):' \
    '    raise SystemExit("failed to download NLTK punkt_tab")' \
    'print("punkt_tab ready")' \
    | "$python" -
or exit $status

echo "[prefetch] pyannote speaker-diarization-community-1 -> $pyannote_dir"
printf '%s\n' \
    'import sys' \
    'from huggingface_hub import get_token, snapshot_download' \
    '' \
    'target = sys.argv[1]' \
    'token = get_token()' \
    'if not token:' \
    '    raise SystemExit("No Hugging Face token found in standard credential storage.")' \
    '' \
    'path = snapshot_download(' \
    '    repo_id="pyannote/speaker-diarization-community-1",' \
    '    local_dir=target,' \
    '    token=token,' \
    ')' \
    'print(path)' \
    | "$python" - "$pyannote_dir"
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
