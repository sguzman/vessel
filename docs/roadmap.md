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

## Toolchain Migration — Adopted Direction

The project is beginning an architecture migration away from bespoke implementations of volatile or
specialized machinery.

Adopted target:

- yt-dlp becomes the first-class YouTube acquisition backend;
- FFmpeg/ffprobe remain external media plumbing;
- WhisperX becomes the preferred heavy ASR/alignment/diarization backend;
- Phonon-2 remains only if its lightweight QA niche continues to justify itself;
- Python tooling is isolated behind subprocess boundaries, preferably managed with uv;
- cross-video named-speaker identity and calibration work are removed from the active roadmap;
- Vessel continues to own policy, reconciliation, provenance, validation, operational state, and
  Sourcearium materialization.

Migration is replacement-first and deletion-second: prove each external adapter before retiring the
legacy native path, then delete superseded duplicate machinery rather than maintaining parallel stacks.

See [Toolchain Migration Autopsy](toolchain-migration-autopsy-2026-10-03.md).

## New Priority Order

1. Preserve Sourcearium contracts, reconciliation, provenance, validation, and non-destructive semantics.
2. Introduce a first-class yt-dlp acquisition adapter.
3. Prove discovery, metadata, creator-caption, auto-caption, and audio acquisition through that adapter.
4. Introduce a WhisperX subprocess adapter for heavy ASR/alignment/optional diarization.
5. Preserve a lightweight QA backend only where it materially reduces test cost.
6. Cut normal `vessel update` over to the external acquisition/transcription toolchain.
7. Verify idempotency, representation precedence, provenance, and atomic materialization after cutover.
8. Retire superseded native YouTube/download/ASR/diarization machinery.
9. Remove named-speaker identity/matching/calibration machinery from the maintained product.
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
- durable 16 kHz diarization fixtures are reused as ASR input when present, avoiding duplicate media downloads and normalization
- CLI overrides for ASR model, device, and language
- explicit `--force-local-asr` update override for bounded diagnostics/acceptance; it skips caption acquisition but still requires Sourcearium policy to allow local ASR

Implemented additionally:

- one Whisper model is loaded lazily and reused across ASR fallbacks in the same update run
- validator is available through `vessel validate` for offline corpus checks

Local QA / acceptance rule:

- when a test merely needs *some* local ASR output to exercise downstream Vessel behavior, use the lightest suitable backend: `phonon-2` for English fixtures
- do not spend Whisper compute on generic QA
- use `whisper-candle` or another heavier backend only when the test specifically targets that backend, multilingual behavior, or a quality characteristic that `phonon-2` cannot exercise
- tests that only exercise diarization, speaker matching, attribution, rendering, or persisted evidence must reuse existing audio/evidence and invoke no ASR at all

Real-source acceptance discovered and documented:

- hosted-runner YouTube anti-bot gate can block player metadata while channel crawl still succeeds
- Vessel now classifies this as an access/environment failure rather than generic missing metadata
- authentication/attestation is an explicit deferred design boundary, not a silent fallback

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
