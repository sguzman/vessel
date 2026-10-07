# Speaker Diarization Boundary

## Maintained Scope

Vessel supports only optional **anonymous, file-local diarization** as part of the WhisperX transcription path.

```text
SPEAKER_00
SPEAKER_01
```

Those labels describe clusters within one media item. They are not identities.

## Retired Scope

The following system is deleted and is not an active Vessel capability:

- speaker registries;
- cross-video identity matching;
- speaker embeddings for identity;
- anchor caches;
- creator priors/cohorts;
- cosine/rank-gap scoring;
- threshold calibration;
- Rust/Sherpa diarization commands;
- operational speaker-evidence files.

The historical failure is documented in [the speaker-identity scope-hijack postmortem](postmortem-speaker-identity-scope-hijack-2026-10-03.md).

## Current Flow

```text
yt-dlp -> FFmpeg -> WhisperX/pyannote
                    |
                    v
        anonymous speaker-labelled segments
                    |
                    v
                Vessel
                    |
                    v
              Sourcearium
```

Vessel owns normalization, provenance, validation, and materialization; it does not own speaker-recognition ML.


## Manual Downstream Identity Annotation

A human may listen to a Sourcearium transcript's underlying media and separately assert that a file-local anonymous label corresponds to a named person.

That is a **Sourcearium manual annotation**, not a Vessel capability.

The boundary is:

```text
Vessel
-> anonymous file-local SPEAKER_XX labels

human audio review
-> separate Sourcearium file-local identity annotation
```

The original transcript remains mechanical and unchanged. A manual assertion must not become a reusable voiceprint, embedding anchor, cross-video registry entry, creator prior, or automatic attribution rule.

The first exercised example is Tiny Clipper `YOSJDOLe_R0`, where human review verified `SPEAKER_10 = Destiny` in a separate Sourcearium annotation artifact.

## Compatibility

Older Sourcearium artifacts may contain historical speaker-attribution metadata. Vessel preserves existing corpus material conservatively but does not create new named-speaker identity data.

Reviving named identity requires a new explicit product decision, not incremental extension of anonymous diarization.
