# Milestone 06: yt-dlp Bridge

## Goal

Add an optional fallback backend that uses `yt-dlp` while native extractors mature.

## Deliverables

- [ ] `--backend ytdlp` selection
- [ ] Bridge path for `info`
- [ ] Bridge path for `download`
- [ ] Translation layer from bridge output to normalized models

## Acceptance Criteria

- [ ] The bridge can fetch metadata for unsupported native targets.
- [ ] Bridge downloads still update artifact and archive state.
- [ ] Native and bridge outputs are comparable in the parity test suite.
