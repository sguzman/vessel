# Milestone 03: Channel Tracking

## Goal

Track channels and synchronize current and historical channel/video metadata.

## Deliverables

- [ ] `vessel channel add <channel>`
- [ ] `vessel channel sync`
- [ ] Channel snapshot persistence
- [ ] Video discovery from channels
- [ ] Freshness policy implementation

## Acceptance Criteria

- [ ] Channels are stored as tracked targets.
- [ ] Sync discovers new videos and updates known ones.
- [ ] Re-running sync without changes does not create duplicate snapshots.
