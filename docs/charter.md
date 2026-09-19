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

Sourcearium is the durable heterogeneous corpus. Vessel is one producer for it.

## Sourcearium Contract

Vessel currently targets **Sourcearium artifact schema v1**.

This is an external contract owned by the Sourcearium project.

Vessel must not silently invent a separate corpus format or extend Sourcearium v1 core fields.

See [docs/sourcearium-contract.md](sourcearium-contract.md).

A materialized Sourcearium item must preserve the three provenance layers:

- source identity
- textual representation
- acquisition process

Normal update behavior is additive and conservative.

Operational details such as last-check timestamps belong in ignored/cache state unless they materially define a newly acquired representation.

## Transcript Precedence

Default precedence:

1. creator-provided subtitle/caption track
2. platform automatic caption track
3. local ASR

A stronger representation may replace a weaker one for the same Sourcearium artifact identity.

A weaker representation must not automatically overwrite a stronger one.

## ASR Policy

- CPU-first default
- GPU optional
- Rust-native implementation preferred
- transcription backend behind a stable interface
- temporary media disposable after successful materialization unless policy says otherwise
- ASR engine/model recorded in Sourcearium representation provenance

The transcription backend is replaceable without invalidating Sourcearium semantics.

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
- Separate acquisition state from durable Sourcearium output.
- Keep repeated updates idempotent.
- Avoid Git churn from operational refresh metadata.
- Validate candidates before replacing durable artifacts.
- Use atomic replacement for Sourcearium writes.
- Keep heavy work off UI/render threads if a UI is ever added.
- Preserve the ability to inspect and manipulate Sourcearium outputs without Vessel.

## Success Test

The project is succeeding when a user can maintain a meaningful set of media sources by editing a small declarative policy file and periodically running one command, after which Sourcearium contains every desired text artifact with trustworthy provenance.

Everything else is secondary.
