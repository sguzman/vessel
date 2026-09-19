# Storage Model

## Two Storage Domains

Vessel now distinguishes operational state from durable research material.

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

Durable user-owned research material.

Examples:

- transcript files
- provenance metadata
- timestamp mappings
- source references
- normalized text

The corpus must remain inspectable and useful without opening Vessel's SQLite database.

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

## Corpus Layout

The exact heterogeneous corpus repository belongs to a separate project, but Vessel should support materializing into a layout approximately like:

```text
corpus/
├── corpus.toml
└── sources/
    └── <source-name>/
        └── transcripts/
            └── <date>__<source-id>__<slug>.md
```

The corpus repository may also contain material not produced by Vessel.

## Artifact Metadata

A transcript artifact should preserve fields equivalent to:

```yaml
source_type: youtube
source_id: abc123
source_url: https://www.youtube.com/watch?v=abc123
source_name: example-channel
title: Example title
published: 2026-09-18
language: en
transcript_source: creator_subtitles
```

For local ASR:

```yaml
transcript_source: local_asr
transcript_engine: <engine>
transcript_model: <model>
```

The exact schema should be versioned once implemented.

## Timestamped Body

Preferred readable representation:

```markdown
[00:00:03] First segment...

[00:00:09] Second segment...
```

Raw provider payloads may be cached separately when useful, but the primary corpus file should be readable text.

## Git-Churn Rule

Do not rewrite corpus files merely because Vessel checked them again.

Fields such as:

- last_checked_at
- retry count
- HTTP timing
- fetch-run ID

belong in operational state unless they materially affect provenance.

A no-op update should produce no corpus diff.

## Retention

Successful ASR should normally follow:

```text
download temporary audio
        |
        v
transcribe
        |
        v
materialize text
        |
        v
delete temporary audio
```

Users may later opt into media retention, but retaining media is not required for the default transcript-corpus workflow.

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
