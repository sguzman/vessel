# Local ASR

## Current direction

Vessel's default local-ASR backend is now **WhisperX**.

The architecture is intentionally process-based:

~~~text
Vessel
-> normalized local audio
-> whisperx executable
-> JSON
-> TranscriptCandidate
-> Sourcearium materialization
~~~

Vessel owns transcript precedence, normalization, provenance, reconciliation, validation, and durable
materialization. It does not need to own Whisper inference or diarization internals.

## Defaults

For `vessel update`:

~~~text
ASR backend = whisperx
model = large-v3
device = cpu
diarization backend = whisperx
~~~

Diarization is still opt-in. When enabled through the WhisperX path, anonymous labels such as
`SPEAKER_00` and `SPEAKER_01` may be preserved in the normalized transcript.

Cross-video named-speaker identity is not an active product goal.

## External tool boundary

WhisperX is invoked as an executable and its JSON is parsed into Vessel's existing normalized
`TranscriptCandidate`.

The adapter currently supports operational controls for:

- model selection;
- device selection;
- language hints;
- explicit model directories/cache-only use;
- progress output;
- optional diarization;
- diarization model;
- optional minimum/maximum speaker hints;
- Hugging Face token environment for diarization model access.

Python is an implementation detail behind this process boundary.

Prefer an isolated installation managed with `uv` rather than making Vessel itself a Python
application.

## Media acquisition

When `vessel update` uses the default YouTube backend, ASR source audio is acquired through
**yt-dlp**, not Vessel's native format planner/downloader.

Temporary media lives under:

~~~text
<sourcearium>/.cache/vessel/asr/<video-id>/
~~~

Vessel then normalizes the source audio to 16 kHz mono PCM WAV through FFmpeg before transcription.

A durable compatible local fixture may still be reused to avoid a redundant network download.

If acquisition, normalization, transcription, validation, or materialization fails, no durable
Sourcearium artifact is replaced.

## Provenance

A local ASR artifact records the actual engine/model used.

For the default path this is conceptually:

~~~toml
[representation]
derivation = "local_asr"
engine = "whisperx-faster-whisper"
model = "large-v3"
timestamps = true
~~~

If diarization is used, diarization engine/model provenance is recorded separately.

Sourcearium semantics do not depend on WhisperX. The backend remains replaceable.

## Legacy backends during migration

The following implementations remain temporarily available while the external-tool migration is being
proven:

- `whisper-candle`;
- `phonon-2`;
- Rust/Sherpa diarization.

They are not equal-priority roadmap branches.

`phonon-2` may survive if its fast English QA niche remains useful. The bespoke Whisper/Sherpa paths
should be retired once the replacement path has sufficient acceptance evidence.

Do not invest new product work in named-speaker matching, embedding calibration, or speaker identity.

## Operational validation

Useful checks:

~~~bash
vessel doctor
vessel asr doctor
vessel update --preview --max-videos 3
~~~

A real local-ASR acceptance run should use a release build and a bounded source/video selection.

The acceptance question is whether the external toolchain produces a valid Sourcearium candidate with
correct provenance, not whether Vessel can reproduce the ML stack internally.
