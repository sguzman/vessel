# Architecture

## North Star

Vessel is the durable control plane between upstream media and Sourcearium.

```text
Sourcearium policy
      |
      v
    Vessel
      |
      +---------- yt-dlp ---------- YouTube
      |
      +---------- FFmpeg ---------- media normalization
      |
      +---------- WhisperX -------- ASR / optional diarization
      |
      v
normalized TranscriptCandidate
      |
      v
policy + provenance + reconcile + validation
      |
      v
Sourcearium schema v1
```

## Owned Semantics

Vessel owns:

1. source-policy parsing;
2. include/exclude/date selection;
3. transcript precedence;
4. operational SQLite reconciliation state;
5. normalized transcript and provenance structures;
6. create/preserve/upgrade/no-op decisions;
7. deterministic Sourcearium serialization;
8. validation and atomic materialization;
9. explicit prune semantics.

## External Adapters

### YouTube

`vessel-extractors::youtube::YtDlpConfig` is the canonical YouTube boundary. Vessel consumes yt-dlp JSON and uses yt-dlp for temporary ASR audio acquisition.

There is no native watch-page/InnerTube/signature/cipher implementation.

### ASR

`vessel-asr` is an executable adapter layer, not an inference library.

- WhisperX is the default heavy backend.
- Phonon-2 is an optional lightweight English QA backend.
- WhisperX may invoke pyannote when anonymous diarization is requested.

### Media

FFmpeg performs the only media normalization needed by the update path: converting acquired audio to 16 kHz mono PCM WAV for ASR.

## Transcript Provider Chain

```text
creator subtitles
      |
platform automatic captions
      |
local ASR
```

A stronger representation may replace a weaker one. A weaker representation never automatically downgrades a stronger existing artifact.

## State Authority

Sourcearium is authoritative for durable acquired text.

SQLite/cache state is operational and replaceable. It may contain discovery membership, publication facts, transcript probe times, and other reconciliation bookkeeping. Losing it must not invalidate already materialized corpus artifacts.

## Workspace

Maintained crates:

- `vessel-cli`: small product CLI and reconcile orchestration;
- `vessel-core`: Sourcearium policy, models, transcript semantics, validation/materialization;
- `vessel-extractors`: external acquisition adapters and caption normalization;
- `vessel-asr`: external ASR adapters;
- `vessel-store`: SQLite operational state;
- `vessel-ledger`: store/idempotency support;
- `vessel-logging`: logging;
- `vessel-testing`: shared testing utilities.

The retired `vessel-diarization`, `vessel-download`, `vessel-formats`, and `vessel-postprocess` crates are deleted.

## Acceptance Boundary

CI protects the migration by:

- rejecting retired native machinery;
- testing the full workspace on Linux;
- compiling/testing durable layers on Windows;
- running a deterministic end-to-end `update --preview` test with a fake yt-dlp executable;
- checking the reduced CLI and offline doctor behavior;
- checking formatting.

See [Toolchain Migration Autopsy](toolchain-migration-autopsy-2026-10-03.md).
