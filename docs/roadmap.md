# Vessel Roadmap

## Current State

The 2026-10-03 toolchain migration is complete.

The project has finished the architectural reset from "Rust-native media stack" to "small Sourcearium control plane around mature external tools."

As of 2026-10-07, the current external-toolchain architecture has also completed its first real target-environment production acceptance:

- creator subtitles were materialized for `Saw | ContraPoints`;
- WhisperX `large-v3` local ASR was materialized for Tiny Clipper `YOSJDOLe_R0`;
- optional pyannote diarization completed on that same real target;
- interrupted-cache recovery defects exposed by the run were repaired and the target subsequently completed successfully.

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

- [ ] harden yt-dlp JSON compatibility when upstream output changes;
- [x] classify authentication, browser-cookie, and rate-limit failures with actionable diagnostics;
- [ ] improve bounded retry/reporting behavior without contaminating durable artifacts;
- [x] gate a deterministic materialize -> validate/inventory -> repeat/no-op corpus loop in CI;
- [x] reject interrupted/corrupt normalized-audio cache entries and promote normalized WAV atomically;
- [x] reuse only verified cached source audio on retry;
- [ ] keep hardening SQLite/cache recovery only when real corpus runs expose a new failure.

### Corpus Semantics

- [ ] strengthen Sourcearium validation as real corpora expose edge cases;
- [ ] improve transcript normalization only when source-preserving;
- [ ] add representation providers only when they materially improve corpus coverage;
- [x] materialize creator subtitles through the current external-toolchain path on a real target;
- [x] materialize WhisperX local ASR through the current external-toolchain path on a real target;
- [x] materialize optional file-local WhisperX/pyannote diarization on real material;
- [x] preserve deterministic no-op updates for the strongest existing representation in acceptance tests;
- [ ] prove no-op behavior against a real target-environment Sourcearium run;
- [ ] materialize a real automatic-caption representation through the current toolchain;
- [ ] prove existing Sourcearium text survives an upstream private/deleted/inaccessible state in practice.

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
