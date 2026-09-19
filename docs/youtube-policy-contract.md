# Sourcearium YouTube Policy Contract

## External Policy

Vessel consumes Sourcearium's stable YouTube source policy v1.

Authoritative Sourcearium files:

```text
schema/youtube-source-policy-v1.schema.json
docs/youtube-policy-v1.md
```

The policy lives in Sourcearium because it describes the desired research corpus, not Vessel execution state.

## Location

For each tracked YouTube channel:

```text
<sourcearium-root>/sources/youtube/<source-key>/source.toml
```

Vessel discovers configured channels by scanning those source directories.

## Minimal Valid Policy

```toml
schema = 1
family = "youtube"
source_key = "example"

[channel]
input = "https://www.youtube.com/@example"
```

## Policy Fields

### Channel

Required:

- `channel.input`

Optional:

- `channel.id`
- `channel.handle`

Vessel may use the optional stable ID to verify resolution.

Normal update must not rewrite Sourcearium policy merely because Vessel learned refreshed channel metadata.

### Selection

Optional:

- `published_on_or_after`
- `include_video_ids`
- `exclude_video_ids`

Selection algorithm:

1. explicit exclusion -> exclude
2. explicit inclusion -> include regardless of date cutoff
3. otherwise apply inclusive publication cutoff when configured
4. otherwise include

A video present in both include and exclude lists is invalid configuration. Vessel should reject/report the policy conflict.

If a cutoff exists and publication date remains unknown after normal metadata resolution, do not implicitly include the video. Report it as unresolved unless explicitly included.

### Transcripts

Optional:

- `enabled`
- `preferred_languages`
- `allow_creator_subtitles`
- `allow_auto_captions`
- `allow_local_asr`

Defaults when absent:

```text
enabled = true
allow_creator_subtitles = true
allow_auto_captions = true
allow_local_asr = true
preferred_languages = provider/source default
```

Among allowed providers, quality preference remains:

1. creator subtitles
2. platform automatic captions
3. local ASR

## Desired Set Versus Execution Throttle

Sourcearium policy defines **what should eventually exist**.

It must not be polluted with one-run operational settings.

Do not persist these in `source.toml`:

- max videos this run
- concurrency
- retries
- timeout
- last checked
- cursor
- temporary path
- ASR thread count

Those are Vessel CLI/config/cache concerns.

This distinction is required for deterministic desired-state reconciliation.

## Source Key

`source_key` is a user-controlled stable local research key.

It anchors the Sourcearium directory:

```text
sources/youtube/<source-key>/
```

Do not automatically rename it when channel title or handle changes.

## Validation

Before performing network work for a source, Vessel should:

1. parse `source.toml`
2. validate policy schema v1
3. enforce semantic validation not expressible in JSON Schema
4. reject include/exclude overlap
5. only then resolve/discover the upstream channel

Invalid policy must not cause partial corpus mutation.

## Compatibility

YouTube source policy v1 is frozen externally.

A grunt must not add convenient ad hoc fields to Sourcearium policy.

If new durable corpus-selection semantics are genuinely required, escalate to a new Sourcearium policy version.
