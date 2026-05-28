# Milestone 06: Native Parity

## Goal

Close the remaining native YouTube parity gaps without any runtime dependency on Python or `yt-dlp`.

## Deliverables

- [ ] Deeper native YouTube comment coverage beyond the first page of top-level comments
- [ ] Native YouTube download hardening for signature/cipher-protected media
- [ ] Explicit unsupported behavior documented for still-missing native features

## Acceptance Criteria

- [ ] `vessel video comments sync <video>` supports pagination and broader comment coverage without external tools.
- [ ] `vessel download <url>` never shells out to Python or `yt-dlp`.
- [ ] Unsupported native gaps fail explicitly and do not silently delegate to external tools.
