# Milestone 00: Skeleton

## Goal

Create a serious Rust workspace with the base CLI, config, logging, and SQLite schema.

## Deliverables

- [x] Multi-crate workspace
- [x] Clap command tree
- [x] `tracing` bootstrap
- [x] Config loading
- [x] SQLite initialization command
- [x] Initial schema for current-state, snapshot, and archive tables
- [x] Compile-clean stub modules for extractors, ledger, download, formats, postprocess, and testing

## Acceptance Criteria

- [x] `cargo build`
- [x] `vessel --help`
- [x] `vessel doctor`
- [x] `vessel config show`
- [x] `vessel dataset init`
