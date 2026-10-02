# Speaker Diarization And Identity

## Purpose

Vessel must distinguish three separate operations:

1. speech recognition: what words were spoken
2. speaker diarization: which anonymous speaker spoke when
3. speaker identification: whether an anonymous speaker matches a durable known identity

These must never be collapsed into one opaque "transcription" step.

## ASR Backends

Local ASR is backend-pluggable.

Current:

- `whisper-candle`: Rust-native Whisper backend

Planned:

- `whisperx`: external/optional pipeline using faster-whisper, forced alignment, and pyannote diarization
- `phonon-2`: fast English CPU-oriented backend

Backend selection is operational. The Sourcearium artifact must preserve the backend/model that actually produced the text.

## WhisperX Role

WhisperX is not merely a different Whisper checkpoint.

Its useful capabilities for Vessel are:

- batched faster-whisper ASR
- VAD
- word-level forced alignment
- speaker diarization
- optional speaker embeddings

WhisperX may therefore serve either as:

- a complete ASR + alignment + diarization backend; or
- a diarization/alignment stage layered onto text produced by another ASR backend

Vessel must keep those provenance roles separate.

## Speaker Labels Versus Identity

Per-file diarization labels such as `SPEAKER_00` and `SPEAKER_01` are ephemeral clustering labels. They are not identities and must never be persisted as if they meant the same person across videos.

Stable identities use Vessel speaker keys, for example:

- `creator`
- `guest:<stable-key>`
- `unknown:<stable-key>`

A stable identity mapping requires evidence.

## Creator Voice Registry

For a configured YouTube source, Vessel may maintain a durable speaker registry tied to the stable channel ID.

The registry records:

- stable speaker key
- optional display label
- identity relation to the source, e.g. `creator`
- anchor video IDs and timestamp ranges
- how each anchor was attributed
- diarization/embedding backend and model used
- registry revision

The registry should preserve human-auditable anchors rather than pretending an opaque embedding alone is truth.

Speaker embeddings/centroids may be cached operationally for fast matching. They must record the exact embedding model and registry revision. If durable embeddings are ever added, that is an explicit format decision rather than an accidental SQLite detail.

## Attribution Confidence

A diarized speaker can be mapped to a stable identity only with an explicit attribution state:

- `human_confirmed`
- `model_matched`
- `unresolved`

For model matches, preserve:

- embedding/diarization engine
- model
- similarity/confidence value when available
- registry revision used for comparison

Never silently convert an uncertain match into a hard speaker identity.

## Sourcearium Rendering

Schema v1 remains frozen.

Speaker-aware transcript metadata belongs under an extension namespace, not new v1 core fields.

The readable transcript body may render speaker turns, but the format must remain deterministic and must not erase the distinction between words produced by ASR, anonymous diarization labels, and durable identity attribution.

A possible readable form is:

```text
[00:00:03] <creator> First segment.

[00:00:08] <speaker:guest-01> Second segment.
```

Only use `<creator>` after identity attribution. Anonymous diarization should remain anonymous.

## Long-Running Identity

Desired cross-video behavior:

```text
audio
  -> diarization
  -> anonymous speaker embedding
  -> compare against channel speaker registry
  -> stable attribution when justified
  -> preserve attribution provenance
```

This allows clips, inserted media, interviews, and quoted audio to remain separate from the YouTuber's own voice even when all speech appears in one video.

## Offline Model Handling

Every backend must support an explicit local/offline model path where technically possible.

Automatic first-use download/cache remains allowed, but must never be the only supported model-acquisition path.

Long-running stages must emit progress or heartbeat information. A silent multi-minute model load or inference run is a bug.

## Implementation Order

1. finish Whisper progress/offline-path support
2. introduce an ASR backend abstraction without changing Sourcearium policy
3. add WhisperX as an explicit opt-in backend
4. persist diarization output separately from identity
5. add the channel speaker registry and creator anchors
6. add cross-video embedding matching with explicit confidence/provenance
7. add Phonon-2 as the fast English ASR backend
8. evaluate whether speaker-aware body rendering should become default or remain optional
