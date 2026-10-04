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
- named-speaker matching/calibration machinery.

These are not fallback paths.

## Failure Semantics

Temporary media and ASR cache state live under `<sourcearium>/.cache/vessel/`.

If acquisition, normalization, transcription, validation, or materialization fails, Vessel preserves the existing durable Sourcearium artifact.
