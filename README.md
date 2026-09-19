# Vessel

**Project color:** Vessel Wake Teal `#16A7A0`

Vessel is a Rust-native media acquisition and text-materialization engine for building durable research corpora.

Its primary job is no longer to chase `yt-dlp` feature parity or maintain metadata time series. The existing YouTube extractor, downloader, subtitle, comment, and SQLite machinery remains valuable, but future development is organized around:

```text
discover -> select -> materialize text -> preserve provenance
```

The intended steady-state workflow is:

```bash
vessel update
```

Vessel now targets **Sourcearium artifact schema v1** for durable corpus output.

See [Sourcearium Contract](docs/sourcearium-contract.md).

## Primary Objective

Given declarative source policy, Vessel should:

- discover selected media
- prefer creator subtitles
- fall back to platform automatic captions
- fall back to local ASR when necessary
- validate and materialize stable Sourcearium text artifacts
- avoid rewriting durable output when nothing meaningful changed

Media downloads and operational metadata are supporting machinery.

## Separation Of Responsibilities

```text
Vessel
  - discovers upstream media
  - applies selection policy
  - acquires captions or temporary media
  - transcribes when required
  - validates/materializes Sourcearium artifacts
  - keeps operational cache/state

Sourcearium
  - owns artifact schema
  - owns durable research text
  - owns provenance semantics
  - is Git-versioned
  - may contain material from producers other than Vessel
```

## Transcript Preference

Default order:

1. creator-provided subtitles
2. platform automatic captions
3. local ASR

A stronger representation may replace a weaker one for the same Sourcearium artifact identity.

A weaker representation must not automatically downgrade an existing artifact.

Local ASR is CPU-first by default. GPU acceleration is optional.

## Update Semantics

`vessel update` is intended to be a non-destructive reconcile operation.

Normal update:

- discovers the desired media set
- materializes missing text
- improves an artifact when a stronger representation becomes available
- produces no durable diff on a no-op refresh
- never deletes already acquired text merely because policy narrowed or upstream disappeared

Destructive cleanup belongs behind explicit operations such as:

```bash
vessel prune --dry-run
vessel prune
```

## Current Capabilities

The pre-scope-reset implementation already includes reusable machinery for:

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
- native YouTube signature/cipher handling

These capabilities are substrate, not the future product definition.

## Development Philosophy

- Rust-native strongly preferred.
- No silent Python/`yt-dlp` fallback for supported native behavior.
- Sourcearium schema v1 is an external frozen contract.
- Preserve source / representation / acquisition provenance separately.
- Operational state is replaceable; Sourcearium text is durable.
- Serialize Sourcearium output deterministically.
- Validate before writing.
- Replace durable artifacts atomically.
- Avoid Git churn from refresh telemetry.
- Normal update is non-destructive.
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
- [Sourcearium Contract](docs/sourcearium-contract.md)
- [Architecture](docs/architecture.md)
- [Roadmap](docs/roadmap.md)
- [Storage Model](docs/storage.md)
- [Capability Matrix](docs/parity-matrix.md)
- [Session Guide](docs/session-guide.md)

## Historical Note

Vessel began as a Rust-native, replacement-oriented `yt-dlp` project with local dataset/history semantics.

That work remains useful infrastructure, but the project north star is now:

> maintain selected media-derived text as durable, provenance-preserving Sourcearium material.
