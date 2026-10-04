use std::path::{Path, PathBuf};

use serde_json::Value;
use time::OffsetDateTime;
use tokio::process::Command;

use vessel_core::models::{
    Availability, ChannelMetadata, InputKind, InputRef, MediaFormat, Platform, SubtitleTrack,
    Thumbnail, VideoMetadata,
};
use vessel_core::{Result, VesselError};

use super::{ChannelVideoCrawlReport, ChannelVideoRef};

pub const YT_DLP_BACKEND_NAME: &str = "yt-dlp";

#[derive(Debug, Clone)]
pub struct YtDlpConfig {
    pub executable: PathBuf,
    pub cookies_from_browser: Option<String>,
}

impl Default for YtDlpConfig {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("yt-dlp"),
            cookies_from_browser: None,
        }
    }
}

impl YtDlpConfig {
    pub fn new(executable: impl Into<PathBuf>, cookies_from_browser: Option<String>) -> Self {
        Self {
            executable: executable.into(),
            cookies_from_browser,
        }
    }

    pub async fn doctor(&self) -> Result<String> {
        let output = Command::new(&self.executable)
            .arg("--version")
            .output()
            .await
            .map_err(|error| command_start_error(&self.executable, error))?;
        if !output.status.success() {
            return Err(command_failure(
                &self.executable,
                output.status.code(),
                &output.stderr,
            ));
        }
        let version = String::from_utf8(output.stdout)
            .map_err(|error| VesselError::Extractor(format!("yt-dlp version output was not UTF-8: {error}")))?;
        Ok(version.trim().to_owned())
    }

    pub async fn extract_video(&self, input: &InputRef) -> Result<VideoMetadata> {
        let target = video_target(input)?;
        let json = self.run_json(&target, false).await?;
        parse_video_json(json)
    }

    pub async fn discover_channel(
        &self,
        input: &InputRef,
    ) -> Result<(ChannelMetadata, ChannelVideoCrawlReport)> {
        let target = channel_target(input)?;
        let json = self.run_json(&target, true).await?;
        let channel = parse_channel_json(&json, &target)?;
        let crawl = parse_channel_listing_json(&json)?;
        Ok((channel, crawl))
    }

    pub async fn crawl_channel_videos(&self, input: &InputRef) -> Result<ChannelVideoCrawlReport> {
        self.discover_channel(input).await.map(|(_, crawl)| crawl)
    }

    pub async fn download_best_audio(
        &self,
        video_id: &str,
        output_template: &Path,
    ) -> Result<PathBuf> {
        let target = format!("https://www.youtube.com/watch?v={video_id}");
        let mut command = Command::new(&self.executable);
        command
            .arg("--no-config")
            .arg("--quiet")
            .arg("--no-warnings")
            .arg("--no-playlist")
            .arg("--format")
            .arg("bestaudio")
            .arg("--output")
            .arg(output_template)
            .arg("--print")
            .arg("after_move:filepath");

        if let Some(browser) = self.cookies_from_browser.as_deref() {
            command.arg("--cookies-from-browser").arg(browser);
        }
        command.arg(&target);

        let output = command
            .output()
            .await
            .map_err(|error| command_start_error(&self.executable, error))?;
        if !output.status.success() {
            return Err(command_failure(
                &self.executable,
                output.status.code(),
                &output.stderr,
            ));
        }

        let stdout = String::from_utf8(output.stdout).map_err(|error| {
            VesselError::Extractor(format!("yt-dlp download output was not UTF-8: {error}"))
        })?;
        let path = stdout
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                VesselError::Extractor(format!(
                    "yt-dlp downloaded audio for {video_id} but did not report the output path"
                ))
            })?;
        if !path.is_file() {
            return Err(VesselError::Extractor(format!(
                "yt-dlp reported audio output {} but the file does not exist",
                path.display()
            )));
        }
        Ok(path)
    }

    async fn run_json(&self, target: &str, flat_playlist: bool) -> Result<Value> {
        let mut command = Command::new(&self.executable);
        command
            .arg("--no-config")
            .arg("--quiet")
            .arg("--no-warnings")
            .arg("--dump-single-json")
            .arg("--skip-download");

        if flat_playlist {
            command
                .arg("--flat-playlist")
                .arg("--extractor-args")
                .arg("youtubetab:approximate_date");
        }
        if let Some(browser) = self.cookies_from_browser.as_deref() {
            command.arg("--cookies-from-browser").arg(browser);
        }
        command.arg(target);

        let output = command
            .output()
            .await
            .map_err(|error| command_start_error(&self.executable, error))?;

        if !output.status.success() {
            return Err(command_failure(
                &self.executable,
                output.status.code(),
                &output.stderr,
            ));
        }

        serde_json::from_slice(&output.stdout).map_err(|error| {
            VesselError::Extractor(format!(
                "yt-dlp returned invalid JSON for {target:?}: {error}"
            ))
        })
    }
}

