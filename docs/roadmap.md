# Vessel Roadmap

## Current State

The 2026-10-03 toolchain migration is complete.

The project has finished the architectural reset from "Rust-native media stack" to "small Sourcearium control plane around mature external tools."

Completed migration outcomes:

- yt-dlp is the only maintained YouTube acquisition path;
- the native YouTube protocol implementation is deleted;
- the native downloader and format-selection crates are deleted;
- the native postprocessing crate is deleted;
- bespoke Whisper/Candle inference is deleted;
- bespoke Sherpa diarization is deleted;
- named-speaker identity/matching/calibration is deleted;
- WhisperX is the default heavy ASR and optional diarization path;
- Phonon-2 remains only as an optional external QA backend;
- the public CLI is reduced to doctor/update/validate/inventory/prune;
- deterministic external-toolchain acceptance is gated in CI.

## Maintained Product Work

Future work should improve the corpus loop rather than rebuild external machinery.

### Reliability

- harden yt-dlp JSON compatibility when upstream output changes;
- improve diagnostics around browser cookies/authentication;
- improve retry/reporting behavior without contaminating durable artifacts;
- keep SQLite state recoverable and disposable.

### Corpus Semantics

- strengthen Sourcearium validation as real corpora expose edge cases;
- improve transcript normalization only when source-preserving;
- add representation providers only when they materially improve corpus coverage;
- preserve deterministic no-op updates.

### Performance

- reduce redundant remote probes;
- reuse safe operational facts;
- keep ASR bounded and explicit;
- prefer the lightest sufficient external backend.

## Explicitly Closed Branches

Do not reopen without a new product justification:

- native YouTube protocol extraction;
- native media downloader/format-selection parity;
- bespoke Whisper inference;
- bespoke diarization;
- cross-video named-speaker identity;
- embedding calibration and speaker registries;
- broad yt-dlp replacement/parity work.

## Product Success Metric

A configured Sourcearium source should remain maintainable through a boring repeated `vessel update`, with trustworthy provenance and no unnecessary custom media/ML stack.
