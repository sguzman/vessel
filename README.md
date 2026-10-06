# Vessel

**Project color:** Vessel Wake Teal `#16A7A0`

Vessel is a small Rust control plane for maintaining media-derived research text in Sourcearium.

Its governing rule is:

> **Own the semantics; outsource the machinery.**

The steady-state loop is:

```bash
vessel inventory --sourcearium /path/to/sourcearium
vessel validate --sourcearium /path/to/sourcearium
vessel update --sourcearium /path/to/sourcearium
```

Use preview for bounded inspection:

```bash
vessel update --sourcearium /path/to/sourcearium --preview --max-videos 3 --report-items
```

Destructive cleanup is explicit:

```bash
vessel prune --sourcearium /path/to/sourcearium
vessel prune --sourcearium /path/to/sourcearium --apply
```

## Architecture

Vessel owns:

- declarative source policy and selection;
- transcript precedence: creator subtitles > platform auto captions > local ASR;
- Sourcearium schema mapping and provenance;
- idempotent reconciliation and representation upgrades;
- validation, deterministic serialization, and atomic materialization;
- SQLite operational state needed to make repeated updates reliable.

External tools own specialized machinery:

- **yt-dlp**: YouTube discovery, metadata, subtitle metadata, and temporary audio acquisition;
- **FFmpeg/ffprobe**: media normalization and probing;
- **WhisperX/faster-whisper**: heavy local ASR;
- **pyannote through WhisperX**: optional anonymous diarization;
- **Phonon-2**: optional lightweight English QA ASR.

There is no maintained native YouTube protocol stack, native downloader/format selector, bespoke Whisper inference implementation, bespoke diarization implementation, or named-speaker identity system.

## Public CLI

The maintained product surface is deliberately small:

- `vessel doctor`
- `vessel update`
- `vessel validate`
- `vessel inventory`
- `vessel prune`

The old dataset/channel/video/download/formats/plugin/ASR/diarization command families were migration-era machinery and are retired.

## Local ASR

Default local ASR is WhisperX with `large-v3` on CPU unless overridden.

```bash
vessel update --force-local-asr
vessel update --force-local-asr --asr-device cuda
vessel update --force-local-asr --diarize
```

Diarization preserves only file-local labels such as `SPEAKER_00`. Vessel does not attempt cross-video speaker identity.

## Dependencies

Required for the relevant paths:

- Rust toolchain
- `yt-dlp`
- `ffmpeg` / `ffprobe`
- `whisperx` when local ASR or diarization is needed
- `uv` is the only supported manager for Python-backed tooling; provision ahead of ingestion with source builds disabled
- `fermion` only if the optional Phonon-2 backend is used

```bash
cargo build --release --locked
target/release/vessel doctor
```

Normal `vessel update` never installs or builds Python packages. WhisperX/pyannote must be provisioned explicitly ahead of time; offline/cached preparation is preferred when practical.

## Sourcearium

Sourcearium schema v1 is an external frozen contract. Vessel must preserve source, representation, and acquisition provenance separately and must not invent new v1 core fields.

Normal `update` is conservative and non-destructive. It may create a missing artifact or upgrade a weaker representation, but it does not delete existing corpus material merely because upstream state or policy changed.

See:

- [Project Charter](docs/charter.md)
- [Architecture](docs/architecture.md)
- [Sourcearium Contract](docs/sourcearium-contract.md)
- [ASR](docs/asr.md)
- [Speaker Diarization Boundary](docs/speaker-attribution.md)
- [Migration Autopsy](docs/toolchain-migration-autopsy-2026-10-03.md)
- [Roadmap](docs/roadmap.md)
