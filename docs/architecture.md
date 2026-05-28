# Architecture

## Workspace Map

- `vessel-cli`: user-facing CLI and command wiring.
- `vessel-core`: config, errors, event model, normalized metadata types.
- `vessel-extractors`: extractor traits, registry, and site-specific implementations.
- `vessel-ledger`: ledger-facing sync APIs and decisions.
- `vessel-store`: SQLite schema, migrations, and store traits.
- `vessel-logging`: `tracing` initialization and log formatting.
- `vessel-formats`: format selector AST and output templates.
- `vessel-download`: download planning interfaces.
- `vessel-postprocess`: FFmpeg/postprocessor planning interfaces.
- `vessel-testing`: shared fixture helpers for later parity and regression tests.

## Core Boundaries

- Extraction produces structured metadata. It does not own downloads.
- Downloads consume normalized metadata and format decisions.
- The ledger owns idempotency, snapshot insertion rules, and fetch-run recording.
- The store owns persistence details and current-vs-history table semantics.
- Logging consumes the internal event stream rather than inventing separate state transitions.

## Event Flow

1. CLI loads config and initializes logging.
2. CLI resolves an extractor through `ExtractorRegistry`.
3. Extractor returns normalized `ExtractedItem` values plus raw payloads.
4. Ledger hashes normalized payloads and decides whether snapshots changed.
5. Store persists current rows, snapshots, and fetch attempts.
6. Download and postprocess stages attach artifacts later in the roadmap.

## Initial Assumptions

- Native-first and YouTube-first.
- SQLite-first; PostgreSQL is deferred until the ledger semantics are stable.
- Strict native-only runtime: `vessel` must not shell out to Python or `yt-dlp` for supported behavior.