fn command_start_error(executable: &Path, error: std::io::Error) -> VesselError {
    VesselError::Extractor(format!(
        "failed to start yt-dlp executable {}: {error}; install yt-dlp or pass an explicit executable",
        executable.display()
    ))
}

fn command_failure(executable: &Path, code: Option<i32>, stderr: &[u8]) -> VesselError {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    VesselError::Extractor(format!(
        "yt-dlp executable {} failed with status {}{}",
        executable.display(),
        code.map(|value| value.to_string()).unwrap_or_else(|| "signal".into()),
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    ))
}

fn video_target(input: &InputRef) -> Result<String> {
    match input.kind {
        InputKind::VideoId => Ok(format!("https://www.youtube.com/watch?v={}", input.raw)),
        InputKind::Url => Ok(input.raw.clone()),
        InputKind::ChannelId | InputKind::PlaylistId => Err(VesselError::Unsupported(
            "yt-dlp video extraction expects a video URL or video id".into(),
        )),
    }
}

fn channel_target(input: &InputRef) -> Result<String> {
    match input.kind {
        InputKind::ChannelId => Ok(format!("https://www.youtube.com/channel/{}", input.raw)),
        InputKind::Url => Ok(input.raw.clone()),
        InputKind::VideoId | InputKind::PlaylistId => Err(VesselError::Unsupported(
            "yt-dlp channel discovery expects a channel URL or channel id".into(),
        )),
    }
}

pub fn parse_video_json(raw: Value) -> Result<VideoMetadata> {
    let video_id = required_string(&raw, "id")?;
    let url = string(&raw, "webpage_url")
        .or_else(|| string(&raw, "original_url"))
        .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={video_id}"));

    let duration_seconds = raw
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value.round() as u64);

    let release_timestamp = raw
        .get("release_timestamp")
        .or_else(|| raw.get("timestamp"))
        .and_then(Value::as_i64)
        .and_then(|timestamp| OffsetDateTime::from_unix_timestamp(timestamp).ok());

    let formats = raw
        .get("formats")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_format).collect())
        .unwrap_or_default();

    let mut subtitles = parse_subtitle_map(raw.get("subtitles"), false);
    subtitles.extend(parse_subtitle_map(raw.get("automatic_captions"), true));

    let thumbnails = raw
        .get("thumbnails")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_thumbnail).collect())
        .unwrap_or_default();

    Ok(VideoMetadata {
        platform: Platform::YouTube,
        video_id,
        channel_id: string(&raw, "channel_id").or_else(|| string(&raw, "uploader_id")),
        url,
        title: string(&raw, "title"),
        description: string(&raw, "description"),
        duration_seconds,
        upload_date: string(&raw, "upload_date"),
        release_timestamp,
        tags: string_array(&raw, "tags"),
        categories: string_array(&raw, "categories"),
        primary_category: raw
            .get("categories")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .map(str::to_owned),
        view_count: unsigned(&raw, "view_count"),
        like_count: unsigned(&raw, "like_count"),
        comment_count: unsigned(&raw, "comment_count"),
        availability: parse_availability(string(&raw, "availability").as_deref()),
        formats,
        subtitles,
        thumbnails,
        fetched_at: OffsetDateTime::now_utc(),
        raw,
    })
}

