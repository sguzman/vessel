# Architecture

## Architectural North Star

Vessel is an acquisition/materialization engine, not the canonical research archive.

The architecture is split into two state domains:

```text
upstream media
    |
    v
Vessel acquisition engine
    |
    +-- operational cache / SQLite / temporary media
    |
    v
external corpus repository
    |
    +-- durable text
    +-- timestamps
    +-- provenance
    +-- Git history
```

The corpus must remain useful without Vessel running.

## Core Pipeline

A corpus update is modeled as:

1. **Discover** candidate upstream objects.
2. **Select** candidates using declarative policy.
3. **Resolve text** using the preferred transcript providers.
4. **Acquire media** only if text cannot otherwise be obtained.
5. **Transcribe** locally when required.
6. **Normalize** text and timestamp structure.
7. **Materialize** durable Git-friendly artifacts.
8. **Record operational state** outside the durable artifact unless it is meaningful provenance.

This is a desired-state reconcile loop, not a metadata polling loop.

## Transcript Provider Chain

The default provider chain is:

```text
Creator subtitles
      |
      v
Platform automatic captions
      |
      v
Local ASR
```

Provider choice and provenance are separate concepts. Even when all providers normalize to the same internal transcript structure, the emitted artifact must retain which provider produced it.

## Workspace Map

Existing crates remain useful:

- `vessel-cli`: user-facing CLI and command wiring
- `vessel-core`: config, errors, events, normalized metadata
- `vessel-extractors`: source discovery and site-specific extraction
- `vessel-ledger`: idempotency and operational decisions
- `vessel-store`: local SQLite/cache persistence
- `vessel-logging`: tracing/log formatting
- `vessel-formats`: media format selection
- `vessel-download`: media acquisition
- `vessel-postprocess`: FFmpeg-backed media transformations
- `vessel-testing`: shared test fixtures

Future corpus work should introduce boundaries rather than overload existing crates:

- transcript-provider abstraction
- local ASR backend abstraction
- corpus policy parser/evaluator
- corpus materializer/exporter

Exact crate names are implementation decisions, not charter-level commitments.

## State Authority

### Durable authority

The external corpus repository is authoritative for acquired research text.

It should contain ordinary inspectable files and provenance.

### Operational authority

Vessel may use SQLite and caches for:

- discovered source identities
- pagination/cursors
- fetch attempts
- local acquisition state
- content hashes
- materialization bookkeeping
- temporary media paths
- error/retry information

Operational state may be deleted and rebuilt without invalidating the corpus.

## Selection Policy

Selection policy belongs with the corpus or a user-controlled project configuration.

Initial policy should remain deliberately small:

- source/channel
- publication date cutoff
- explicit include identifiers
- explicit exclude identifiers

Do not begin by building a query language.

More expressive rules can be added only when real corpus use demonstrates the need.

## Materialization Contract

A materialized transcript should support:

- stable upstream identifier
- source URL
- source/channel identity
- title
- publication date when known
- language
- transcript provenance class
- ASR engine/model when applicable
- timestamped segments when available
- normalized readable text

The durable format should be Git-friendly and human-readable. Markdown with structured front matter is the baseline design candidate.

## Update And Prune

`vessel update` should converge toward the desired corpus set without destroying previously acquired material.

If existing material falls outside current policy:

- leave it in place during update
- report it as no longer selected when useful
- remove it only through an explicit prune operation

## Rust And Runtime Policy

Rust-native remains the preferred implementation strategy.

Supported native features must not silently invoke Python or `yt-dlp`.

FFmpeg remains acceptable media plumbing.

ASR should default to a CPU-capable Rust-facing backend. The transcript interface must not depend on one implementation, model family, or accelerator.

## Source Extensibility

Vessel is media-oriented.

The external corpus can be heterogeneous. That does not imply Vessel itself must ingest every textual source type.

For example, tweets may belong in the same corpus as YouTube transcripts while being populated by an entirely different acquisition tool. The shared contract is provenance-preserving text, not a single universal scraper.
