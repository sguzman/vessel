# Vessel Project Charter

## Mission

Vessel maintains selected media-derived text as durable, provenance-preserving Sourcearium artifacts.

The canonical loop is:

```text
discover -> select -> resolve best text -> transcribe if necessary
-> reconcile -> validate -> materialize
```

The normal user operation is `vessel update`.

## Governing Principle

> **Own the semantics; outsource the machinery.**

Vessel owns corpus policy, transcript precedence, provenance, idempotency, upgrade decisions, validation, atomic writes, and operational reconciliation state.

Vessel delegates YouTube behavior to yt-dlp, media plumbing to FFmpeg/ffprobe, heavy ASR to WhisperX/faster-whisper, and optional anonymous diarization to pyannote through WhisperX.

Rust is the control-plane implementation language. It is not a mandate to reimplement mature external systems.

## Product Scope

Vessel should:

- discover configured media sources;
- apply include/exclude/date policy;
- prefer creator subtitles, then platform automatic captions, then local ASR;
- preserve timestamps and source/representation/acquisition provenance;
- maintain replaceable SQLite/cache state for incremental updates;
- deterministically materialize Sourcearium schema v1;
- upgrade weaker representations without downgrading stronger ones;
- keep normal update non-destructive;
- expose explicit offline prune planning and application.

## Explicit Non-Goals

Vessel is not:

- a yt-dlp replacement;
- a codec/media framework;
- an ML inference framework;
- a speaker-recognition research system;
- a metadata time-series warehouse;
- a universal heterogeneous corpus repository.

Cross-video named-speaker identity is retired.

## External Tool Policy

The maintained boundaries are:

- yt-dlp for YouTube acquisition;
- FFmpeg/ffprobe for media normalization;
- WhisperX for production local ASR and optional diarization;
- Phonon-2 only as an optional lightweight QA backend;
- uv as the preferred Python environment manager.

Tool/model provenance must be recorded when it defines the durable textual representation.

## Engineering Invariants

- Prefer explicit failure over hidden fallback.
- Keep Sourcearium durable data independent of Vessel's caches and external runtime tools.
- Preserve source / representation / acquisition provenance separately.
- Keep repeated updates idempotent.
- Validate before replacing durable output.
- Write atomically.
- Never downgrade a stronger transcript automatically.
- Never delete acquired research text merely because upstream content disappears.
- Keep destructive prune separate from update.
- Do not revive parallel native stacks "just in case."

## Success Test

Vessel succeeds when a user can edit small declarative source policies, periodically run one command, and obtain every desired text artifact in Sourcearium with trustworthy provenance and minimal maintenance machinery.
