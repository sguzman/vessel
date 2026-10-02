# YouTube Access Boundaries

## Why This Exists

Vessel's YouTube channel discovery and player/transcript acquisition do not have identical access requirements.

A channel page may remain crawlable while individual player requests are rejected.

The first real Sourcearium acceptance source, ContraPoints, demonstrated this on 2026-09-19 from GitHub-hosted Ubuntu runners.

Channel discovery succeeded and found 36 videos.

Player metadata requests for sampled videos returned:

```text
playabilityStatus.status = LOGIN_REQUIRED
playabilityStatus.reason = Sign in to confirm you’re not a bot
```

The same hosted-runner IP was tested with several current Innertube client profiles:

- Android
- Android VR
- TV
- downgraded TV
- VisionOS
- Web Embedded

The non-embedded clients were bot-gated. The embedded client reported the sampled video unavailable.

## Residential Acceptance - 2026-10-02

The same ContraPoints acceptance source was re-tested from the actual residential target environment.

The player-access blocker observed on hosted CI did **not** reproduce.

For the sampled video `uiGIbdrQjbI` ("Saw | ContraPoints"), Vessel resolved:

- normal public playability
- video/channel metadata
- playable streaming data
- one creator English caption track
- one automatic English caption track

This proves that the target environment is suitable for normal player/media acquisition.

A three-video Sourcearium preview then completed without extractor errors. Channel membership remained durable at 36 videos. On the later incremental crawl, 32 currently visible first-page videos were re-observed while the persisted 36-video backlog remained authoritative. This is expected once completed tab backfills stop re-walking historical continuations.

### Caption Body Access

The advertised creator and automatic caption tracks did not yield transcript bodies through Vessel's direct `timedtext` requests.

Observed behavior:

```text
caption track advertised in player metadata
HTTP request succeeds
response body is empty
```

This is materially different from "the video has no captions."

As of 2026, the same zero-byte-success behavior is associated in the wider YouTube tooling ecosystem with proof-of-origin / PO-token enforcement on subtitle requests.

Vessel therefore preserves empty caption responses as an access diagnostic and may continue to local ASR when policy permits. It does **not** silently add cookies, browser automation, yt-dlp fallback, or PO-token machinery.

If native caption retrieval is pursued later, treat proof-of-origin support as an explicit acquisition/authentication feature rather than hiding it inside the subtitle parser.

### Operational SQLite Finalization

The acceptance run also exposed that WAL-mode operational state could remain primarily in `vessel.sqlite-wal` after the CLI returned.

`vessel update` now explicitly:

1. checkpoints the WAL with `PRAGMA wal_checkpoint(TRUNCATE)`
2. awaits SQLx pool closure
3. reports `operational_state.checkpointed = true`

A completed update therefore leaves the main `vessel.sqlite` as a self-contained readable database rather than requiring a live WAL sidecar for recently committed state.


## Error Classification

When a player response lacks `videoDetails`, Vessel must inspect `playabilityStatus` before calling the response malformed.

For the observed YouTube bot-confirmation response, Vessel reports an explicit anti-bot gate error.

This distinction matters operationally:

```text
missing/malformed response != YouTube refused this environment
```

## Target Environment

Real YouTube acceptance should preferentially run from the actual target/residential environment.

Hosted cloud CI is useful for:

- compilation
- unit/integration tests
- Sourcearium schema validation
- deterministic corpus logic

It is not guaranteed to be a usable YouTube player-acquisition environment.

## Authentication And Attestation Boundary

Vessel does not silently solve a bot gate by adding:

- account cookies
- browser-cookie extraction
- browser automation
- PO-token generation/providers
- external yt-dlp fallback

Those mechanisms have different security, account-risk, provenance, portability, and maintenance consequences.

If the target environment is also bot-gated, authentication/attestation becomes an explicit project design problem.

The human principal and project director should choose that boundary deliberately before implementation.

## Native Client Identity

Vessel's secondary Android player request uses a current 2026 Android client identity rather than the older May-era profile.

This request can improve player/streaming-data compatibility when YouTube permits the environment.

It is not treated as an anti-bot bypass.

## Sourcearium Safety

A player-access failure must not:

- create an empty transcript
- create a partial transcript
- trigger local ASR without usable media acquisition
- replace an existing Sourcearium artifact
- reinterpret an inaccessible source as deleted/private without evidence
- cause prune candidates

Normal failure leaves durable corpus authority unchanged.

## Direct Transcript Endpoint

The 2026-09-19 ContraPoints acceptance also tested whether Vessel could avoid player metadata for captioned videos by calling YouTube's Innertube `get_transcript` endpoint directly.

The diagnostic generated transcript parameters for the sampled video and tried:

- WEB + manual English
- WEB + auto-generated English
- Android + manual English
- Android + auto-generated English

All four hosted-runner requests returned:

```text
HTTP 400
FAILED_PRECONDITION
Precondition check failed.
```

So, on the tested GitHub-hosted IP, a caption-first direct-transcript path does not bypass the access restriction.

Do not add a `get_transcript` fallback merely because older examples show the endpoint working. Re-evaluate it only against the actual target environment or after independently verified upstream behavior changes.

## Target-Environment Preflight

Before running a real Sourcearium update on a new network/environment, test one known public video through Vessel's normal extractor:

```powershell
vessel info https://www.youtube.com/watch?v=uiGIbdrQjbI
```

This is a connectivity/access preflight, not corpus QA.

If it succeeds, proceed to the normal Sourcearium preview:

```powershell
vessel update --preview --max-videos 3
```

If it returns the explicit YouTube anti-bot gate error, stop before materialization and treat access/authentication as a separate design problem.

The sampled URL is only a stable acceptance probe; Sourcearium policy remains the authority for which sources belong in the corpus.
