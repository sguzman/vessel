# Vessel Roadmap

## Scope Reset

Vessel originally targeted broad Rust-native `yt-dlp` parity plus historical local metadata datasets.

That work produced substantial reusable infrastructure. It is no longer the product north star.

Current priority:

> make selected media-derived text easy to maintain as durable, provenance-preserving Sourcearium artifacts.

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

1. Define external Sourcearium contracts.
2. Implement transcript-provider abstraction.
3. Implement Sourcearium v1 serializer + validator.
4. Implement desired-set reconciliation behind `vessel update`.
5. Integrate existing subtitle acquisition into the provider chain.
6. Add CPU-first local ASR fallback.
7. Make update idempotent and non-destructive.
8. Add explicit safe prune behavior. Preview is already implemented separately from pruning.
9. Harden provenance, replacement rules, and transcript verification.
10. Generalize media-source support only where it materially improves corpus acquisition.

## New Milestones

| Milestone | Title | Status |
| --- | --- | --- |
| 10 | Sourcearium Artifact Contract | Completed |
| 11 | Sourcearium YouTube Policy | Completed |
| 12 | Transcript Provider Abstraction | Completed |
| 13 | Sourcearium Serializer + Validator | Completed |
| 14 | `vessel update` Reconcile Loop | In Progress |
| 15 | Subtitle Provider Integration | Completed |
| 16 | Local ASR Fallback | In Progress |
| 17 | Safe Prune + Replacement Semantics | Completed |
| 18 | Additional Media Sources | Deferred |

## Milestone 10: Sourcearium Artifact Contract

Completed:

- Sourcearium artifact schema v1 frozen externally
- exact YouTube transcript field mapping documented
- provenance layers separated into source / representation / acquisition
- stable artifact identity defined
- deterministic serialization requirement defined
- atomic candidate validation/replacement requirement defined

See [Sourcearium Contract](sourcearium-contract.md).

## Milestone 11: Sourcearium YouTube Policy

Completed:

- policy owned by Sourcearium
- one `source.toml` per YouTube source directory
- stable local `source_key`
- inclusive publication cutoff
- explicit include IDs
- explicit exclude IDs
- transcript provider permissions
- execution throttles excluded from durable policy
- include/exclude conflict defined as invalid

See [Sourcearium YouTube Policy Contract](youtube-policy-contract.md).

## Milestone 12: Transcript Provider Abstraction

Status: **Completed**

Implemented one internal transcript resolution interface that can represent:

- creator subtitles
- platform automatic captions
- local ASR

It must return normalized transcript data plus enough provenance to materialize Sourcearium v1.

Do not make Sourcearium serialization depend directly on YouTube response structs.

## Milestone 13: Sourcearium Serializer + Validator

Status: **Completed**

Implemented deterministic artifact serialization and validation against Sourcearium v1.

Acceptance intent:

- stable TOML field order
- stable body rendering
- no-op serialization byte-equivalent
- local ASR requires engine/model
- transcript timestamp presence explicit
- validate before replacement

## Milestone 14: Update Reconcile Loop

Status: **In Progress**

Implemented:

- Sourcearium YouTube policy discovery and validation
- operational SQLite under `.cache/vessel/vessel.sqlite`
- per-tab YouTube cursor reuse
- completed backfills refresh first pages without recrawling full history
- discovered channel/video membership stays operational rather than entering corpus files
- membership table is used as a persistent processing backlog, so `--max-videos` cannot strand older discovered videos
- channel discovery order is preserved instead of sorting by opaque video IDs, making limited runs process the upstream order predictably
- cursor advancement is gated on durable membership persistence, so an operational-state failure causes safe rediscovery instead of silent loss
- upgrade probes for weaker existing transcripts are operationally throttled (30 days by default) instead of refetching every historical watch page on every run
- exact resolved publication dates are cached as replaceable operational state, so date-cutoff exclusions do not require repeated watch-page fetches
- no operational cursor state is written to Sourcearium policy or artifacts
- offline corpus inventory through `vessel inventory`
- opt-in per-video decision reporting through `--report-items`
- non-materializing preview mode through `--preview`; preview skips corpus writes and local ASR while allowing disposable cache warming

Target:

```bash
vessel update
```

Acceptance intent:

- scan Sourcearium YouTube policies
- validate policies before network work
- discover desired video sets
- compare desired set against materialized artifact identities
- acquire missing/upgradeable representations
- never delete material during update
- no Git churn on no-op runs

## Milestone 15: Subtitle Provider Integration

Status: **Completed**

Existing subtitle/caption extraction is normalized behind the transcript-provider path.

Provider precedence:

1. creator subtitles
2. platform automatic captions
3. local ASR

## Milestone 16: Local ASR

Status: **In Progress**

Implemented:

- dedicated `vessel-asr` crate
- pure-Rust `whisper-candle-core` backend
- CPU-first default
- multilingual `small` default model
- ASR result normalization into `TranscriptCandidate`
- engine/model provenance for Sourcearium v1
- backend kept outside Sourcearium and YouTube-specific layers

Implemented additionally:

- `vessel update` invokes ASR when platform captions are unavailable and policy allows it
- existing `bestaudio` download planning reused for temporary media
- ASR input normalized to 16 kHz mono PCM WAV
- temporary ASR media retained on failure and deleted after successful materialization
- CLI overrides for ASR model, device, and language

Implemented additionally:

- one Whisper model is loaded lazily and reused across ASR fallbacks in the same update run
- validator is available through `vessel validate` for offline corpus checks

Remaining before calling the ASR path proven:

- real-world local-ASR smoke test against a configured Sourcearium video

Optional acceleration is deferred until CPU behavior is proven and performance justifies it.

## Milestone 17: Safe Prune

Status: **Completed**

`update` remains non-destructive.

Offline cleanup is explicit:

```bash
vessel prune
vessel prune --apply
```

Semantics:

- bare `vessel prune` is plan-only
- `--apply` is required for deletion
- only configured YouTube source directories are considered
- explicit `exclude_video_ids` can produce prune candidates
- a known artifact publication date before `published_on_or_after` can produce a prune candidate
- explicit include still overrides the date cutoff
- missing/uncertain publication dates are preserved
- disabled transcript acquisition does not imply deletion
- removed/missing source policy does not imply deletion
- upstream disappearance/private state never implies deletion
- artifact identity is revalidated immediately before removal
- candidate paths are canonicalized and must remain inside Sourcearium's `sources/` tree

This keeps deletion policy local, inspectable, and independent of volatile upstream availability.

## Deprioritized Work

Not current success criteria:

- option-level `yt-dlp` parity
- external downloader parity
- analytics dashboards
- exhaustive metadata history
- subscriber/view time-series collection
- PostgreSQL support
- generic non-media text acquisition
