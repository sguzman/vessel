# Milestone 05: Subtitles, Thumbnails, Comments

## Goal

Expand native metadata and artifact coverage for subtitles and thumbnails, while keeping comment support explicitly native-only and not yet implemented.

## Deliverables

- [x] `vessel video subtitles sync <video>`
- [ ] Native `vessel video comments sync <video>`
- [x] Thumbnail fetch and persistence
- [x] Optional channel sync integration flags for native subtitle and thumbnail flows
- [x] Explicit unsupported response for native comment sync

## Acceptance Criteria

- [x] `vessel video comments sync <video>` fails explicitly with a native-only unsupported response.
- [x] Subtitle tracks are normalized and persisted.
- [x] Thumbnail records are linked to the related video.
