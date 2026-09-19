# Vessel

**Project color:** Vessel Wake Teal `#16A7A0`

Vessel is a Rust-native media acquisition and text-materialization engine for building durable research corpora.

Its primary job is no longer to chase `yt-dlp` feature parity or maintain metadata time series. The existing YouTube extractor, downloader, subtitle, comment, and SQLite machinery remains valuable, but future development is organized around a simpler research workflow:

```text
discover -> select -> materialize text -> preserve provenance
```

The intended user experience is eventually:

```bash
vessel update
```

Given declarative source policy, Vessel discovers matching media, prefers upstream text when available, falls back to local transcription when necessary, and materializes stable text artifacts into an external corpus repository.

See [Project Charter](docs/charter.md) for the governing scope.

## Primary Objective

Vessel should make it easy to maintain a local, Git-versioned corpus of selected media sources.

For YouTube, a corpus policy may say:

- track these channels
- ignore videos before a date cutoff
- explicitly include these videos
- explicitly exclude these videos
- prefer creator subtitles
- otherwise use platform automatic captions
- otherwise download audio temporarily and transcribe it locally

The durable output is text plus provenance. Media downloads and operational metadata are supporting machinery.

## Separation Of Responsibilities

Vessel is the engine. The corpus is a separate repository.

```text
Vessel
  - discovers upstream sources
  - applies source-selection policy
  - acquires subtitles or audio
  - performs local ASR when required
  - emits normalized text + provenance
  - keeps operational cache/state

Corpus repository
  - contains durable research text
  - is Git-versioned
  - may contain material from many source types
  - is not owned exclusively by Vessel
```

This distinction is deliberate. A research corpus may contain YouTube transcripts, transcripts derived from arbitrary video or audio, tweets, copied text, quotations, or other textual sources. Vessel does not need to become the acquisition tool for every possible source in order to materialize media-derived text into that corpus.

## Transcript Acquisition Policy

For selected media, the default precedence is:

1. creator-provided subtitles
2. platform automatic captions
3. local ASR from downloaded audio

Every materialized transcript must retain provenance sufficient to distinguish those cases.

A locally generated transcript should also record the ASR engine, model, and relevant generation metadata.

Local ASR should be CPU-first by default. GPU acceleration is optional, never required for the normal corpus-maintenance workflow.

## Update Semantics

The target `vessel update` command is a reconcile operation:

1. read declarative corpus/source policy
2. discover current upstream candidates
3. apply inclusion and exclusion rules
4. compare the desired set against materialized corpus artifacts
5. acquire missing text
6. write or improve corpus artifacts only when meaningful content changes

Normal updates are additive and non-destructive.

If policy changes make existing corpus material no longer selected, `vessel update` must not silently delete it. Destructive cleanup belongs behind an explicit operation such as:

```bash
vessel prune --dry-run
vessel prune
```

## What Vessel Is Not

Vessel is not primarily:

- a channel analytics product
- a view/subscriber time-series tracker
- a generic historical metadata warehouse
- a complete clone of every `yt-dlp` option
- the canonical long-term repository for research text
- a requirement to retain downloaded audio/video after text is materialized

Existing metadata/history capabilities may remain when useful, but they are subordinate to reliable source discovery, acquisition, provenance, and text materialization.

## Current Capabilities

The pre-scope-reset implementation already includes substantial reusable machinery:

- native YouTube video extraction
- native channel extraction and backlog crawling
- channel-scoped sync
- SQLite storage
- subtitles and automatic-caption metadata
- comments and thumbnails
- native downloads
- format selection
- FFmpeg-backed postprocessing
- plugin infrastructure
- no silent runtime fallback to Python or `yt-dlp` for supported native paths

These capabilities are assets, not the definition of future scope.

## Development Philosophy

- Rust-native remains strongly preferred.
- Supported native behavior must fail explicitly rather than silently delegating to `yt-dlp`.
- Operational state should be replaceable; durable corpus text should be portable.
- Provenance is part of the artifact, not an afterthought.
- Repeated updates should be idempotent.
- Do not create Git churn from meaningless refresh timestamps.
- Do not delete already acquired corpus material during ordinary update.
- Optimize for a boring one-command maintenance loop.

## Build

Requirements:

- Rust toolchain
- `ffmpeg` for current media/postprocessing paths

```bash
cargo build --release
target/release/vessel --help
```

## Documentation

- [Project Charter](docs/charter.md)
- [Architecture](docs/architecture.md)
- [Roadmap](docs/roadmap.md)
- [Storage Model](docs/storage.md)
- [Parity / Capability Matrix](docs/parity-matrix.md)
- [Session Guide](docs/session-guide.md)

## Historical Note

Vessel began as a Rust-native, replacement-oriented `yt-dlp` project with local dataset/history semantics. That work produced useful extraction and download infrastructure, but the project goal has changed.

The current north star is much narrower and more useful:

> maintain selected media-derived text as durable, provenance-preserving research material.
