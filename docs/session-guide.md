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

Make Vessel a reliable media-to-text materialization engine for maintaining external research corpora.

Do not optimize a task for broad `yt-dlp` parity unless the task explicitly requires it.

## Session Start Checklist

Before implementation:

1. Read `docs/charter.md`.
2. Read the relevant roadmap milestone.
3. Identify whether the task changes durable corpus semantics or only operational state.
4. State the bounded implementation target.
5. Preserve unrelated working capabilities.

## Engineering Rules

- Prefer Rust-native implementation.
- No silent Python/`yt-dlp` fallback for supported native behavior.
- Preserve provenance.
- Keep corpus output readable without Vessel.
- Keep operational refresh state out of durable artifacts unless meaningful.
- Repeated runs should be idempotent.
- Normal update must be non-destructive.
- Do not introduce a generalized DSL where a small explicit schema is enough.
- Do not create new time-series collection merely because fields are available.
- Tests should target reconciliation, idempotency, provenance, and safe failure.

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

## Current Next Work

The highest-value next implementation sequence is:

1. corpus policy schema
2. corpus artifact format
3. transcript-provider abstraction
4. `vessel update` reconciliation skeleton
5. subtitle acquisition integration
6. local ASR fallback
7. safe prune semantics
