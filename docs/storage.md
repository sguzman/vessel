# Storage Model

## Two Storage Domains

Vessel distinguishes operational state from durable research material.

### Operational Vessel state

Replaceable state used to make acquisition reliable and incremental.

Examples:

- SQLite database
- fetch attempts
- source discovery cache
- cursors
- download archive
- content hashes
- temporary audio/video
- retry/error state

### External corpus state

Durable user-owned research material lives in Sourcearium.

Vessel currently targets **Sourcearium artifact schema v1**.

See [Sourcearium Contract](sourcearium-contract.md).

Examples of durable corpus content:

- transcript body
- upstream source identity
- representation provenance
- acquisition provenance
- timestamp mappings

The corpus must remain inspectable and useful without opening Vessel's SQLite database.

## Sourcearium-Local Operational State

When `vessel update` operates directly on a Sourcearium checkout, its disposable state lives under the repository's ignored cache tree:

```text
<sourcearium>/.cache/vessel/
├── vessel.sqlite
└── asr/
    └── <video-id>/
```

The SQLite database stores crawl cursors, discovered backlog, transcript upgrade-probe timestamps, and other acquisition memory. It is explicitly not corpus authority.

Deleting `.cache/vessel/` must not delete or invalidate existing Sourcearium artifacts; it only makes future acquisition more expensive because Vessel must rediscover state.

A completed YouTube backfill still refreshes each tab's initial page on later runs so new uploads are noticed, while old continuation pages are not repeatedly traversed.

## Operational Layout

Existing project state may continue to live under:

```text
.cache/vessel/<project>/
├── vessel.sqlite
├── vessel.toml
├── downloads/
├── thumbnails/
├── subtitles/
└── plugins/
```

This layout is implementation state, not the canonical research corpus.

## Sourcearium Layout

Recommended YouTube transcript destination:

```text
<sourcearium-root>/
└── sources/
    └── youtube/
        └── <channel-key>/
            └── transcripts/
                └── <YYYY-MM-DD>__<video-id>__<slug>.md
```

The filesystem path is not canonical identity. Each artifact carries a stable Sourcearium `artifact_id`.

## Sourcearium v1 Metadata

A YouTube transcript maps to:

```toml
schema = 1
artifact_id = "youtube:video:abc123:transcript"
kind = "transcript"
title = "Example title"

[source]
family = "youtube"
kind = "video"
id = "abc123"
url = "https://www.youtube.com/watch?v=abc123"
creator = "Example Channel"
creator_id = "UCexample"
published = "2026-09-18"

[representation]
derivation = "creator_subtitles"
language = "en"
timestamps = true

[acquisition]
producer = "vessel"
method = "platform_caption_fetch"
```

For local ASR, `representation.engine` and `representation.model` are required.

Do not add ad hoc Sourcearium core fields from Vessel. Source-specific extras use `[extensions.youtube]`.

## Timestamped Body

Canonical readable representation:

```text
[00:00:03] First segment.

[00:00:09] Second segment.
```

Raw provider payloads may be cached in Vessel when useful, but the Sourcearium artifact is readable text.

## Deterministic Output

Sourcearium writes should be deterministic.

A no-op update must not create a durable diff.

Do not rewrite corpus files merely because Vessel checked them again.

Fields such as:

- last_checked_at
- retry count
- HTTP timing
- fetch-run ID

belong in operational state.

If `acquisition.acquired_at` is emitted, preserve it while the current representation remains unchanged.

## Retention

Successful ASR should normally follow:

```text
download temporary audio
        |
        v
transcribe
        |
        v
validate Sourcearium candidate
        |
        v
atomically materialize text
        |
        v
delete temporary audio
```

Users may later opt into media retention, but retaining media is not required for the default transcript-corpus workflow.

## Replacement Safety

A new candidate must not damage an existing durable artifact.

Before replacement:

- candidate acquisition/transcription completes
- Sourcearium v1 metadata validates
- transcript body validates
- candidate is stronger or meaningfully changed under policy

Write atomically.

On failure, preserve the old artifact unchanged.

## Non-Destructive Reconciliation

Normal `update` never removes an existing artifact simply because:

- selection policy became narrower
- a channel/video disappeared upstream
- a source became private
- an extractor can no longer rediscover it

Removal requires explicit prune behavior.

Git history is additional recovery protection, not justification for destructive defaults.

## Existing SQLite History

The current codebase contains current-state and historical metadata tables from the original project direction.

They may remain for compatibility and operational usefulness.

Future development should not expand historical metadata collection unless a concrete corpus need requires it.
