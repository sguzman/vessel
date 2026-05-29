pub const MIGRATIONS: &[(&str, &str)] = &[
(
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

CREATE TABLE IF NOT EXISTS subtitle_snapshots (
    id TEXT PRIMARY KEY,
    video_id TEXT NOT NULL,
    language TEXT NOT NULL,
    is_auto_generated INTEGER NOT NULL,
    fetched_at TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    normalized_json TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    changed_fields_json TEXT,
    UNIQUE(video_id, language, is_auto_generated, content_hash)
);

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

CREATE TABLE IF NOT EXISTS comment_snapshots (
    id TEXT PRIMARY KEY,
    comment_id TEXT NOT NULL,
    video_id TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    normalized_json TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    changed_fields_json TEXT,
    UNIQUE(comment_id, content_hash)
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
),
(
    "0002_native_parity_schema",
    r#"
ALTER TABLE videos ADD COLUMN primary_category TEXT;
ALTER TABLE videos ADD COLUMN tags_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE videos ADD COLUMN dislike_count INTEGER;

ALTER TABLE video_snapshots ADD COLUMN title TEXT;
ALTER TABLE video_snapshots ADD COLUMN primary_category TEXT;
ALTER TABLE video_snapshots ADD COLUMN tags_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE video_snapshots ADD COLUMN view_count INTEGER;
ALTER TABLE video_snapshots ADD COLUMN like_count INTEGER;
ALTER TABLE video_snapshots ADD COLUMN dislike_count INTEGER;
ALTER TABLE video_snapshots ADD COLUMN comment_count INTEGER;

ALTER TABLE channel_snapshots ADD COLUMN title TEXT;
ALTER TABLE channel_snapshots ADD COLUMN description TEXT;
ALTER TABLE channel_snapshots ADD COLUMN subscriber_count INTEGER;
ALTER TABLE channel_snapshots ADD COLUMN video_count INTEGER;
ALTER TABLE channel_snapshots ADD COLUMN view_count INTEGER;
ALTER TABLE channel_snapshots ADD COLUMN avatar_url TEXT;
ALTER TABLE channel_snapshots ADD COLUMN banner_url TEXT;

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

UPDATE videos
SET
    primary_category = (
        SELECT json_extract(video_snapshots.normalized_json, '$.primary_category')
        FROM video_snapshots
        WHERE video_snapshots.video_id = videos.video_id
        ORDER BY video_snapshots.fetched_at DESC
        LIMIT 1
    ),
    tags_json = COALESCE((
        SELECT json_extract(video_snapshots.normalized_json, '$.tags')
        FROM video_snapshots
        WHERE video_snapshots.video_id = videos.video_id
        ORDER BY video_snapshots.fetched_at DESC
        LIMIT 1
    ), '[]'),
    dislike_count = (
        SELECT json_extract(video_snapshots.normalized_json, '$.dislike_count')
        FROM video_snapshots
        WHERE video_snapshots.video_id = videos.video_id
        ORDER BY video_snapshots.fetched_at DESC
        LIMIT 1
    );

UPDATE video_snapshots
SET
    title = json_extract(normalized_json, '$.title'),
    primary_category = json_extract(normalized_json, '$.primary_category'),
    tags_json = COALESCE(json_extract(normalized_json, '$.tags'), '[]'),
    view_count = json_extract(normalized_json, '$.view_count'),
    like_count = json_extract(normalized_json, '$.like_count'),
    dislike_count = json_extract(normalized_json, '$.dislike_count'),
    comment_count = json_extract(normalized_json, '$.comment_count');

UPDATE channel_snapshots
SET
    title = json_extract(normalized_json, '$.title'),
    description = json_extract(normalized_json, '$.description'),
    subscriber_count = json_extract(normalized_json, '$.subscriber_count'),
    video_count = json_extract(normalized_json, '$.video_count'),
    view_count = json_extract(normalized_json, '$.view_count'),
    avatar_url = json_extract(normalized_json, '$.avatar_url'),
    banner_url = json_extract(normalized_json, '$.banner_url');
"#,
),
];
