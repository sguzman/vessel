# Speaker Diarization And Identity

## Purpose

Vessel must distinguish three separate operations:

1. speech recognition: what words were spoken
2. speaker diarization: which anonymous speaker spoke when
3. speaker identification: whether an anonymous speaker matches a durable known identity

These must never be collapsed into one opaque "transcription" step.

## ASR Backends

Local ASR and speaker diarization are separate backend choices.

Implemented ASR:

- `whisper-candle`: Rust-native Whisper backend
- `phonon-2`: fast English CPU-oriented external backend
- `whisperx`: optional external compatibility backend using faster-whisper

Backend selection is operational. The Sourcearium artifact must preserve the backend/model that actually produced the text.

## Diarization Backends

Primary:

- `sherpa-onnx`: Rust integration over sherpa's C ABI, offline diarization, Pyannote segmentation ONNX plus speaker-embedding ONNX, no Python runtime

Optional compatibility:

- `whisperx`: Python pipeline retained for users who explicitly choose it

Diarization does not choose or replace the ASR backend. The normal architecture supports, for example:

```text
whisper-candle -> sherpa-onnx
phonon-2       -> sherpa-onnx
whisperx       -> sherpa-onnx
```

and the older bundled WhisperX diarization path remains available only when explicitly requested.

## WhisperX Role

WhisperX is useful for faster-whisper ASR, VAD, forced word alignment, and its own diarization pipeline. It remains an optional compatibility backend because that implementation requires a Python environment. Vessel's primary diarization and speaker-identity path must not depend on it.

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


## Implemented Operational Evidence

When a diarization backend runs, Vessel keeps compact non-corpus evidence under:

```text
.cache/vessel/speaker-evidence/<video-id>.json
```

The evidence survives cleanup of the temporary ASR media directory and records:

- file-local diarization labels and time ranges
- ASR engine/model
- diarization engine/model
- speaker embeddings when explicitly requested

It does not contain a durable speaker identity assignment.

Durable identity lives in:

```text
sources/youtube/<source-key>/speakers.toml
```

Current commands:

```text
vessel speakers init --sourcearium <root> --source-key <key>
vessel speakers show --sourcearium <root> --source-key <key>
vessel speakers anchor --sourcearium <root> --source-key <key> --speaker creator --video-id <id> --start-seconds <n> --end-seconds <n>
```

The next matching layer will compare file-local embeddings against embeddings supported by these human-confirmed anchor ranges. Similarity is evidence; it will not be silently promoted to identity without an explicit calibrated attribution rule.


## Read-Only Cross-Video Matching

Vessel now has a read-only matching stage:

```text
vessel speakers match \
  --sourcearium <root> \
  --source-key <key> \
  --video-id <target-video-id>
```

The matcher does not rewrite transcripts or `speakers.toml`.

It uses:

- human-confirmed registry anchors from `speakers.toml`
- persisted backend-neutral speaker evidence from `.cache/vessel/speaker-evidence/<video-id>.json`

For each human-confirmed anchor:

1. find diarized segments that overlap the anchor time range
2. require one file-local diarization label to dominate the speech overlap
3. require that label to have a speaker embedding
4. require compatible diarization engine/model and embedding dimension with the target
5. normalize the embedding and use it as one sample for the stable identity

Multiple compatible anchor samples for one identity are normalized, averaged, and normalized again to produce an operational identity centroid.

For each anonymous target speaker cluster, Vessel computes cosine similarity against compatible identity centroids.

Default read-only acceptance gates are deliberately conservative and are **not yet calibrated as universal truth**:

- minimum cosine similarity: `0.80`
- minimum margin above the runner-up identity: `0.05`
- minimum anchor cluster dominance: `0.80`

All three can be changed explicitly:

```text
--min-similarity <value>
--min-margin <value>
--min-anchor-dominance <value>
```

A result can be:

- `matched`
- `below_similarity`
- `ambiguous_margin`
- `missing_embedding`
- `no_compatible_anchors`

The report also preserves per-anchor diagnostics such as missing evidence, incompatible provenance, low dominance, missing embeddings, or incompatible dimensions.

A read-only `matched` result is still evidence, not durable identity. Automatic transcript identity application remains a separate later step.


## Applying Model-Matched Identity

After reviewing a read-only match report, Vessel can persist the accepted `matched` decisions:

```text
vessel speakers apply \
  --sourcearium <root> \
  --source-key <key> \
  --video-id <target-video-id>
```

`apply` recomputes the match report using the supplied thresholds and refuses to apply it if the target artifact's diarization engine/model differs from the evidence.

Model-matched identity is stored under the Sourcearium `speaker_attribution` extension. The raw transcript body is **not rewritten**.

This is intentional:

- `<speaker:SPEAKER_00>` remains the raw file-local diarization observation.
- the extension records that `SPEAKER_00` matched stable identity `creator` under a particular registry revision and threshold configuration.
- rerunning the same application is idempotent.
- if the registry or calibration changes later, the attribution layer can be replaced without destroying the original diarization labels.

Each persisted assignment records:

