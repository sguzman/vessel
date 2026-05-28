# Session Guide

## Recommended Session Order

1. Finish Milestone 00 until the CLI and schema are stable.
2. Implement native YouTube `info` for a single video.
3. Persist video snapshots and video history queries.
4. Add tracked channels and channel sync.
5. Add download planning and file transaction semantics.
6. Add native subtitles, thumbnails, and first-pass native comments.
7. Harden native YouTube downloads and broaden native comment pagination.
8. Complete baseline format selector parsing and evaluation parity.
9. Add FFmpeg-backed merge, remux, audio extraction, metadata/thumbnail embedding, and subtitle conversion.

## Definition Of Done

- A milestone is done when its acceptance checklist is complete and the relevant command path is no longer a stub.
- Schema changes must preserve the current-vs-snapshots distinction.
- New behavior should add tests for idempotency or compatibility when applicable.
- Unsupported features must fail explicitly and may not shell out to Python or `yt-dlp`.

## Future Session Rules

- Keep extraction, ledger, and download changes separated by crate boundary.
- Do not bypass the event model with ad hoc logging once real execution paths are added.
- Preserve the native-only runtime contract even when parity gaps remain.
