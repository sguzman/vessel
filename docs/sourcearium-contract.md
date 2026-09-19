# Sourcearium Contract

## External Contract

Vessel materializes durable research text into Sourcearium.

The current target is:

```text
Sourcearium artifact schema v1
```

Sourcearium v1 is externally owned by `sguzman/sourcearium`. Vessel must conform to it rather than inventing a parallel artifact dialect.

The authoritative schema is:

```text
sourcearium/schema/sourcearium-v1.schema.json
```

## Required YouTube Transcript Mapping

A materialized YouTube transcript must map to the Sourcearium v1 core as follows:

```toml
schema = 1
artifact_id = "youtube:video:<video-id>:transcript"
kind = "transcript"
title = "<video title>"

[source]
family = "youtube"
kind = "video"
id = "<video-id>"
url = "<canonical watch URL>"
creator = "<channel display name>"
creator_id = "<stable channel id>"
published = "<best known publication date/time>"

[representation]
derivation = "<creator_subtitles | platform_auto_caption | local_asr>"
language = "<language>"
timestamps = true

[acquisition]
producer = "vessel"
method = "<platform_caption_fetch | local_asr>"
```

If timing is unavailable, `timestamps = false` is required rather than fabricated timestamps.

For `derivation = "local_asr"`, these Sourcearium v1 fields are required:

```toml
[representation]
engine = "<engine>"
model = "<model>"
```

## Provenance Layers

Do not collapse the Sourcearium layers.

- `source`: upstream media identity
- `representation`: textual representation stored
- `acquisition`: process that put it in Sourcearium

Vessel is an acquisition producer. It is not the upstream source or the author of the representation.

## Stable Artifact Identity

For YouTube transcripts:

```text
youtube:video:<video-id>:transcript
```

The artifact ID must not depend on:

- title
- channel display-name changes
- filesystem slug
- fetch date
- local database IDs

## Channel Provenance

When channel metadata is available during update:

- channel display title is written to `source.creator`
- stable channel ID is written to `source.creator_id`
- current handle may be written under `[extensions.youtube]` as `channel_handle`

The stable ID remains identity. Display title and handle are provenance/convenience and may change upstream.

## Recommended Path

```text
sources/youtube/<channel-key>/transcripts/<YYYY-MM-DD>__<video-id>__<slug>.md
```

The path is presentation/organization. The Sourcearium `artifact_id` is identity.

Do not rename an existing file merely because the upstream title changes unless a later explicit rename policy requires it.

## Deterministic Serialization

Vessel should emit deterministic Sourcearium artifacts.

Requirements:

- UTF-8 text
- TOML front matter delimited by `+++`
- LF line endings
- stable field/table ordering
- deterministic transcript segmentation/rendering
- no volatile refresh telemetry
- no random IDs
- no current-time field written on a no-op refresh

A no-op `vessel update` must generate byte-equivalent durable output or skip rewriting the file.

Mutable display metadata alone (for example, a renamed video title or channel handle) does not rewrite an existing artifact when the textual representation and its representation provenance are unchanged. Sourcearium is a text corpus, not a metadata time series.

## Acquisition Time

`acquisition.acquired_at` is optional.

If Vessel emits it, it records when the **current representation** was actually acquired/generated.

It must not be refreshed when Vessel merely checks the upstream source again.

When the representation is unchanged, retain the existing durable acquisition metadata.

## Extension Rule

Vessel must not add new Sourcearium v1 core fields.

YouTube-specific durable metadata belongs under:

```toml
[extensions.youtube]
```

Examples:

- channel handle
- caption-track identity
- platform-specific language/track labels

Extensions must not duplicate or override core semantics.

## Representation Preference

Default rank, strongest first:

1. `creator_subtitles`
2. `platform_auto_caption`
3. `local_asr`

Rules:

- missing artifact -> acquire best available representation
- stronger representation becomes available -> eligible replacement
- weaker representation becomes available -> do not downgrade automatically
- same-rank representation unchanged -> no durable write
- failed candidate generation/validation -> preserve existing artifact unchanged

## Atomic Replacement

Never partially overwrite a Sourcearium artifact.

Replacement flow:

1. acquire/generate complete candidate
2. normalize candidate
3. serialize deterministically
4. validate metadata against Sourcearium v1
5. validate transcript body invariants
6. compare against current durable artifact
7. write temporary file in destination filesystem
8. atomically replace final path

If any step fails before replacement, the existing artifact remains authoritative.

## Body Rendering

For timestamped transcripts:

```text
[00:00:03] First segment.

[00:00:08] Second segment.
```

Requirements:

- timestamps monotonic
- one rendered segment per paragraph
- timestamp format `[HH:MM:SS]`
- hours may exceed 23
- no fabricated timestamps
- no editorial rewriting of transcript text during materialization

Normalization may remove transport-only caption markup and normalize line endings/whitespace, but must not paraphrase the source.

## Operational State

The following stay in Vessel state and must not leak into Sourcearium durable files:

- last checked
- retry count
- request duration
- cursors
- fetch-run IDs
- temporary paths
- HTTP details
- local database primary keys

## Compatibility Rule

Sourcearium `schema = 1` is frozen.

If Sourcearium later publishes schema v2, Vessel support for v2 is an explicit migration/feature task.

A Codex/grunt session must not silently extend schema v1 because implementation would be easier.
