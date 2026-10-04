# Vessel Project Charter

## Identity

**Name:** Vessel  
**Project color:** Vessel Wake Teal `#16A7A0`  
**Prompt marker:** `[VESSEL | #16A7A0]`

The color is stable project identity. Use it in future Codex/director prompts, project dashboards, and cross-project coordination unless explicitly superseded.

## Mission

Vessel is an acquisition and text-materialization orchestrator for research corpora.

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

## ASR And External Tool Policy

- transcription remains behind a stable replaceable interface
- heavy ASR/alignment/diarization should be delegated to mature external tooling
- WhisperX is the target heavy ASR/diarization backend
- Phonon-2 may remain only where its lightweight QA role is materially useful
- Python tooling is acceptable behind an isolated subprocess boundary, preferably managed with uv
- temporary media remains disposable after successful materialization unless policy says otherwise
- ASR engine/model/tool provenance must be recorded in Sourcearium representation provenance

The transcription backend is replaceable without invalidating Sourcearium semantics.

The same principle applies to media acquisition: yt-dlp is the target first-class YouTube acquisition
backend, while Vessel retains authority over policy, reconciliation, provenance, and durable output.

## Toolchain Ownership Rule

Vessel should own domain semantics and delegate commodity or specialized machinery.

The default decision test is:

~~~text
does this code encode Vessel/Sourcearium semantics?
-> yes: Vessel may own it

does this code primarily reimplement a mature external tool's specialty?
-> yes: prefer an adapter/process boundary
~~~

Rust is the control-plane implementation language, not a mandate to rebuild YouTube, codecs, ASR,
diarization, or speaker-recognition infrastructure.

Cross-video named-speaker identity is not an active product goal.

See [Toolchain Migration Autopsy](toolchain-migration-autopsy-2026-10-03.md).

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

## Scope-Hijack Guard

Optional subsystems must not become de facto project priorities merely because their next technical problem is solvable.

Before extending an optional subsystem, the director must answer:

1. Which current core Vessel milestone does this unblock?
2. Can Vessel satisfy its corpus mission without it?
3. Is an existing external tool materially cheaper than bespoke implementation?
4. Is the next experiment bounded by a clear stopping condition?
5. Would the human principal still recognize and want to maintain Vessel after the change?

If the work does not materially advance durable corpus production, the default decision is **defer**.

A failed optional experiment does not automatically justify a more elaborate experiment. Re-evaluate whether the feature is needed before extending the research branch.

Rust-native implementation is a preference, not a justification for rebuilding an existing external capability when doing so would expand Vessel beyond its mission.

See [the 2026-10-03 speaker-identity scope-hijack postmortem](postmortem-speaker-identity-scope-hijack-2026-10-03.md).

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
