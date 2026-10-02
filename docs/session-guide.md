# Session Guide

## Project Identity

Use this marker in future implementation prompts:

```text
[VESSEL | #16A7A0]
```

Project color: Vessel Wake Teal `#16A7A0`.

## Director-Era Operating Model

Vessel predates the current director/grunt workflow. Future work follows this model.

### Human principal

Owns:

- ultimate goals
- value judgments
- major scope changes
- approval of destructive or externally consequential behavior

### Project director

Owns:

- project continuity
- architecture
- invariants
- roadmap
- task decomposition
- integration decisions
- documentation consistency
- deciding whether a proposed feature advances the actual mission

### Codex / grunt session

Owns:

- one bounded implementation task
- tests for that task
- required doc updates
- reporting concrete blockers or deviations

A grunt does not silently redefine the macro-goal.

## Current Macro-Goal

Make Vessel a reliable media-to-text materialization engine for maintaining Sourcearium.

Do not optimize a task for broad `yt-dlp` parity unless the task explicitly requires it.

## Frozen External Contracts

Sourcearium has two current frozen contracts relevant to Vessel:

1. artifact schema v1
2. YouTube source policy v1

Before corpus-facing implementation, read:

```text
docs/sourcearium-contract.md
docs/youtube-policy-contract.md
```

A grunt must not:

- invent new Sourcearium v1 core fields
- invent new YouTube policy v1 fields
- collapse source/representation/acquisition provenance
- serialize operational refresh telemetry into Sourcearium
- move execution throttles into durable corpus policy
- weaken non-destructive update semantics
- change Sourcearium semantics to simplify Vessel implementation

If an external contract is insufficient, escalate to the director.

## Session Start Checklist

Before implementation:

1. Read `docs/charter.md`.
2. Read both Sourcearium contracts for corpus-facing work.
3. Read `docs/youtube-access.md` before changing YouTube authentication, player-client, cookie, or PO-token behavior.
4. Read `docs/speaker-attribution.md` before changing ASR backends, diarization, speaker embeddings, or cross-video identity behavior.
5. Read the relevant roadmap milestone.
6. State the bounded implementation target.
7. Identify durable Sourcearium semantics versus Vessel operational state.
8. Preserve unrelated working capabilities.

## Engineering Rules

- Prefer Rust-native implementation.
- No silent Python/`yt-dlp` fallback for supported native behavior.
- Preserve provenance.
- Keep Sourcearium output readable without Vessel.
- Keep operational refresh state out of durable artifacts/policy.
- Repeated runs should be idempotent.
- Serialize Sourcearium deterministically.
- Validate policy before network work.
- Validate candidate artifacts before replacement.
- Replace durable artifacts atomically.
- Normal update must be non-destructive.
- Bare `vessel prune` must never delete; deletion requires `--apply`.
- Prune must never infer deletion from upstream disappearance or an absent source policy.
- Do not introduce a generalized DSL where the frozen policy is enough.
- Do not create new time-series collection merely because fields are available.
- Tests should target reconciliation, idempotency, provenance, deterministic output, and safe failure.

## Definition Of Done

A task is done when:

- the requested path works
- tests cover the important invariant
- failure behavior is explicit
- relevant docs match actual behavior
- no unrelated scope was silently introduced

## Correction Lifecycle

When a task implementation is wrong:

- preserve the macro-goal
- describe the defect precisely
- issue a corrected bounded task
- do not rewrite the project mission to justify the mistaken implementation

## Current Implementation State

The following are already implemented and CI-tested:

- Sourcearium artifact schema v1 types and deterministic serializer
- YouTube source-policy v1 parser/validator
- creator-subtitle and automatic-caption provider path
- pure-Rust Candle/Whisper local ASR fallback
- CPU-first ASR with lazy model reuse
- pluggable local-ASR backend boundary
- Phonon-2 CLI backend with runtime engine/model provenance
- explicit ASR model catalog/prefetch commands
- offline model-directory and external-backend executable overrides
- long-running ASR heartbeat/progress output
- versioned per-source speaker registries with human-confirmed anchors
- Rust-native sherpa-onnx diarization with file-local speaker labels and embeddings
- backend-neutral persisted speaker-evidence sidecars
- read-only cross-video cosine speaker matching with threshold/margin gates
- non-destructive Sourcearium speaker-attribution metadata application
- speaker-attributed transcript projection without canonical-body mutation
- Sourcearium materializer with atomic replacement and downgrade protection
- offline `vessel validate`
- offline `vessel inventory`
- Sourcearium-driven `vessel update`
- persistent disposable crawl/backlog/cutoff/probe state
- limited-run backlog safety
- upgrade-probe cadence for weaker transcripts
- metadata-only no-churn semantics
- opt-in per-video decision reporting
- non-materializing `--preview` mode
- operational `--video-id` targeting for bounded acceptance/debug runs
- explicit caption-access diagnostics when advertised YouTube caption tracks return empty bodies
- explicit SQLite WAL checkpoint/close at the end of Sourcearium updates
- explicit offline safe prune: plan by default, delete only with `--apply`
- Linux full-workspace CI
- native Windows path/store/ASR tests and CLI compile
- locked Cargo dependency resolution

## Local ASR Choice And Provenance

Local ASR is not synonymous with Whisper.

