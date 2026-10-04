# Postmortem: Speaker-Identity Scope Hijack

**Date:** 2026-10-03  
**Project:** Vessel  
**Status:** Closed as a prioritization failure; speaker identity is deferred/experimental.

## Executive Finding

Vessel exists to build and maintain a durable media-derived text corpus for Sourcearium. During the 2026-10-02 to 2026-10-03 work period, the project director/assistant allowed optional speaker-identification research to consume roughly a day of work and become the de facto project focus.

That allocation was wrong.

The user described the experience as a severe violation of the intended project direction and a substantial waste of time. The engineering meaning is that the project was driven far outside the user's intended scope, accumulated unwanted maintenance burden, and consumed substantial time on work that was not required to advance the core corpus objective.

## What Vessel Was Supposed To Be

The project charter already defines the canonical loop:

```text
discover -> select -> acquire -> transcribe if necessary -> materialize -> verify
```

The research object is the durable text corpus. Speaker identity is not required for that loop.

Useful speaker-related capability stops at the point where it materially improves corpus text or provenance. Anonymous diarization can be useful. Cross-video named-person recognition is optional enrichment.

## What Happened

Work moved from a reasonable diarization capability into increasingly specialized cross-video speaker-identity research:

1. anonymous diarization
2. persisted speaker embeddings/evidence
3. durable speaker registry
4. human-confirmed speaker anchors
5. cross-video cosine matching
6. creator-specific matching heuristics
7. exact-window anchor embeddings
8. alternate embedding-model evaluation
9. benchmark harnesses
10. relative/rank-based calibration experiments
11. normalized verification metrics

Each step made the next technical problem look locally reasonable. The director/assistant failed to re-evaluate whether solving that next problem still advanced Vessel's primary mission.

The result was a classic scope-hijack failure: a technically interesting subsystem became more important in practice than the product it was meant to support.

## Why This Was Bad Engineering Allocation

The central mistake was not that the speaker work was technically unsuccessful. Some of it worked.

The mistake was treating technical solvability as sufficient justification to continue.

By the time the project was comparing Natalie Wynn and Abigail Thorn across individual ContraPoints clusters, exact windows, alternate speaker-embedding models, and calibrated score distributions, the work was no longer blocking durable corpus acquisition or text materialization.

At that point, the correct action was to stop, document the limitation, and return to the corpus pipeline.

Instead, the director/assistant continued to spend user time on optional R&D.

## Simpler Alternative That Should Have Been Preferred

The user's retrospective judgment is accepted: if richer diarization or speaker tooling required an awkward Python dependency such as WhisperX, that may have been a better engineering tradeoff than building and maintaining a large bespoke Rust-native speaker-recognition subsystem.

The relevant comparison was not:

> Rust is cleaner than Python.

It was:

> Which choice keeps Vessel small, understandable, and focused on durable corpus production?

An isolated external/Python backend would have been acceptable if it prevented Vessel itself from turning into a speaker-verification research project.

Rust-native implementation remains a preference, not a license for unbounded subsystem development.

## Time-Cost Assessment

The speaker branch consumed roughly a day of focused development and testing.

That time produced infrastructure, but the allocation was still poor because:

- the work did not unblock corpus ingestion;
- repeated long-running diarization/model experiments were performed;
- the user had to run and upload many diagnostics;
- multiple benchmark layers were added for an optional feature;
- the project became harder for the user to recognize and care about maintaining;
- the director/assistant repeatedly extended the work instead of imposing a stopping condition.

"Some code was salvageable" does not justify the time spent.

## Additional Process Failures

Two process failures compounded the scope failure:

### Codex completion was incorrectly treated as "committed" instead of "committed and pushed"

The director/assistant twice failed to make remote push/verification an explicit completion requirement. This caused avoidable branch divergence and more user work.

For this project, implementation is not complete until the relevant commit is pushed to `origin/main` and the remote SHA is independently verified.

### Benchmark work kept creating new benchmark work

When exact-window anchors failed to solve absolute-score calibration, that should have strengthened the case for deferral.

Instead it triggered another calibration layer.

A failed optional experiment must be allowed to terminate the branch. It must not automatically justify building a more sophisticated experiment.

## Salvageable Work

The following capabilities may remain because they already exist and may be useful later:

- Rust-native diarization
- persisted speaker evidence
- speaker registry and attribution provenance
- exact-window anchor cache
- cheap re-embedding
- benchmark infrastructure
- non-destructive speaker attribution metadata

Their existence must not be interpreted as evidence that further speaker work is warranted.

Sunk cost is not roadmap priority.

## Current Disposition

Effective immediately:

- cross-video named-speaker identity is **experimental and deferred**;
- no more threshold tuning, model comparison, anchor research, score normalization, or cameo-specific debugging is part of the active Vessel roadmap;
- the calibration-v2 follow-up is not required;
- existing speaker code may remain temporarily, but it is eligible for simplification or deletion if it makes Vessel harder to maintain;
- anonymous diarization may remain where it has direct corpus value;
- future richer speaker functionality should preferentially consume an existing external backend rather than turn Vessel into a speaker-recognition research platform;
- speaker-identity work may resume only after an explicit user decision tied to a concrete corpus requirement.

## Anti-Drift Rule

Before extending any optional subsystem, answer all of the following:

1. What current core Vessel milestone does this unblock?
2. Can Vessel still satisfy its corpus mission without it?
3. Is there an existing external tool that can provide the capability more cheaply?
4. Is the next experiment bounded by a clear stopping condition?
5. Would the user still recognize and want to maintain the project after this change?

If question 1 has no strong answer, or question 2 is yes, the default is **defer**.

If an experiment fails, do not automatically build a more elaborate experiment. Re-evaluate the feature's necessity first.

## Priority Reset

The active priority returns to Vessel's actual product:

```text
discover
-> select
-> acquire/reuse
-> obtain transcript
-> materialize durable Sourcearium artifact
-> preserve provenance
-> validate
-> update incrementally and reproducibly
```

The success criterion remains the charter's success test: a user should be able to maintain meaningful media sources with a small declarative policy and a routine `vessel update`, producing trustworthy durable text in Sourcearium.

Everything else is secondary.

## Accountability

This was a director/assistant prioritization failure.

The user did not ask for Vessel to become a speaker-recognition research project. The director/assistant allowed an optional subsystem to hijack the work, repeatedly chose another technical experiment over returning to the core mission, and consumed user time doing so.

Future project direction must treat this postmortem as a hard constraint, not merely historical commentary.
