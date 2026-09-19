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
