# Vessel Roadmap

`vessel` is a Rust-native port and extension of `yt-dlp`, with native YouTube extraction and an idempotent metadata ledger as the initial priority.

## Priority Order

1. Bootstrap the workspace, CLI, logging, config, and SQLite schema.
2. Build native YouTube metadata extraction.
3. Build the ledger path for videos, then channels.
4. Add download planning and artifact/archive persistence.
5. Add comments, subtitles, and thumbnails.
6. Add `yt-dlp` bridge compatibility while native extractors mature.
7. Close parity gaps in format selection, postprocessing, and plugins.

## Milestone Status

| Milestone | Title | Status |
| --- | --- | --- |
| [00](milestones/00-skeleton.md) | Skeleton | In Progress |
| [01](milestones/01-youtube-video-metadata.md) | YouTube Video Metadata | Completed |
| [02](milestones/02-dataset-ledger.md) | Dataset Ledger | Completed |
| [03](milestones/03-channel-tracking.md) | Channel Tracking | Completed |
| [04](milestones/04-basic-download.md) | Basic Download | Planned |
| [05](milestones/05-subs-thumbs-comments.md) | Subtitles, Thumbnails, Comments | Planned |
| [06](milestones/06-ytdlp-bridge.md) | yt-dlp Bridge | Planned |
| [07](milestones/07-format-selector-parity.md) | Format Selector Parity | Planned |
| [08](milestones/08-postprocessing.md) | Postprocessing | Planned |
| [09](milestones/09-plugin-system.md) | Plugin System | Planned |

## Parity Tiers

- Tier 1: native YouTube metadata, `info`, `formats`, `download`, subtitles, archive semantics.
- Tier 2: stronger YouTube completeness, comments, automatic captions, livestream handling, PO token plumbing.
- Tier 3: native support for more sites and generic extraction.
- Tier 4: option-level `yt-dlp` parity, plugin system, external downloader parity, postprocessor parity.

## Supporting Docs

- [Architecture](architecture.md)
- [Storage](storage.md)
- [Parity Matrix](parity-matrix.md)
- [Session Guide](session-guide.md)
