# Speaker Diarization

## Current status

Vessel supports **anonymous, file-local speaker diarization** where it improves corpus materialization.

Cross-video named-speaker identity is retired from the maintained product. Vessel no longer exposes the
`speakers` command family, speaker-registry management, identity matching, anchor calibration, or
named-speaker controls on `vessel update`.

The historical failure and the reason for this boundary are documented in the
[speaker-identity scope-hijack postmortem](postmortem-speaker-identity-scope-hijack-2026-10-03.md).

## Product boundary

The maintained distinction is now deliberately simple:

~~~text
speech recognition
    -> what words were spoken

anonymous diarization
    -> which file-local speaker cluster spoke when
~~~

Vessel does **not** attempt to answer whether `SPEAKER_00` in one video is the same human as
`SPEAKER_03` in another video.

Labels such as `SPEAKER_00` and `SPEAKER_01` are file-local observations, not identities.

## Default path

The default heavy transcription stack is:

~~~text
yt-dlp
  -> temporary audio acquisition

FFmpeg
  -> 16 kHz mono PCM normalization

WhisperX
  -> ASR
  -> optional anonymous diarization

Vessel
  -> TranscriptCandidate
  -> provenance
  -> validation
  -> Sourcearium materialization
~~~

Diarization remains opt-in:

~~~bash
vessel update --diarize
~~~

Because WhisperX is the default ASR and diarization backend, no separate Rust-native speaker model is
required for the normal path.

## Sourcearium representation

Anonymous labels may appear in a timestamped transcript body:

~~~text
[00:00:03] <speaker:SPEAKER_00> First segment.

[00:00:08] <speaker:SPEAKER_01> Second segment.
~~~

The artifact also preserves diarization engine/model provenance separately from ASR provenance.

Vessel does not promote these labels into stable identities.

## Operational evidence

Legacy/diagnostic diarization paths may persist compact evidence under:

~~~text
.cache/vessel/speaker-evidence/<video-id>.json
~~~

The evidence format records:

- video id;
- ASR engine/model;
- diarization engine/model;
- file-local speaker segments;
- optional embeddings when an explicit legacy diagnostic path produced them.

This is operational state, not durable named identity.

The small `vessel-core::speaker_evidence` module exists only to validate/read this backend-neutral
evidence format. The former registry and cross-video matching modules have been removed.

## Legacy Sherpa path

The Rust/Sherpa diarization implementation and explicit `vessel diarization` maintenance commands
remain temporarily available during migration comparison/recovery.

They are not the default `vessel update` architecture and must not grow into a second active
speaker-recognition roadmap.

When the external WhisperX path has sufficient real-source acceptance evidence, the remaining
superseded Sherpa-specific machinery is eligible for deletion.

## Compatibility with old artifacts

Older Sourcearium material may already contain a `speaker_attribution` extension or a
`speakers.toml` sidecar from the abandoned identity experiment.

Current Vessel behavior is conservative:

- existing transcript files are not deleted because those historical fields exist;
- `speakers.toml` is ignored by inventory/validation rather than treated as active configuration;
- Vessel does not create new named-speaker attribution;
- if re-diarization changes diarization provenance, stale `speaker_attribution` metadata is cleared
  so an old identity mapping cannot silently attach to new anonymous clusters.

The frozen Sourcearium v1 contract is therefore preserved without keeping the identity system alive.

## Engineering rule

Speaker work must directly serve transcript materialization.

Anonymous diarization is allowed when it materially improves readable corpus text. Cross-video identity,
embedding calibration, registries, creator priors, anchor research, and matching thresholds are outside
the maintained Vessel product.
