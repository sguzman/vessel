# Vessel Toolchain Migration Autopsy

> **Completion:** The migration described below is complete. yt-dlp, FFmpeg, and WhisperX now own the external machinery; the superseded native YouTube/download/format/postprocess/Whisper/Sherpa and named-speaker stacks were deleted. The maintained CLI is the Sourcearium reconcile surface only.

**Status:** completed 2026-10-03/04  
**Date:** 2026-10-03

## Executive conclusion

Vessel should stop treating Rust-native implementation as a goal in itself.

The durable value of Vessel is not that it knows how YouTube, codecs, Whisper inference, diarization,
speaker embeddings, or media formats work internally.

The durable value is that Vessel knows:

- which media-derived text belongs in Sourcearium;
- which representation is preferable;
- how provenance is represented;
- whether an existing artifact should be created, preserved, or upgraded;
- how to make repeated updates idempotent and non-destructive;
- how to validate and atomically materialize durable corpus artifacts.

The migration principle is therefore:

> **Own the semantics; outsource the machinery.**

Vessel should remain a small, reliable orchestration and reconciliation layer around mature installable
tools.

## Product boundary

The canonical product loop remains:

~~~text
discover
-> select
-> acquire the best available text
-> transcribe only when necessary
-> normalize
-> preserve provenance
-> reconcile with existing Sourcearium state
-> validate
-> materialize
~~~

Everything in Vessel should be judged by whether it directly supports that loop.

## What Vessel should own

Vessel should continue to own project-specific semantics:

| Responsibility | Ownership |
| --- | --- |
| source configuration and policy | Vessel |
| include/exclude/date selection rules | Vessel |
| transcript precedence | Vessel |
| Sourcearium schema mapping | Vessel |
| source / representation / acquisition provenance | Vessel |
| idempotency and reconcile decisions | Vessel |
| representation upgrade rules | Vessel |
| atomic validated materialization | Vessel |
| operational inventory and validation | Vessel |
| explicit non-destructive prune semantics | Vessel |
| operational cache/state needed for reconciliation | Vessel |

These are not commodity implementation details. They encode the product.

## What Vessel should stop owning

Vessel should not maintain bespoke implementations of mature, volatile, specialized machinery when a
stable process boundary can provide the capability.

| Current or historical responsibility | Direction |
| --- | --- |
| YouTube protocol/page/InnerTube reverse engineering | replace with yt-dlp |
| signature/cipher/player maintenance | replace with yt-dlp |
| general media download implementation | replace with yt-dlp |
| media format-selection engine | replace with yt-dlp where possible |
| codec/container/resampling logic | delegate to FFmpeg/ffprobe |
| Whisper inference implementation | replace with external ASR tooling |
| diarization implementation | replace with WhisperX/pyannote |
| speaker embedding implementation | stop owning |
| cross-video named-speaker matching | remove from active product |
| speaker anchor/cache/calibration research | remove from active product |
| creator-prior/cohort matching heuristics | remove from active product |

The existence of working bespoke code is not sufficient reason to keep maintaining it.

## Target architecture

~~~text
configured Sourcearium policy
          |
          v
        Vessel
          |
          +--------------------+
          |                    |
          v                    v
       yt-dlp               FFmpeg
 discovery / metadata       media normalization
 captions / audio
          |
          +----------+
                     |
                     v
               WhisperX
        ASR / alignment / optional
              diarization
                     |
                     v
          normalized transcript
                     |
                     v
             Vessel reconcile
       policy / provenance / quality
          / upgrade / validation
                     |
                     v
                Sourcearium
~~~

The external programs are implementation dependencies, not authorities over corpus semantics.

## YouTube

### Decision

Make **yt-dlp** the first-class YouTube acquisition backend.

Do not describe it as a fallback.

Vessel should invoke yt-dlp through a subprocess boundary and consume structured output for:

- source/video discovery;
- metadata;
- creator subtitles;
- platform automatic captions;
- best-audio acquisition when ASR is needed;
- format selection where necessary;
- browser-cookie or authentication integration when explicitly configured.

This moves YouTube churn out of Vessel.

### Consequence

The following bespoke surfaces become retirement candidates once the replacement path is proven:

- native watch-page parsing;
- InnerTube/client behavior;
- signature/cipher maintenance;
- native channel crawling where yt-dlp can provide the required discovery semantics;
- native format-selection logic;
- native download plumbing that duplicates yt-dlp.

Do not maintain two full YouTube implementations indefinitely.

## Local ASR

### Decision

External ASR becomes the normal architecture.

Vessel should not be an inference library.

### Heavy/production path

Use **WhisperX** as the preferred heavy transcription path when alignment or diarization is useful.

WhisperX provides the combined toolchain for:

- Whisper-family ASR via faster-whisper;
- VAD;
- word alignment;
- optional speaker diarization through pyannote.

Vessel consumes machine-readable output and maps it into its own normalized transcript representation.

### Lightweight QA path

Retain **Phonon-2** only if its fast local QA role remains materially useful.

The rule is:

~~~text
routine downstream QA -> lightest sufficient backend
production ASR / alignment / diarization -> WhisperX
~~~

Do not spend heavyweight compute merely to exercise downstream Vessel behavior.

### Python boundary

Python is acceptable behind a process boundary.

Vessel itself does not need to become a Python application.

Use **uv** to isolate Python tooling so the runtime relationship is:

~~~text
Rust Vessel
-> executable contract
-> isolated Python tool environment
-> JSON / files / exit status
~~~

