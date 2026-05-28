# Milestone 02: Dataset Ledger

## Goal

Persist video metadata history with idempotent snapshot semantics.

## Deliverables

- [x] `vessel video refresh <video>`
- [x] `vessel video history <video>`
- [ ] Video snapshot diffing
- [x] Fetch attempt recording

## Acceptance Criteria

- [x] Unchanged refreshes do not create duplicate snapshots.
- [ ] Changed metadata creates one new snapshot.
- [x] Failed refreshes preserve the prior latest state.
