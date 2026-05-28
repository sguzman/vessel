# Milestone 02: Dataset Ledger

## Goal

Persist video metadata history with idempotent snapshot semantics.

## Deliverables

- [ ] `vessel video refresh <video>`
- [ ] `vessel video history <video>`
- [ ] Video snapshot diffing
- [ ] Fetch attempt recording

## Acceptance Criteria

- [ ] Unchanged refreshes do not create duplicate snapshots.
- [ ] Changed metadata creates one new snapshot.
- [ ] Failed refreshes preserve the prior latest state.