The user must be able to choose the speed/accuracy/resource tradeoff per run without changing durable Sourcearium source policy. Backend/model selection is operational configuration; the resulting representation provenance is durable.

Required provenance for every local-ASR artifact:

- `representation.derivation = "local_asr"`
- `representation.engine` identifies the inference engine/backend
- `representation.model` identifies the model that produced the text

A transcript produced by one backend/model must never be presented as if another backend/model produced it.

Current backends:

- `whisper-candle`: pure-Rust Whisper, multilingual, timestamp-capable, CPU-first
- `phonon-2`: Fermion Research CLI backend, English-only fast path, JSON segment timestamps, live stderr progress, local model-directory support

Current optional external ASR backend:

- `whisperx`: faster-whisper + forced alignment + optional bundled diarization/alignment behavior when explicitly selected

WhisperX remains a compatibility backend and may require Python. It is not a prerequisite for normal Vessel operation.

### Diarization Is Independent From ASR

Speaker diarization is a separate operational stage from speech recognition.

Primary backend:

- `sherpa-onnx`: Rust integration over sherpa's C ABI, dynamically loaded at execution time; offline speaker diarization with Pyannote segmentation ONNX + speaker-embedding ONNX, CPU-first, no Python runtime

Optional compatibility backend:

- `whisperx`: external Python CLI path retained for users who explicitly want that pipeline

This means `whisper-candle + sherpa-onnx` and `phonon-2 + sherpa-onnx` are both valid pipelines. A diarization backend must never dictate which ASR engine produced the transcript.

Rust-native diarization acquisition workflow:

```text
vessel diarization models
vessel diarization fetch --plan
vessel diarization fetch
vessel diarization doctor
```

`vessel diarization fetch --plan` performs zero network I/O and prints the exact runtime/model URLs, target paths, expected byte counts, whether each artifact is already available, and the integrity-receipt paths that a successful fetch will create.

`vessel diarization fetch` is the only Vessel-managed network acquisition step for the primary sherpa path. It explicitly downloads both the platform-native sherpa/ONNX Runtime bundle and the official Pyannote segmentation + 3D-Speaker embedding model pair into durable user data with visible progress. Downloads use resumable `.download` partials and exact expected-size validation before installation. On Linux x86_64 the current explicit payload is about 53.5 MiB total: about 9.1 MiB native runtime, 6.6 MiB segmentation archive, and 37.8 MiB speaker-embedding model.

After installation, Vessel writes local BLAKE3 receipts for the sherpa runtime library and inference models. `vessel diarization doctor` verifies those receipts entirely offline. A receipt mismatch makes the fetched default installation not ready for diarization; explicitly supplied local model paths remain usable without requiring a Vessel-generated receipt.

Vessel does not depend on the `sherpa-onnx` or `sherpa-onnx-sys` Cargo crates. `cargo build` therefore does not run sherpa's downloader or fetch sherpa native runtime/model artifacts. At execution time Vessel dynamically loads the already-fetched sherpa C runtime from the user-data directory. If the runtime or models are missing, `--diarize` fails with an explicit instruction to run `vessel diarization fetch`; it does not silently download them.

Model acquisition is always explicit:

1. `vessel diarization fetch` performs the network acquisition when desired
2. explicit local runtime/model paths support pre-staged or fully offline layouts

Normal `vessel update --diarize` never performs first-use downloads.

Operational model and preflight commands:

```text
vessel asr models
vessel asr doctor --backend whisperx
vessel asr doctor --backend phonon-2
vessel asr fetch --backend whisper-candle --model small
vessel asr fetch --backend phonon-2
```

`vessel asr models` reports what Vessel implements, not what is installed on the current machine.

`vessel asr doctor` checks local runtime readiness without starting a transcription. For external backends it probes the executable, optional model directory, and (for WhisperX diarization) only whether the configured Hugging Face token environment variable is present. It never prints the token value.

`vessel asr fetch` prints the exact local model directory. Reuse it later with `vessel update --asr-model-dir <directory>`. Phonon can also use `--asr-executable <path>` for an explicitly selected virtual-environment binary.

Long-running ASR work must never be silent. At minimum Vessel must report model loading, audio normalization, transcription start, transcription progress when the backend exposes it, and completion/error.

Real ASR acceptance should use an optimized release binary, not `target/debug/vessel`.

## Current Next Work

The architecture is no longer the main uncertainty.

Highest-value next steps:

1. validate Rust-native sherpa-onnx diarization on the actual target machine
2. validate at least one ASR/diarization combination without Python
3. create the first human-confirmed ContraPoints voice anchor from a clean creator-only range
4. validate `speakers match`, `speakers apply`, and `speakers render` against real sherpa speaker embeddings
5. calibrate matching thresholds from real repeated creator/guest samples
6. rerun the same target and verify no-op Git behavior
7. expand to a small bounded batch
8. keep WhisperX as optional future compatibility rather than a required dependency

The residential `--preview --max-videos 3` acceptance is complete. Player/media acquisition works on the target network; current direct caption-body access is degraded by empty `timedtext` responses, so the sampled videos resolve to ASR fallback while retaining the caption-access diagnostic.

Do not add more generalized acquisition abstractions before real Sourcearium data demonstrates a need.
