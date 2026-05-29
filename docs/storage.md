# Storage Model

## Project Layout

- Default local state lives under `.cache/vessel/<project>/`.
- Each project gets its own SQLite database at `.cache/vessel/<project>/vessel.sqlite`.
- Default local artifact roots:
  - `downloads/`
  - `thumbnails/`
  - `subtitles/`
  - `plugins/`
- `vessel project list` enumerates known project folders under `.cache/vessel/`.
- Artifact paths stored in SQLite are project-root-relative so the dataset stays portable within the project folder.
- SQLite remains plain on-disk SQLite, so tools such as `litecli` can open a project directly:
  - `litecli .cache/vessel/<project>/vessel.sqlite`

## State Layers

- Current tables expose the latest known state for fast queries.
- Snapshot tables preserve historical states for diffing and auditing.
- Raw JSON stays alongside normalized JSON so extractor evolution does not lose data.

## Initial Tables

- `channels`
- `channel_snapshots`
- `videos`
- `video_snapshots`
- `fetch_runs`
- `fetch_attempts`
- `artifacts`
- `download_archive`

## Snapshot Rules

- [x] Latest state lives in current tables.
- [x] Historical state lives in snapshot tables.
- [x] Unchanged fetches do not create duplicate snapshots.
- [x] Each fetch still records a run or attempt entry.
- [x] Snapshot hashes use normalized JSON plus BLAKE3.

## Idempotent Sync Semantics

1. Fetch current metadata.
2. Normalize it into typed models.
3. Serialize to canonical JSON and compute a BLAKE3 content hash.
4. Compare against the latest stored hash.
5. Insert a snapshot only when the hash changed.
6. Upsert the current-state row every run so `last_seen_at` advances.
7. Record fetch attempts regardless of change status.

## Freshness Policy

The first bootstrap only documents policy. Implementation comes later.

- Channels: default stale threshold of 24h.
- Recent videos: shorter stale threshold than older videos.
- Livestreams/upcoming videos: aggressive refresh.
- Comments and subtitles: optional, slower refresh cadence by default.
