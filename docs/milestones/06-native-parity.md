# Milestone 06: Native Parity

## Goal

Close the remaining native YouTube parity gaps without any runtime dependency on Python or `yt-dlp`.

## Deliverables

- [x] Deeper native YouTube comment coverage beyond the first page of top-level comments
- [x] Native YouTube download hardening for signature/cipher-protected media
- [x] Explicit unsupported behavior documented for still-missing native features

## Acceptance Criteria

- [x] `vessel video comments sync <video>` supports pagination and broader comment coverage without external tools.
- [x] `vessel download <url>` never shells out to Python or `yt-dlp`.
- [x] Unsupported native gaps fail explicitly and do not silently delegate to external tools.
