# Session Guide

## Recommended Session Order

1. Finish Milestone 00 until the CLI and schema are stable.
2. Implement native YouTube `info` for a single video.
3. Persist video snapshots and video history queries.
4. Add tracked channels and channel sync.
5. Add download planning and file transaction semantics.
6. Add comments, subtitles, and thumbnails.
7. Add bridge compatibility and close parity gaps.

## Definition Of Done

- A milestone is done when its acceptance checklist is complete and the relevant command path is no longer a stub.
- Schema changes must preserve the current-vs-snapshots distinction.
- New behavior should add tests for idempotency or compatibility when applicable.

## Future Session Rules

- Keep extraction, ledger, and download changes separated by crate boundary.
- Do not bypass the event model with ad hoc logging once real execution paths are added.
- Treat bridge support as temporary compatibility, not the long-term architecture center.
