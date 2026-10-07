# Local ASR

## Maintained Architecture

Vessel delegates ASR through executable adapters.

```text
yt-dlp -> temporary source audio
FFmpeg -> 16 kHz mono PCM WAV
WhisperX -> JSON
Vessel -> TranscriptCandidate -> Sourcearium
```

The default backend is **WhisperX** with `large-v3`. CPU is the default device; GPU acceleration may be selected explicitly.

Phonon-2 remains an optional external English-only QA backend. It is not a second production ML stack inside Vessel.

## Python Toolchain Policy

For Python-backed tooling, **uv is the only supported package/environment manager**.

Vessel owns no Python environment and performs no Python package installation during `update`.

Hard rules:

- provision WhisperX/pyannote explicitly ahead of normal corpus work;
- use `uv` for Python package/environment operations;
- provisioning must use uv's no-build mode so an unavailable wheel/cache entry fails instead of compiling a source distribution;
- normal `vessel update` must never run pip, uv sync/install, a Python build backend, or an implicit package installer;
- offline/cached provisioning is preferred when practical;
- Vessel consumes a pre-provisioned `whisperx` executable or one supplied with `--asr-executable`.

A typical explicit provisioning policy is equivalent to:

```text
uv ... --no-build
```

When the required packages and metadata are already cached, uv's `--offline` mode may also be used. Provisioning is intentionally separate from corpus ingestion.

For the supported persistent setup on the target Linux machine, run:

```fish
fish scripts/provision-whisperx.fish
```

That pins the current accepted stable WhisperX release, requests Python 3.12 through uv, and passes `--no-build`. If any required Python package lacks an installable wheel/cache entry, provisioning fails instead of compiling it.

After the online provisioning pass has populated uv's package cache, the same environment can be recreated without network package access when cache coverage is complete:

```fish
fish scripts/provision-whisperx.fish --offline
```

Model weights are not Python package builds. WhisperX may fetch model weights when its cache is incomplete, but for predictable/offline work prefer preparing the model cache ahead of time and passing it with `--asr-model-dir`. Vessel then asks WhisperX to use cache-only model loading.

## Offline Model Prefetch

Keep network acquisition separate from transcription:

```fish
fish scripts/prefetch-whisperx-models.fish <sourcearium-root>
```

The script requires `HF_TOKEN` because `pyannote/speaker-diarization-community-1` is gated. It stores:

- Faster-Whisper `large-v3` in the Hugging Face cache rooted at `<sourcearium>/.cache/vessel/models/whisperx`;
- pyannote `speaker-diarization-community-1` under `<sourcearium>/.cache/vessel/models/pyannote/speaker-diarization-community-1`.

The later Vessel run should pass those paths with `--asr-model-dir` and `--diarization-model`. WhisperX then uses cache-only loading for the ASR model, and pyannote loads its local pipeline directory.

## Diarization

`vessel update --diarize` asks WhisperX to run anonymous diarization through pyannote.

Vessel preserves file-local labels such as `SPEAKER_00` and records diarization provenance. It does not infer stable human identity across videos.

## Adapter Controls

The update path supports:

- ASR backend selection: WhisperX or Phonon-2;
- model selection;
- executable path;
- device;
- language hint;
- explicit model/cache directory;
- optional diarization model;
- optional minimum/maximum speaker hints;
- configurable Hugging Face token environment.

Diarization requires WhisperX.

## Removed Implementations

The migration deleted:

- `whisper-candle` inference;
- Rust/Sherpa diarization;
- speaker-embedding evidence persistence;
- named-speaker matching/calibration machinery;
- direct Python model-provisioning helpers inside Vessel.

These are not fallback paths.

## Failure Semantics

Temporary media and ASR cache state live under `<sourcearium>/.cache/vessel/`.

If acquisition, normalization, transcription, validation, or materialization fails, Vessel preserves the existing durable Sourcearium artifact.
