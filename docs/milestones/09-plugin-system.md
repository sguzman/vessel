# Milestone 09: Plugin System

## Goal

Add a plugin API for extractor extensions and future token/provider integrations.

## Deliverables

- [ ] `vessel plugin list`
- [ ] `vessel plugin install`
- [ ] Extractor plugin interface
- [ ] Provider hooks for token and credential sources

## Acceptance Criteria

- [ ] Plugins can register extractors or providers without patching core crates.
- [ ] Plugin failures are isolated and reported cleanly.
