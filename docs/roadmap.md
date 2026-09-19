# Vessel Roadmap

## Scope Reset

Vessel originally targeted broad Rust-native `yt-dlp` parity plus historical local metadata datasets.

That work produced substantial reusable infrastructure. It is no longer the product north star.

Current priority:

> make selected media-derived text easy to maintain as a durable, provenance-preserving corpus.

## Legacy Capability Baseline

Already implemented and retained as useful substrate:

- native YouTube video metadata extraction
- channel extraction and backlog crawling
- local SQLite state
- channel sync
- basic native download
- subtitles
- automatic-caption metadata
- comments
- thumbnails
- format selection
- resume/archive behavior
- FFmpeg merge/remux/audio extraction
- metadata/thumbnail embedding
- subtitle conversion
- plugin system
- native YouTube signature/cipher download hardening

These capabilities should not force future work to continue the old parity campaign.

## New Priority Order

1. Define corpus contract and scope invariants.
2. Define a small declarative source-selection policy.
3. Implement desired-set reconciliation behind `vessel update`.
4. Implement transcript-provider precedence.
5. Materialize stable human-readable transcript artifacts into an external corpus path/repository.
6. Add CPU-first local ASR fallback.
7. Make update idempotent and non-destructive.
8. Add explicit prune/dry-run behavior.
9. Harden provenance, replacement rules, and transcript verification.
10. Generalize media-source support only where it materially improves corpus acquisition.

## New Milestones

| Milestone | Title | Status |
| --- | --- | --- |
| 10 | Corpus Charter + Contract | Completed |
| 11 | Corpus Policy | Planned |
| 12 | `vessel update` Reconcile Loop | Planned |
| 13 | Transcript Provider Chain | Planned |
| 14 | Corpus Materializer | Planned |
| 15 | Local ASR Fallback | Planned |
| 16 | Safe Prune + Replacement Semantics | Planned |
| 17 | Provenance Hardening | Planned |
| 18 | Additional Media Sources | Deferred |

## Milestone 11: Corpus Policy

Initial supported policy:

- named source/channel
- publication cutoff
- explicit include IDs
- explicit exclude IDs
- ASR fallback enabled/disabled

Avoid title-regex/query-language complexity until real usage requires it.

## Milestone 12: Update Reconcile Loop

Target:

```bash
vessel update
```

Acceptance intent:

- discover selected upstream material
- compare against already materialized corpus artifacts
- acquire missing items
- avoid duplicate work
- avoid Git churn when nothing meaningful changed
- never delete existing corpus material

## Milestone 13: Transcript Provider Chain

Default precedence:

1. creator subtitles
2. platform automatic captions
3. local ASR

Provider provenance must survive normalization.

## Milestone 14: Corpus Materializer

Emit Git-friendly files with:

- stable source identity
- title/date/source URL
- provenance metadata
- transcript language
- timestamped segments
- normalized readable text

The corpus path is external to Vessel's operational cache.

## Milestone 15: Local ASR

Requirements:

- CPU-first normal operation
- optional acceleration
- Rust-native/Rust-facing backend
- backend abstraction
- temporary audio cleanup after success
- engine/model provenance captured in output

## Milestone 16: Safe Prune

`update` is non-destructive.

Explicit cleanup:

```bash
vessel prune --dry-run
vessel prune
```

Upstream deletion or policy narrowing must not silently erase acquired research material.

## Deprioritized Work

The following are not current success criteria:

- option-level `yt-dlp` parity
- external downloader parity
- analytics dashboards
- exhaustive metadata history
- subscriber/view time-series collection
- PostgreSQL support
- generic non-media text acquisition

They may be revisited only if they directly support the corpus mission.