pub fn parse_channel_json(raw: &Value, requested_url: &str) -> Result<ChannelMetadata> {
    let channel_id = string(raw, "channel_id")
        .or_else(|| {
            string(raw, "id").filter(|value| value.starts_with("UC"))
        })
        .ok_or_else(|| {
            VesselError::Extractor(
                "yt-dlp channel JSON is missing a stable YouTube channel_id".into(),
            )
        })?;

    let handle = string(raw, "uploader_id").filter(|value| value.starts_with('@'));
    let url = string(raw, "channel_url")
        .or_else(|| string(raw, "webpage_url"))
        .unwrap_or_else(|| requested_url.to_owned());

    let thumbnails = raw
        .get("thumbnails")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let avatar_url = thumbnails
        .iter()
        .find(|value| {
            value
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.contains("avatar"))
        })
        .and_then(|value| value.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let banner_url = thumbnails
        .iter()
        .find(|value| {
            value
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.contains("banner"))
        })
        .and_then(|value| value.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    Ok(ChannelMetadata {
        platform: Platform::YouTube,
        channel_id,
        handle,
        url,
        title: string(raw, "channel")
            .or_else(|| string(raw, "uploader"))
            .or_else(|| string(raw, "title")),
        description: string(raw, "description"),
        subscriber_count: unsigned(raw, "channel_follower_count"),
        video_count: unsigned(raw, "playlist_count"),
        view_count: unsigned(raw, "view_count"),
        avatar_url,
        banner_url,
        fetched_at: OffsetDateTime::now_utc(),
        raw: raw.clone(),
    })
}

pub fn parse_channel_listing_json(raw: &Value) -> Result<ChannelVideoCrawlReport> {
    let entries = raw
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| VesselError::Extractor("yt-dlp channel JSON did not contain entries".into()))?;

    let mut videos = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut videos_per_tab = std::collections::BTreeMap::<String, usize>::new();
    let mut tabs_visited = Vec::<String>::new();

    collect_channel_entries(
        entries,
        infer_channel_tab(raw).as_deref().unwrap_or("uploads"),
        &mut videos,
        &mut seen,
        &mut videos_per_tab,
        &mut tabs_visited,
    );

    if videos.is_empty() && !entries.is_empty() {
        return Err(VesselError::Extractor(
            "yt-dlp channel JSON contained entries but no video entries could be identified".into(),
        ));
    }

    Ok(ChannelVideoCrawlReport {
        videos,
        cursors: Vec::new(),
        videos_per_tab,
        tabs_completed: tabs_visited.clone(),
        tabs_visited,
        tabs_resumed_from_checkpoint: Vec::new(),
    })
}

