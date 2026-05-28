# Milestone 03: Channel Tracking

## Goal

Track channels and synchronize current and historical channel/video metadata.

## Deliverables

- [x] `vessel channel add <channel>`
- [x] `vessel channel sync`
- [x] Channel snapshot persistence
- [x] Video discovery from channels
- [x] Freshness policy implementation

## Acceptance Criteria

- [x] Channels are stored as tracked targets.
- [x] Sync discovers new videos and updates known ones.
- [x] Re-running sync without changes does not create duplicate snapshots.
