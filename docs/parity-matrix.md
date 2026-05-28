# Parity Matrix

## Native Milestones

- [x] Workspace scaffold
- [x] Native CLI command tree
- [x] Config loading
- [x] Logging bootstrap
- [x] SQLite schema bootstrap
- [x] Native YouTube video metadata extraction
- [x] Video snapshot history
- [x] Channel tracking and sync
- [x] Basic native download path
- [x] Subtitles
- [x] Thumbnails
- [x] Comments

## Downloader And Postprocessing

- [ ] Format selector parser for baseline expressions
- [x] Output template rendering
- [x] Resume and partial file handling
- [x] Artifact persistence
- [x] Archive semantics
- [ ] FFmpeg merge/remux
- [ ] Audio extraction
- [ ] Metadata embedding
- [ ] Thumbnail embedding
- [ ] Subtitle conversion

## Compatibility Milestones

- [ ] `yt-dlp` bridge backend
- [ ] Additional native extractors beyond YouTube
- [ ] Option-level CLI parity campaign
- [ ] Plugin API and plugin management
- [ ] External downloader parity

## Regression Targets

- [x] Re-running channel sync does not duplicate snapshots.
- [x] Metadata changes create exactly one new snapshot.
- [ ] Failed fetches preserve prior latest state.
- [ ] Deleted/private states preserve old metadata.
- [x] Download resume keeps archive and artifacts consistent.
- [ ] Format selection matches documented baseline expressions.
