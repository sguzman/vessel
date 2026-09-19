# Vessel Project Charter

## Identity

**Name:** Vessel  
**Project color:** Vessel Wake Teal `#16A7A0`  
**Prompt marker:** `[VESSEL | #16A7A0]`

The color is stable project identity. Use it in future Codex/director prompts, project dashboards, and cross-project coordination unless explicitly superseded.

## Mission

Vessel is a Rust-native acquisition and text-materialization engine for research corpora.

Its job is to turn declarative interest in media sources into durable text artifacts with provenance.

The canonical loop is:

```text
discover -> select -> acquire -> transcribe if necessary -> materialize -> verify
```

The desired steady-state user operation is:

```bash
vessel update
```

## Governing Product Principle

The research object is the text corpus, not the upstream platform metadata.

Metadata exists to:

- identify sources
- select material
- recover provenance
- support acquisition
- verify and revisit text

Metadata is not collected merely because it can be collected.

## Scope

Vessel should:

- discover media from configured sources
- support declarative selection policy
- acquire upstream subtitles/captions when available
- fall back to local ASR when acceptable upstream text does not exist
- preserve timestamps and source provenance
- materialize normalized, Git-friendly text artifacts
- maintain operational state needed to make updates incremental and idempotent
- retain existing useful download/extraction machinery

YouTube is the first-class source because the existing implementation is already strong there.

Vessel may later support other video/audio sources when that advances corpus materialization.

## Explicit Non-Goals

Vessel should not become, by default:

- a social analytics dashboard
- a subscriber/view time-series collector
- a historical warehouse for every upstream field
- a universal `yt-dlp` compatibility project
- a heterogeneous research corpus repository
- a tweet archiver merely because the corpus may contain tweets
- a permanent media archive when only the derived text is desired

A separate corpus repository may contain heterogeneous text from many producers and acquisition tools.

## Corpus Contract

Vessel must treat the external corpus as durable user-owned data.

Normal update behavior is additive and conservative.

A materialized item should have enough provenance to answer:

- where did this text come from?
- what upstream object does it represent?
- was the text creator-supplied, platform-generated, or locally transcribed?
- which language is represented?
- if locally transcribed, which engine/model produced it?
- what timestamp mapping is available?
- when was the artifact materially produced or replaced?

Operational details such as "last checked" belong in ignored cache/state unless they materially affect the research artifact.

## Transcript Precedence

Default precedence:

1. creator-provided subtitle/caption track
2. platform automatic caption track
3. local ASR

The precedence should be configurable eventually, but the provenance label must never be erased.

An ASR transcript may later be replaced by a better upstream transcript, but replacement should be content-aware and auditable through Git.

## ASR Policy

- CPU-first default
- GPU optional
- Rust-native implementation preferred
- transcription backend behind a stable interface
- temporary media is disposable after successful materialization unless policy says otherwise
- do not make a specific ASR engine part of the corpus file format

The engine should be replaceable without invalidating corpus semantics.

## Destructive Operations

`update` must not remove already materialized text solely because a source no longer matches current selection policy.

Pruning is a distinct operation and should support dry-run inspection.

Upstream deletion/private state must not automatically erase local research material.

## Director Model

Vessel predates the current director/grunt workflow. Going forward:

- The human principal defines goals, values, and major scope decisions.
- The project director maintains architecture, roadmap, invariants, task decomposition, and cross-session continuity.
- Codex/grunt sessions implement bounded tasks.
- A grunt task must not silently reinterpret the macro-goal.
- New subsystems or scope expansions require explicit director-level justification.
- Corrections should preserve the macro-goal while narrowing or replacing the implementation task.
- Documentation is part of implementation, not cleanup work deferred to the end.

## Engineering Invariants

- Prefer explicit failure over hidden fallback.
- Preserve provenance across every transformation.
- Separate acquisition state from durable corpus output.
- Keep repeated updates idempotent.
- Avoid Git churn from operational refresh metadata.
- Keep heavy work off UI/render threads if a UI is ever added.
- Prefer ordinary inspectable files for durable user data.
- Do not make SQLite the only route to reading a corpus.
- Preserve the ability to inspect and manipulate outputs without Vessel.

## Success Test

The project is succeeding when a user can maintain a meaningful set of media sources by editing a small declarative policy file and periodically running one command, after which the corpus contains every desired text artifact with trustworthy provenance.

Everything else is secondary.
