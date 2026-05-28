# Vessel Roadmap

`vessel` is a Rust-native port targeting `yt-dlp` feature parity without any runtime dependency on Python or the `yt-dlp` executable.

## Priority Order

1. Bootstrap the workspace, CLI, logging, config, and SQLite schema.
2. Build native YouTube metadata extraction.
3. Build the ledger path for videos, then channels.
4. Add download planning and artifact/archive persistence.
5. Add comments, subtitles, and thumbnails.
6. Close native YouTube parity gaps in comments and download hardening.
7. Expand format selection, postprocessing, and plugins without breaking the native-only runtime contract.

## Milestone Status

| Milestone | Title | Status |
| --- | --- | --- |
| [00](milestones/00-skeleton.md) | Skeleton | In Progress |
| [01](milestones/01-youtube-video-metadata.md) | YouTube Video Metadata | Completed |
| [02](milestones/02-dataset-ledger.md) | Dataset Ledger | Completed |
| [03](milestones/03-channel-tracking.md) | Channel Tracking | Completed |
| [04](milestones/04-basic-download.md) | Basic Download | Completed |
| [05](milestones/05-subs-thumbs-comments.md) | Subtitles, Thumbnails, Comments | Completed |
| [06](milestones/06-native-parity.md) | Native Parity | Completed |
| [07](milestones/07-format-selector-parity.md) | Format Selector Parity | Completed |
| [08](milestones/08-postprocessing.md) | Postprocessing | Completed |
| [09](milestones/09-plugin-system.md) | Plugin System | Planned |

## Parity Tiers

- Tier 1: native YouTube metadata, `info`, `formats`, `download`, subtitles, archive semantics.
- Tier 2: stronger YouTube completeness, automatic captions, livestream handling, PO token plumbing, and native direct-download coverage for ciphered watch-page formats.
- Tier 3: native support for more sites and generic extraction.
- Tier 4: option-level parity, plugin system, external downloader parity, postprocessor parity.

## Supporting Docs

- [Architecture](architecture.md)
- [Storage](storage.md)
- [Parity Matrix](parity-matrix.md)
- [Session Guide](session-guide.md)
