# Milestone 07: Format Selector Parity

## Goal

Close baseline parity gaps in format selection syntax and behavior.

## Deliverables

- [x] Parser for selectors like `bestvideo+bestaudio/best`
- [x] Filtered selector expressions
- [x] Selector evaluation tests

## Implemented Scope

- [x] Parse baseline selectors: `best`, `worst`, `bestaudio`, `bestvideo`, exact format IDs.
- [x] Parse merge and fallback expressions such as `bestvideo+bestaudio/best`.
- [x] Parse baseline filters such as `ext=mp4` and `height<=720`.
- [x] Evaluate filtered selectors against native format metadata.
- [x] Preserve explicit unsupported behavior for merge execution until postprocessing exists.

## Acceptance Criteria

- [x] Baseline selector expressions parse successfully.
- [x] Selection results match documented expectations for fixture metadata.
