# Capability Matrix

## Current Mission

The success criterion is corpus materialization, not global `yt-dlp` parity.

## Existing Media Capabilities

- [x] Rust workspace and native CLI
- [x] Config loading
- [x] Logging
- [x] SQLite operational storage
- [x] Native YouTube video metadata extraction
- [x] Channel discovery/tracking
- [x] Native download path
- [x] Subtitles
- [x] Automatic-caption metadata
- [x] Thumbnails
- [x] Comments
- [x] Format selection
- [x] Resume and partial-file handling
- [x] Archive semantics
- [x] FFmpeg merge/remux
- [x] Audio extraction
- [x] Metadata embedding
- [x] Thumbnail embedding
- [x] Subtitle conversion
- [x] Plugin API
- [x] YouTube comment pagination
- [x] Signature/cipher-protected native YouTube download support

## Corpus Mission Capabilities

- [x] Project charter and scope boundary
- [ ] External corpus policy schema
- [ ] Publication cutoff selection
- [ ] Explicit include list
- [ ] Explicit exclude list
- [ ] Desired-set reconciliation
- [ ] `vessel update`
- [ ] Creator-subtitle preference
- [ ] Platform-auto-caption fallback
- [ ] Local ASR fallback
- [ ] CPU-first ASR default
- [ ] Stable transcript provenance schema
- [ ] Git-friendly transcript materialization
- [ ] No-op update produces no corpus diff
- [ ] Explicit dry-run prune
- [ ] Explicit prune
- [ ] Upstream deletion/private state preserves local text

## Still Useful But Deprioritized Compatibility Work

- [ ] Additional native media extractors beyond YouTube
- [ ] Option-level `yt-dlp` parity
- [ ] External downloader parity

These items are not blockers for the corpus mission.

## Regression Invariants

- [x] Existing channel sync does not duplicate unchanged historical snapshots.
- [x] Download resume keeps archive and artifacts consistent.
- [ ] Corpus update is idempotent.
- [ ] Failed acquisition preserves previously materialized text.
- [ ] Better transcript replacement preserves provenance in Git history.
- [ ] Policy narrowing never causes implicit deletion.