fn collect_channel_entries(
    entries: &[Value],
    inherited_tab: &str,
    videos: &mut Vec<ChannelVideoRef>,
    seen: &mut std::collections::HashSet<String>,
    videos_per_tab: &mut std::collections::BTreeMap<String, usize>,
    tabs_visited: &mut Vec<String>,
) {
    if !tabs_visited.iter().any(|tab| tab == inherited_tab) {
        tabs_visited.push(inherited_tab.to_owned());
    }

    for entry in entries {
        let tab = infer_channel_tab(entry).unwrap_or_else(|| inherited_tab.to_owned());

        if let Some(nested) = entry.get("entries").and_then(Value::as_array) {
            collect_channel_entries(
                nested,
                &tab,
                videos,
                seen,
                videos_per_tab,
                tabs_visited,
            );
            continue;
        }

        if !looks_like_video_entry(entry) {
            continue;
        }
        let Some(video_id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        if video_id.trim().is_empty() || !seen.insert(video_id.to_owned()) {
            continue;
        }

        *videos_per_tab.entry(tab.clone()).or_insert(0) += 1;
        if !tabs_visited.iter().any(|existing| existing == &tab) {
            tabs_visited.push(tab.clone());
        }

        videos.push(ChannelVideoRef {
            video_id: video_id.to_owned(),
            tab_name: tab,
            title: string(entry, "title"),
            published_at: string(entry, "upload_date")
                .or_else(|| string(entry, "release_date")),
        });
    }
}

fn looks_like_video_entry(value: &Value) -> bool {
    if value.get("entries").is_some() {
        return false;
    }

    let id = value.get("id").and_then(Value::as_str).unwrap_or_default();
    if id.is_empty() || id.starts_with("UC") || id.starts_with("UU") {
        return false;
    }

    let ie_key = value.get("ie_key").and_then(Value::as_str).unwrap_or_default();
    if ie_key.eq_ignore_ascii_case("Youtube") {
        return true;
    }

    let url = value
        .get("url")
        .or_else(|| value.get("webpage_url"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    url.contains("watch?v=") || url.contains("/shorts/") || id.len() == 11
}

fn infer_channel_tab(value: &Value) -> Option<String> {
    for key in ["webpage_url", "url", "original_url"] {
        if let Some(url) = value.get(key).and_then(Value::as_str) {
            if url.contains("/shorts") {
                return Some("shorts".into());
            }
            if url.contains("/streams") || url.contains("/live") {
                return Some("streams".into());
            }
            if url.contains("/videos") {
                return Some("videos".into());
            }
        }
    }

    if let Some(title) = value.get("title").and_then(Value::as_str) {
        let lower = title.to_ascii_lowercase();
        if lower.ends_with(" - shorts") {
            return Some("shorts".into());
        }
        if lower.ends_with(" - streams") || lower.ends_with(" - live") {
            return Some("streams".into());
        }
        if lower.ends_with(" - videos") {
            return Some("videos".into());
        }
        if lower.starts_with("uploads from ") {
            return Some("uploads".into());
        }
    }

    None
}

fn parse_subtitle_map(value: Option<&Value>, automatic: bool) -> Vec<SubtitleTrack> {
    let Some(map) = value.and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut tracks = Vec::new();
    for (language, formats) in map {
        let Some(formats) = formats.as_array() else {
            continue;
        };
        let chosen = formats
            .iter()
            .rev()
            .find_map(|format| format.get("url").and_then(Value::as_str));
        if let Some(url) = chosen {
            tracks.push(SubtitleTrack {
                language: language.clone(),
                url: Some(url.to_owned()),
                is_auto_generated: automatic,
            });
        }
    }
    tracks
}

fn parse_thumbnail(value: &Value) -> Option<Thumbnail> {
    Some(Thumbnail {
        url: value.get("url")?.as_str()?.to_owned(),
        width: value
            .get("width")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        height: value
            .get("height")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
    })
}

fn parse_format(value: &Value) -> Option<MediaFormat> {
    let format_id = value.get("format_id")?.as_str()?.to_owned();
    let ext = value
        .get("ext")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let vcodec = string(value, "vcodec");
    let acodec = string(value, "acodec");
    let has_video = vcodec.as_deref().is_some_and(|codec| codec != "none");
    let has_audio = acodec.as_deref().is_some_and(|codec| codec != "none");

    Some(MediaFormat {
        format_id,
        ext,
        note: string(value, "format_note"),
        video_codec: vcodec.filter(|codec| codec != "none"),
        audio_codec: acodec.filter(|codec| codec != "none"),
        download_url: string(value, "url"),
        protocol: string(value, "protocol"),
        width: unsigned(value, "width").and_then(|value| u32::try_from(value).ok()),
        height: unsigned(value, "height").and_then(|value| u32::try_from(value).ok()),
        bitrate: value
            .get("tbr")
            .or_else(|| value.get("abr"))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| value.round() as u64),
        has_video,
        has_audio,
    })
}

fn parse_availability(value: Option<&str>) -> Availability {
    match value {
        Some("public") => Availability::Public,
        Some("unlisted") => Availability::Unlisted,
        Some("private") => Availability::Private,
        Some("premium_only") | Some("subscriber_only") | Some("needs_auth") => {
            Availability::MembersOnly
        }
        Some("deleted") => Availability::Deleted,
        _ => Availability::Unknown,
    }
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn required_string(value: &Value, key: &str) -> Result<String> {
    string(value, key).filter(|value| !value.is_empty()).ok_or_else(|| {
        VesselError::Extractor(format!("yt-dlp JSON is missing required field {key:?}"))
    })
}

fn unsigned(value: &Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(|value| value.as_u64().or_else(|| value.as_f64().map(|value| value.max(0.0) as u64)))
}

fn string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_video_metadata_and_separates_caption_classes() {
        let raw = serde_json::json!({
            "id": "abc123",
            "webpage_url": "https://www.youtube.com/watch?v=abc123",
            "channel_id": "UC_TEST",
            "title": "Example",
            "description": "desc",
            "duration": 12.4,
            "upload_date": "20261003",
            "timestamp": 1790985600,
            "view_count": 10,
            "like_count": 2,
            "comment_count": 1,
            "availability": "public",
            "tags": ["one", "two"],
            "categories": ["Education"],
            "subtitles": {
                "en": [
                    {"ext": "vtt", "url": "https://example.test/manual?v=1"},
                    {"ext": "json3", "url": "https://example.test/manual?v=2"}
                ]
            },
            "automatic_captions": {
                "es": [{"ext": "json3", "url": "https://example.test/auto"}]
            },
            "formats": [{
                "format_id": "251",
                "ext": "webm",
                "vcodec": "none",
                "acodec": "opus",
                "url": "https://example.test/audio",
                "protocol": "https",
                "abr": 128.0
            }],
            "thumbnails": [{"url": "https://example.test/thumb.jpg", "width": 320, "height": 180}]
        });

        let video = parse_video_json(raw).expect("video");
        assert_eq!(video.video_id, "abc123");
        assert_eq!(video.channel_id.as_deref(), Some("UC_TEST"));
        assert_eq!(video.duration_seconds, Some(12));
        assert_eq!(video.subtitles.len(), 2);
        assert!(video.subtitles.iter().any(|track| !track.is_auto_generated && track.language == "en"));
        assert!(video.subtitles.iter().any(|track| track.is_auto_generated && track.language == "es"));
        assert_eq!(video.formats.len(), 1);
        assert!(!video.formats[0].has_video);
        assert!(video.formats[0].has_audio);
    }

    #[test]
    fn parses_flat_channel_listing_without_cursor_state() {
        let raw = serde_json::json!({
            "webpage_url": "https://www.youtube.com/@example/videos",
            "entries": [
                {"id": "abc123def45", "title": "One", "upload_date": "20261001"},
                {"id": "xyz987uvw65", "title": "Two"}
            ]
        });

        let report = parse_channel_listing_json(&raw).expect("listing");
        assert_eq!(report.videos.len(), 2);
        assert_eq!(report.videos[0].video_id, "abc123def45");
        assert_eq!(report.videos[0].tab_name, "videos");
        assert!(report.cursors.is_empty());
        assert_eq!(report.tabs_completed, vec!["videos"]);
    }

    #[test]
    fn recursively_flattens_multi_tab_channel_output() {
        let raw = serde_json::json!({
            "id": "UC_TEST",
            "channel_id": "UC_TEST",
            "channel": "Example",
            "entries": [
                {
                    "title": "Example - Videos",
                    "webpage_url": "https://www.youtube.com/@example/videos",
                    "entries": [
                        {"id": "abc123def45", "ie_key": "Youtube", "url": "https://www.youtube.com/watch?v=abc123def45"}
                    ]
                },
                {
                    "title": "Example - Shorts",
                    "webpage_url": "https://www.youtube.com/@example/shorts",
                    "entries": [
                        {"id": "shr123def45", "ie_key": "Youtube", "url": "https://www.youtube.com/shorts/shr123def45"}
                    ]
                },
                {
                    "title": "Example - Streams",
                    "webpage_url": "https://www.youtube.com/@example/streams",
                    "entries": [
                        {"id": "liv123def45", "ie_key": "Youtube", "url": "https://www.youtube.com/watch?v=liv123def45"},
                        {"id": "abc123def45", "ie_key": "Youtube", "url": "https://www.youtube.com/watch?v=abc123def45"}
                    ]
                }
            ]
        });

        let report = parse_channel_listing_json(&raw).expect("listing");
        assert_eq!(report.videos.len(), 3);
        assert_eq!(report.videos[0].tab_name, "videos");
        assert_eq!(report.videos[1].tab_name, "shorts");
        assert_eq!(report.videos[2].tab_name, "streams");
        assert_eq!(report.videos_per_tab["videos"], 1);
        assert_eq!(report.videos_per_tab["shorts"], 1);
        assert_eq!(report.videos_per_tab["streams"], 1);
        assert!(report.cursors.is_empty());
    }

    #[test]
    fn parses_channel_identity_from_yt_dlp_metadata() {
        let raw = serde_json::json!({
            "id": "UC_TEST",
            "channel_id": "UC_TEST",
            "channel": "Example",
            "uploader_id": "@example",
            "channel_url": "https://www.youtube.com/channel/UC_TEST",
            "description": "desc",
            "channel_follower_count": 42,
            "entries": []
        });

        let channel = parse_channel_json(&raw, "https://www.youtube.com/@example").expect("channel");
        assert_eq!(channel.channel_id, "UC_TEST");
        assert_eq!(channel.handle.as_deref(), Some("@example"));
        assert_eq!(channel.title.as_deref(), Some("Example"));
        assert_eq!(channel.subscriber_count, Some(42));
    }

    #[test]
    fn video_ids_are_promoted_to_canonical_watch_urls() {
        let input = InputRef {
            raw: "abc123".into(),
            kind: InputKind::VideoId,
        };
        assert_eq!(
            video_target(&input).unwrap(),
            "https://www.youtube.com/watch?v=abc123"
        );
    }
}
