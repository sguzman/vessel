# Local ASR

## Backend

Vessel's initial local-ASR implementation uses `whisper-candle-core`.

Why:

- pure Rust inference path
- no Python runtime
- no PyTorch runtime
- no C/C++ whisper.cpp binding layer
- CPU-capable by default
- Candle-compatible acceleration remains available as a later operational choice
- full-file transcription and timestamped segments are exposed by the library

The dependency is isolated behind the `vessel-asr` crate.

Sourcearium does not know or care which ASR library Vessel uses.

## Defaults

Initial Vessel defaults:

```text
engine = whisper-candle
model = small
device = cpu
```

The default model is multilingual because Sourcearium is not an English-only corpus.

The default device is CPU because GPU acceleration is an optimization, not a correctness requirement.

## Provenance Mapping

A local ASR artifact uses:

```toml
[representation]
derivation = "local_asr"
engine = "whisper-candle"
model = "small"
timestamps = true
```

The detected/selected language is stored in `representation.language`.

## Boundary

`vessel-asr` receives a local media/audio path and returns a normalized `TranscriptCandidate`.

It does not:

- discover YouTube videos
- download media
- understand Sourcearium filesystem layout
- write corpus files
- decide whether ASR is preferable to platform captions

Those responsibilities remain in their existing layers.

## Model Acquisition

The selected Whisper model may be downloaded by the ASR backend on first use and cached according to the backend's model-cache behavior.

Model files are operational dependencies, not Sourcearium artifacts.

## Future Acceleration

GPU support may be added as an operational flag/configuration without changing Sourcearium artifact schema or transcript-provider semantics.

CPU remains the portable baseline.
