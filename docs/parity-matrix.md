# Parity Matrix

## Native Milestones

- [x] Workspace scaffold
- [x] Native CLI command tree
- [x] Config loading
- [x] Logging bootstrap
- [x] SQLite schema bootstrap
- [x] Native YouTube video metadata extraction
- [ ] Video snapshot history
- [ ] Channel tracking and sync
- [ ] Basic native download path
- [ ] Subtitles
- [ ] Thumbnails
- [ ] Comments

## Downloader And Postprocessing

- [ ] Format selector parser for baseline expressions
- [ ] Output template rendering
- [ ] Resume and partial file handling
- [ ] Artifact persistence
- [ ] Archive semantics
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

- [ ] Re-running channel sync does not duplicate snapshots.
- [ ] Metadata changes create exactly one new snapshot.
- [ ] Failed fetches preserve prior latest state.
- [ ] Deleted/private states preserve old metadata.
- [ ] Download resume keeps archive and artifacts consistent.
- [ ] Format selection matches documented baseline expressions.
