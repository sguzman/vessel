pub const SCHEMA: &str = r#"
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
    primary_category TEXT,
    tags_json TEXT NOT NULL DEFAULT '[]',
    view_count INTEGER,
    like_count INTEGER,
    comment_count INTEGER,
    availability TEXT NOT NULL,
    latest_snapshot_id TEXT,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_videos_channel_id ON videos (channel_id);

CREATE TABLE IF NOT EXISTS video_metric_samples (
    video_id TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    view_count INTEGER,
    like_count INTEGER,
    comment_count INTEGER,
    PRIMARY KEY (video_id, fetched_at)
);

CREATE INDEX IF NOT EXISTS idx_video_metric_samples_video_id_fetched_at
ON video_metric_samples (video_id, fetched_at);

CREATE TABLE IF NOT EXISTS channel_metric_samples (
    channel_id TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    subscriber_count INTEGER,
    video_count INTEGER,
    view_count INTEGER,
    PRIMARY KEY (channel_id, fetched_at)
);

CREATE INDEX IF NOT EXISTS idx_channel_metric_samples_channel_id_fetched_at
ON channel_metric_samples (channel_id, fetched_at);

CREATE TABLE IF NOT EXISTS video_revisions (
    id TEXT PRIMARY KEY,
    video_id TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    title TEXT,
    description TEXT,
    primary_category TEXT,
    tags_json TEXT NOT NULL DEFAULT '[]',
    availability TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    changed_fields_json TEXT NOT NULL DEFAULT '[]',
    UNIQUE(video_id, content_hash)
);

CREATE INDEX IF NOT EXISTS idx_video_revisions_video_id_recorded_at
ON video_revisions (video_id, recorded_at);

CREATE TABLE IF NOT EXISTS channel_revisions (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    title TEXT,
    description TEXT,
    avatar_url TEXT,
    banner_url TEXT,
    handle TEXT,
    content_hash TEXT NOT NULL,
    changed_fields_json TEXT NOT NULL DEFAULT '[]',
    UNIQUE(channel_id, content_hash)
);

CREATE INDEX IF NOT EXISTS idx_channel_revisions_channel_id_recorded_at
ON channel_revisions (channel_id, recorded_at);

CREATE TABLE IF NOT EXISTS subtitle_tracks (
    id TEXT PRIMARY KEY,
    video_id TEXT NOT NULL,
    language TEXT NOT NULL,
    url TEXT,
    is_auto_generated INTEGER NOT NULL,
    latest_snapshot_id TEXT,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(video_id, language, is_auto_generated)
);

CREATE TABLE IF NOT EXISTS subtitle_revisions (
    id TEXT PRIMARY KEY,
    video_id TEXT NOT NULL,
    language TEXT NOT NULL,
    is_auto_generated INTEGER NOT NULL,
    recorded_at TEXT NOT NULL,
    url TEXT,
    content_hash TEXT NOT NULL,
    changed_fields_json TEXT NOT NULL DEFAULT '[]',
    UNIQUE(video_id, language, is_auto_generated, content_hash)
);

CREATE INDEX IF NOT EXISTS idx_subtitle_revisions_video_id_recorded_at
ON subtitle_revisions (video_id, recorded_at);

CREATE TABLE IF NOT EXISTS comments (
    id TEXT PRIMARY KEY,
    platform TEXT NOT NULL,
    comment_id TEXT NOT NULL UNIQUE,
    video_id TEXT NOT NULL,
    author_channel_id TEXT,
    author_name TEXT,
    text TEXT NOT NULL,
    like_count INTEGER,
    reply_count INTEGER,
    published_at TEXT,
    updated_at TEXT NOT NULL,
    latest_snapshot_id TEXT
);

CREATE TABLE IF NOT EXISTS comment_revisions (
    id TEXT PRIMARY KEY,
    comment_id TEXT NOT NULL,
    video_id TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    author_channel_id TEXT,
    author_name TEXT,
    text TEXT NOT NULL,
    like_count INTEGER,
    reply_count INTEGER,
    published_at TEXT,
    content_hash TEXT NOT NULL,
    changed_fields_json TEXT NOT NULL DEFAULT '[]',
    UNIQUE(comment_id, content_hash)
);

CREATE INDEX IF NOT EXISTS idx_comment_revisions_video_id_recorded_at
ON comment_revisions (video_id, recorded_at);

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

CREATE TABLE IF NOT EXISTS channel_tab_cursors (
    channel_id TEXT NOT NULL,
    tab_name TEXT NOT NULL,
    continuation_token TEXT,
    visitor_data TEXT,
    delegated_session_id TEXT,
    last_seen_published_at TEXT,
    backfill_complete INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (channel_id, tab_name)
);

CREATE TABLE IF NOT EXISTS channel_video_membership (
    channel_id TEXT NOT NULL,
    video_id TEXT NOT NULL,
    discovered_from_tab TEXT NOT NULL,
    discovered_at TEXT NOT NULL,
    PRIMARY KEY (channel_id, video_id, discovered_from_tab)
);
"#;