The language used internally by an external tool is not part of Vessel's durable architecture.

## Diarization

### Decision

Anonymous diarization may remain a useful transcript enrichment, but Vessel should not implement the
diarization algorithm.

Use WhisperX/pyannote when diarization is requested.

The durable transcript may preserve anonymous labels such as:

~~~text
SPEAKER_00
SPEAKER_01
~~~

That is sufficient for the corpus unless a concrete later research requirement proves otherwise.

## Named speaker identity

### Decision

Cross-video named-speaker identification is removed from the active product direction.

Do not continue work on:

- speaker registries for automatic identity matching;
- cosine identity matching;
- exact-window embedding caches;
- creator priors;
- creator cohorts;
- rank-gap scoring;
- z-score or MAD normalization;
- threshold calibration;
- identity benchmark research.

Existing code may remain temporarily during migration for history/recovery, but it is not an active
milestone and should not receive new engineering investment.

If named identity ever returns, it requires a new explicit product justification.

## FFmpeg

FFmpeg/ffprobe remain appropriate external media plumbing.

Vessel should delegate:

- audio extraction;
- resampling;
- mono conversion;
- container conversion;
- probing.

Do not replace stable codec/media tooling with custom implementations.

## SQLite and operational state

SQLite is not automatically a migration target.

Unlike YouTube protocol handling or speech ML, operational reconciliation state encodes Vessel-specific
semantics:

- discovery cursors/backlog;
- retry state;
- representation probes;
- content hashes;
- materialization bookkeeping;
- cached publication facts.

Keep SQLite where it meaningfully supports idempotent reconciliation.

Simplify it only where the state model itself is unnecessarily complex.

## Current codebase autopsy

The current source tree shows where complexity accumulated.

Approximate source size at the time of this decision:

| Surface | Approximate size |
| --- | ---: |
| `vessel-cli/src/main.rs` | 325 KB |
| `vessel-store/src/sqlite.rs` | 77 KB |
| `vessel-core/src/sourcearium_repo.rs` | 77 KB |
| native YouTube extractor | 73 KB |
| `vessel-diarization` | 65 KB |
| speaker matching | 56 KB |
| `vessel-asr` | 46 KB |
| native download layer | 27 KB |
| postprocessing layer | 19 KB |
| format-selection layer | 6 KB |

The problem is not simply total code size.

The problem is that a large percentage of the complexity sits in commodity or specialized machinery
*before* Sourcearium materialization.

The migration should move code ownership toward the product-specific side of the boundary.

## Target internal shape

The desired end state has five conceptual components:

### 1. Policy

Reads declarative source policy and determines the desired media set.

### 2. Acquisition adapter

Invokes yt-dlp and converts structured output into Vessel's normalized acquisition model.

### 3. Transcription adapter

Invokes WhisperX, and optionally a lightweight QA backend, then converts tool output into a normalized
transcript.

### 4. Reconciler

Decides:

- create;
- preserve;
- upgrade;
- no-op.

It enforces transcript precedence and provenance rules.

### 5. Materializer

Validates Sourcearium candidates and performs deterministic atomic writes.

Any additional subsystem must justify why it cannot fit behind one of these boundaries.

## Migration strategy

Migration is replacement-first, deletion-second.

Do not delete working legacy paths before their replacement is proven.

### Phase 1 — external acquisition

- introduce a yt-dlp adapter;
- prove discovery, metadata, captions, and audio acquisition;
- preserve Vessel's existing policy/reconcile semantics;
- compare replacement outputs against current required Sourcearium fields.

### Phase 2 — external transcription

- introduce a WhisperX subprocess adapter;
- use isolated tooling managed with uv;
- normalize ASR/alignment/diarization output into the existing transcript model;
- preserve engine/model/tool provenance.

### Phase 3 — cut over

- make external acquisition/transcription the normal path;
- remove runtime dependence on the superseded native implementations;
- ensure `inventory`, `validate`, `update`, and `prune` remain coherent.

### Phase 4 — delete dead machinery

After replacement behavior is proven:

- retire native YouTube protocol/download duplication;
- retire bespoke ASR inference code that no longer serves a real role;
- retire bespoke diarization implementation;
- retire named-speaker identity/matching/calibration machinery;
- reduce format/postprocess code to the minimum needed around external tools;
- split oversized CLI orchestration where doing so improves maintainability.

Do not keep parallel stacks "just in case."

## Toolchain policy

The former default:

> Rust-native unless forced otherwise.

is replaced by:

> **Rust owns Vessel's durable semantics. Mature external tools own specialized machinery.**

Tool selection should optimize for:

1. less code Vessel must maintain;
2. less upstream-protocol knowledge Vessel must carry;
3. stable machine-readable process boundaries;
4. offline/local operation where feasible;
5. explicit installation and model acquisition;
6. provenance that Vessel can record;
7. replaceability if a tool later becomes unsuitable.

Implementation language is secondary.

## Success test after migration

The migration succeeds when:

1. a Sourcearium source can be maintained through `vessel update`;
2. Vessel itself contains no unnecessary YouTube protocol implementation;
3. Vessel itself contains no bespoke speaker-recognition research stack;
4. local ASR/diarization is delegated through a small replaceable adapter;
5. the durable corpus remains independent of every external runtime tool;
6. replacing yt-dlp or WhisperX would require changing an adapter, not corpus semantics;
7. the normal maintenance loop is smaller, more boring, and easier to reason about than the current
   implementation.

The target is not maximum implementation purity.

The target is a small program that reliably maintains the desired corpus.
