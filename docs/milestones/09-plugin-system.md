# Milestone 09: Plugin System

## Goal

Add a plugin API for extractor extensions and future token/provider integrations.

## Deliverables

- [x] `vessel plugin list`
- [x] `vessel plugin install`
- [x] Extractor plugin interface
- [x] Provider hooks for token and credential sources

## Implemented Scope

- [x] Manifest-based plugin discovery from system, user, and project plugin directories.
- [x] `vessel plugin install <name>` scaffolding for fixture-extractor and env-provider plugins.
- [x] `vessel plugin list` reporting loaded plugins, registered providers, and isolated load errors.
- [x] Fixture extractor plugins that register through the shared `ExtractorRegistry` without patching core crates.
- [x] Provider plugins for `env`, `file`, and `static` value sources.
- [x] `doctor` reporting for configured provider references such as `youtube.po_token`.

## Acceptance Criteria

- [x] Plugins can register extractors or providers without patching core crates.
- [x] Plugin failures are isolated and reported cleanly.
