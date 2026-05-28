pub const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_initial_schema",
    r#"
CREATE TABLE IF NOT EXISTS channels (
    id TEXT PRIMARY KEY,
    platform TEXT NOT NULL,
    channel_id TEXT NOT NULL UNIQUE,
    handle TEXT,
    canonical_url TEXT NOT NULL,
    title TEXT,
    description TEXT,
    subscriber_count INTEGER,
    video_count INTEGER,
    view_count INTEGER,
    avatar_url TEXT,
    banner_url TEXT,
    latest_snapshot_id TEXT,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS channel_snapshots (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    normalized_json TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    changed_fields_json TEXT,
    UNIQUE(channel_id, content_hash)
);

CREATE TABLE IF NOT EXISTS tracked_channels (
    channel_id TEXT PRIMARY KEY,
    canonical_url TEXT NOT NULL,
    handle TEXT,
    title TEXT,
    added_at TEXT NOT NULL,
    last_sync_at TEXT
);

CREATE TABLE IF NOT EXISTS videos (
    id TEXT PRIMARY KEY,
    platform TEXT NOT NULL,
    video_id TEXT NOT NULL UNIQUE,
    channel_id TEXT,
    canonical_url TEXT NOT NULL,
    title TEXT,
    description TEXT,
    upload_date TEXT,
    duration_seconds INTEGER,
    view_count INTEGER,
    like_count INTEGER,
    comment_count INTEGER,
    availability TEXT NOT NULL,
    latest_snapshot_id TEXT,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS video_snapshots (
    id TEXT PRIMARY KEY,
    video_id TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    normalized_json TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    changed_fields_json TEXT,
    UNIQUE(video_id, content_hash)
);

CREATE TABLE IF NOT EXISTS fetch_runs (
    id TEXT PRIMARY KEY,
    command TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    status TEXT,
    error_count INTEGER NOT NULL DEFAULT 0,
    warning_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS fetch_attempts (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_external_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT NOT NULL,
    status TEXT NOT NULL,
    http_status INTEGER,
    extractor TEXT,
    error_kind TEXT,
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS artifacts (
    id TEXT PRIMARY KEY,
    video_id TEXT,
    artifact_kind TEXT NOT NULL,
    path TEXT NOT NULL,
    content_hash TEXT,
    byte_size INTEGER,
    format_id TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS download_archive (
    id TEXT PRIMARY KEY,
    platform TEXT NOT NULL,
    external_id TEXT NOT NULL,
    archive_key TEXT NOT NULL UNIQUE,
    downloaded_at TEXT NOT NULL,
    artifact_id TEXT
);
"#,
)];