- diarization label
- stable identity key
- `attribution = "model_matched"`
- cosine similarity
- runner-up identity/similarity when present
- similarity margin when present
- number of compatible human anchor samples

The extension also records the registry revision, diarization engine/model, and all matching thresholds.

A future projection/rendering layer may display stable identity names in a human-facing transcript without mutating the canonical raw diarization body.


## Human-Facing Speaker Projection

Canonical transcript bodies keep raw file-local diarization labels even after identity attribution is applied.

For a human-facing view, render a projection:

```text
vessel speakers render \
  --sourcearium <root> \
  --source-key <key> \
  --video-id <video-id>
```

A canonical line such as:

```text
[00:00:03] <speaker:SPEAKER_00> Spoken text.
```

can project as:

```text
[00:00:03] <speaker:creator> Spoken text.
```

To retain the raw cluster label in the rendered view:

```text
vessel speakers render \
  --sourcearium <root> \
  --source-key <key> \
  --video-id <video-id> \
  --show-clusters
```

which renders:

```text
[00:00:03] <speaker:creator|SPEAKER_00> Spoken text.
```

Projection is read-only. It does not rewrite the Sourcearium artifact.


## Integrated Update Mode

Speaker attribution can also be requested as part of a normal bounded Sourcearium update.

First cache the default Rust diarization models when network access is available:

```text
vessel diarization fetch
```

After the runtime/models are installed, Vessel can explicitly seed a durable real-audio fixture:

```text
vessel diarization seed --video-id <youtube-video-id>
```

Seeding is the only networked part of this acceptance path. It downloads the selected audio, normalizes it to mono 16 kHz PCM WAV, writes a provenance sidecar, and installs the fixture under Vessel's user data directory. Successful Sourcearium ASR materialization deletes its disposable per-video ASR cache, so acceptance tests must not point at those temporary cache paths.

A seeded fixture can then be exercised without YouTube, ASR, Sourcearium mutation, or network access:

```text
vessel diarization run --fixture-video-id <youtube-video-id> --speaker-embeddings
```

A direct WAV path remains supported:

```text
vessel diarization run <16-kHz-wav> --speaker-embeddings
```

The standalone probe reports anonymous speaker labels and time segments, plus embedding dimensions/norms when requested; it does not persist identities or rewrite corpus artifacts.

Once a fixture has been proven, an existing timestamped `local_asr` Sourcearium transcript can be enriched without retranscription:

```text
vessel diarization apply --sourcearium <root> --source-key <source> --video-id <youtube-video-id>
```

`diarization apply` is an explicit local mutation. It requires the durable fixture for the same video id, runs sherpa with speaker embeddings enabled, attaches anonymous file-local speaker labels to the existing transcript, persists speaker evidence under `.cache/vessel/speaker-evidence`, and updates only the diarization enrichment. It refuses to change the transcript's derivation, language, timestamp policy, ASR engine, or ASR model. If re-diarization changes the anonymous labeling, any prior speaker-attribution extension is cleared rather than left stale.

The fetch boundary is explicit and authenticated. `vessel diarization fetch --plan` performs no network I/O and reports the exact pinned byte count and SHA-256 for each runtime/model artifact. A real fetch verifies the SHA-256 before an archive is extracted or a downloaded model is promoted into place. After installation, Vessel keeps BLAKE3 integrity receipts so `diarization doctor` can detect later offline corruption without contacting the network.

Then any supported ASR backend can feed the primary Rust diarization path:

```text
vessel update \
  --sourcearium <root> \
  --asr-backend whisper-candle \
  --diarize \
  --speaker-embeddings \
  --attribute-speakers
```

`--attribute-speakers` is deliberately opt-in. It requires:

- `--diarize`
- `--speaker-embeddings`
- a valid `speakers.toml` for the source
- compatible cached speaker evidence for the human-confirmed anchor videos

The default diarization backend is `sherpa-onnx`. WhisperX only becomes a requirement if `--diarization-backend whisperx` is explicitly selected.

The update path reuses the same matching and metadata-application functions as `vessel speakers match` and `vessel speakers apply`; it does not have a second hidden attribution algorithm.

After a diarized ASR artifact materializes, the integrated update:

1. loads the source speaker registry
2. matches the target file-local speaker embeddings against compatible human-confirmed anchor centroids
3. records only threshold-passing `matched` assignments
4. persists those assignments into the non-destructive `speaker_attribution` extension
5. leaves the canonical raw diarization labels unchanged
6. deletes disposable ASR media only after evidence and attribution work complete

Update reports expose attribution attempt, assignment, update/no-op, and skip counts.

Integrated matching uses the same calibration controls as the standalone matcher:

```text
--speaker-min-similarity <value>
--speaker-min-margin <value>
--speaker-min-anchor-dominance <value>
```

Defaults remain `0.80`, `0.05`, and `0.80` respectively. The selected values are emitted in the update report.

At present the Rust-native update path diarizes the normalized local-ASR audio path. Caption-backed artifacts are explicitly skipped until shared audio preparation is wired for them; this is an implementation boundary, not a conceptual restriction.
