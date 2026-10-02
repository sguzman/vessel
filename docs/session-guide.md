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

Current optional external backend:

- `whisperx`: faster-whisper + forced alignment + optional pyannote diarization, file-local speaker labels, optional speaker embeddings, live progress, and offline cache-only model-directory support

WhisperX is executable through Vessel when the external `whisperx` CLI is installed. Diarization identity remains unresolved until speaker-registry evidence is applied.

Model acquisition supports both:

1. automatic first-use download/cache when network access is acceptable
2. explicit offline/local model paths when the model has been downloaded ahead of time

Operational model commands:

```text
vessel asr models
vessel asr fetch --backend whisper-candle --model small
vessel asr fetch --backend phonon-2
```

`vessel asr fetch` prints the exact local model directory. Reuse it later with `vessel update --asr-model-dir <directory>`. Phonon can also use `--asr-executable <path>` for an explicitly selected virtual-environment binary.

Long-running ASR work must never be silent. At minimum Vessel must report model loading, audio normalization, transcription start, transcription progress when the backend exposes it, and completion/error.

Real ASR acceptance should use an optimized release binary, not `target/debug/vessel`.

## Current Next Work

The architecture is no longer the main uncertainty.

Highest-value next steps:

1. materialize one deliberately targeted real artifact with `--video-id`
2. inspect Sourcearium artifact structure and local-ASR provenance
3. rerun the same target and verify no-op Git behavior
4. expand to a small bounded batch
5. exercise prune planning against a deliberate real policy change
6. decide whether native YouTube PO-token support is worth adding for caption-body acquisition
7. consider optional ASR acceleration only after CPU behavior is proven

The residential `--preview --max-videos 3` acceptance is complete. Player/media acquisition works on the target network; current direct caption-body access is degraded by empty `timedtext` responses, so the sampled videos resolve to ASR fallback while retaining the caption-access diagnostic.

Do not add more generalized acquisition abstractions before real Sourcearium data demonstrates a need.
