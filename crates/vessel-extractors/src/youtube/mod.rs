use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use time::OffsetDateTime;
use url::Url;

use vessel_core::Result;
use vessel_core::VesselError;
use vessel_core::models::{
    Availability, ChannelMetadata, CommentMetadata, InputKind, InputRef, MediaFormat, Platform,
    SubtitleTrack, Thumbnail, VideoMetadata,
};

use crate::traits::{ExtractContext, ExtractRequest, ExtractedItem, Extractor, SupportLevel};

#[derive(Debug, Default)]
pub struct YoutubeExtractor;

#[derive(Debug, Clone)]
pub struct ChannelVideoRef {
    pub video_id: String,
    pub tab_name: String,
    pub title: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelTabCursor {
    pub tab_name: String,
    pub continuation_token: Option<String>,
    pub visitor_data: Option<String>,
    pub delegated_session_id: Option<String>,
    pub last_seen_published_at: Option<String>,
    pub backfill_complete: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelVideoCrawlReport {
    pub videos: Vec<ChannelVideoRef>,
    pub cursors: Vec<ChannelTabCursor>,
    pub videos_per_tab: BTreeMap<String, usize>,
    pub tabs_visited: Vec<String>,
    pub tabs_completed: Vec<String>,
    pub tabs_resumed_from_checkpoint: Vec<String>,
}

#[async_trait]
impl Extractor for YoutubeExtractor {
    fn name(&self) -> &str {
        "youtube"
    }

    fn supports(&self, input: &InputRef) -> SupportLevel {
        match input.kind {
            InputKind::Url
                if input.raw.contains("youtube.com") || input.raw.contains("youtu.be") =>
            {
                SupportLevel::Native
            }
            InputKind::VideoId | InputKind::ChannelId | InputKind::PlaylistId => {
                SupportLevel::Native
            }
            InputKind::Url => SupportLevel::Unsupported,
        }
    }

    async fn extract(
        &self,
        request: ExtractRequest,
        _ctx: ExtractContext,
    ) -> Result<ExtractedItem> {
        if is_channel_input(&request.input) {
            return extract_channel(&request.input)
                .await
                .map(ExtractedItem::Channel);
        }

        extract_video(&request.input)
            .await
            .map(ExtractedItem::Video)
    }
}

pub async fn extract_video(input: &InputRef) -> Result<VideoMetadata> {
    let video_id = canonical_video_id(input)?;
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let html = fetch_text(&url).await?;
    let initial_data = extract_embedded_json(&html, "var ytInitialData = ")
        .or_else(|| extract_embedded_json(&html, "ytInitialData = "));
    let mut player_response = extract_embedded_json(&html, "var ytInitialPlayerResponse = ")
        .or_else(|| extract_embedded_json(&html, "ytInitialPlayerResponse = "))
        .ok_or_else(|| {
            VesselError::Extractor(
                "failed to locate ytInitialPlayerResponse in watch page".to_owned(),
            )
        })?;
    if let Some(api_key) = extract_config_string(&html, "\"INNERTUBE_API_KEY\":\"") {
        if let Ok(android_response) =
            fetch_player_response(&api_key, &video_id, "ANDROID", "20.10.38").await
        {
            merge_streaming_data(&mut player_response, &android_response);
        }
    }
    let video = parse_video_metadata(
        &player_response,
        initial_data.as_ref(),
        &html,
        &url,
        OffsetDateTime::now_utc(),
    )?;
    Ok(video)
}

pub async fn extract_comments(
    input: &InputRef,
    max_comments: usize,
) -> Result<Vec<CommentMetadata>> {
    let video_id = canonical_video_id(input)?;
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let html = fetch_text(&url).await?;
    let initial_data = extract_embedded_json(&html, "var ytInitialData = ")
        .or_else(|| extract_embedded_json(&html, "ytInitialData = "))
        .ok_or_else(|| {
            VesselError::Extractor("failed to locate ytInitialData in watch page".to_owned())
        })?;
    let api_key = extract_config_string(&html, "\"INNERTUBE_API_KEY\":\"")
        .ok_or_else(|| VesselError::Extractor("missing INNERTUBE_API_KEY".to_owned()))?;
    let client_version = extract_config_string(&html, "\"INNERTUBE_CLIENT_VERSION\":\"")
        .ok_or_else(|| VesselError::Extractor("missing INNERTUBE_CLIENT_VERSION".to_owned()))?;
    let visitor_data = extract_config_string(&html, "\"visitorData\":\"")
        .ok_or_else(|| VesselError::Extractor("missing visitorData".to_owned()))?;
    let continuation = find_first_comment_continuation(&initial_data).ok_or_else(|| {
        VesselError::Unsupported(
            "youtube watch page did not expose a comment continuation".to_owned(),
        )
    })?;
    let fetched_at = OffsetDateTime::now_utc();
    let mut comments = Vec::new();
    let mut next_token = Some(continuation);

    while let Some(token) = next_token.take() {
        let page = fetch_comment_page(&api_key, &client_version, &visitor_data, &token).await?;
        comments.extend(parse_comment_page(
            &video_id,
            &page,
            fetched_at,
            max_comments.saturating_sub(comments.len()),
        ));
        if comments.len() >= max_comments {
            break;
        }
        next_token = find_next_comment_continuation(&page);
    }

    Ok(comments)
}

pub async fn extract_channel(input: &InputRef) -> Result<ChannelMetadata> {
    let canonical_url = canonical_channel_url(input)?;
    let html = fetch_text(&canonical_url).await?;
    let initial_data = extract_embedded_json(&html, "var ytInitialData = ")
        .or_else(|| extract_embedded_json(&html, "ytInitialData = "))
        .ok_or_else(|| {
        VesselError::Extractor("failed to locate ytInitialData in channel page".to_owned())
    })?;
    let api_key = extract_config_string(&html, "\"INNERTUBE_API_KEY\":\"");
    let client_version = extract_config_string(&html, "\"INNERTUBE_CLIENT_VERSION\":\"");
    let visitor_data = extract_config_string(&html, "\"visitorData\":\"");
    let about_data = if let (Some(api_key), Some(client_version), Some(visitor_data)) =
        (api_key.as_deref(), client_version.as_deref(), visitor_data.as_deref())
    {
        fetch_channel_tab(&initial_data, api_key, client_version, visitor_data, "about")
            .await
            .ok()
    } else {
        None
    };
    parse_channel_metadata(
        &initial_data,
        about_data.as_ref(),
        &canonical_url,
        OffsetDateTime::now_utc(),
    )
}

pub async fn crawl_channel_videos(
    input: &InputRef,
    existing_cursors: &[ChannelTabCursor],
) -> Result<ChannelVideoCrawlReport> {
    let canonical_url = canonical_channel_url(input)?;
    let html = fetch_text(&canonical_url).await?;
    let initial_data = extract_embedded_json(&html, "var ytInitialData = ")
        .or_else(|| extract_embedded_json(&html, "ytInitialData = "))
        .ok_or_else(|| {
            VesselError::Extractor("failed to locate ytInitialData in channel page".to_owned())
        })?;
    let api_key = extract_config_string(&html, "\"INNERTUBE_API_KEY\":\"")
        .ok_or_else(|| VesselError::Extractor("missing INNERTUBE_API_KEY".to_owned()))?;
    let client_version = extract_config_string(&html, "\"INNERTUBE_CLIENT_VERSION\":\"")
        .ok_or_else(|| VesselError::Extractor("missing INNERTUBE_CLIENT_VERSION".to_owned()))?;
    let visitor_data = extract_config_string(&html, "\"visitorData\":\"")
        .ok_or_else(|| VesselError::Extractor("missing visitorData".to_owned()))?;
    crawl_channel_tabs(
        &initial_data,
        &api_key,
        &client_version,
        &visitor_data,
        existing_cursors,
    )
    .await
}

fn is_channel_input(input: &InputRef) -> bool {
    matches!(input.kind, InputKind::ChannelId)
        || (matches!(input.kind, InputKind::Url)
            && (input.raw.contains("/channel/")
                || input.raw.contains("/@")
                || input.raw.contains("/c/")
                || input.raw.contains("/user/")))
}

async fn fetch_text(url: &str) -> Result<String> {
    let client = http_client()?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| VesselError::Extractor(format!("youtube request failed: {err}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(VesselError::Extractor(format!(
            "youtube request returned http status {status}"
        )));
    }
    response
        .text()
        .await
        .map_err(|err| VesselError::Extractor(format!("youtube response decode failed: {err}")))
}

fn http_client() -> Result<Client> {
    Client::builder()
        .user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36",
        )
        .build()
        .map_err(|err| VesselError::Extractor(format!("http client build failed: {err}")))
}

fn canonical_video_id(input: &InputRef) -> Result<String> {
    match input.kind {
        InputKind::VideoId => Ok(input.raw.clone()),
        InputKind::Url => parse_video_id_from_url(&input.raw),
        InputKind::ChannelId | InputKind::PlaylistId => Err(VesselError::Unsupported(
            "youtube video extraction expects a video url or video id".to_owned(),
        )),
    }
}

fn canonical_channel_url(input: &InputRef) -> Result<String> {
    match input.kind {
        InputKind::ChannelId => Ok(format!("https://www.youtube.com/channel/{}", input.raw)),
        InputKind::Url => {
            let url = Url::parse(&input.raw)
                .map_err(|err| VesselError::Extractor(format!("invalid youtube url: {err}")))?;
            let host = url
                .host_str()
                .ok_or_else(|| VesselError::Extractor("youtube url missing host".to_owned()))?;
            if !host.ends_with("youtube.com") {
                return Err(VesselError::Unsupported(
                    "unsupported youtube channel url host".to_owned(),
                ));
            }
            Ok(input.raw.clone())
        }
        _ => Err(VesselError::Unsupported(
            "youtube channel extraction expects a channel url or channel id".to_owned(),
        )),
    }
}

fn parse_video_id_from_url(raw: &str) -> Result<String> {
    let url = Url::parse(raw)
        .map_err(|err| VesselError::Extractor(format!("invalid youtube url: {err}")))?;
    if let Some(host) = url.host_str() {
        if host == "youtu.be" || host == "www.youtu.be" {
            let mut segments = url.path_segments().ok_or_else(|| {
                VesselError::Extractor("youtu.be url has no path segments".to_owned())
            })?;
            if let Some(id) = segments.next() {
                if !id.is_empty() {
                    return Ok(id.to_owned());
                }
            }
        }
        if host.ends_with("youtube.com") {
            if let Some((_, value)) = url.query_pairs().find(|(key, _)| key == "v") {
                if !value.is_empty() {
                    return Ok(value.into_owned());
                }
            }
            let segments: Vec<_> = url
                .path_segments()
                .map(|segments| segments.collect())
                .unwrap_or_default();
            if segments.len() >= 2 && segments[0] == "shorts" {
                return Ok(segments[1].to_owned());
            }
        }
    }
    Err(VesselError::Unsupported(
        "unsupported youtube url shape; expected watch, short, or youtu.be link".to_owned(),
    ))
}

fn extract_embedded_json(html: &str, marker: &str) -> Option<Value> {
    let start = html.find(marker)? + marker.len();
    let slice = &html[start..];
    let object_start = slice.find('{')?;
    let json_text = take_balanced_json(&slice[object_start..])?;
    serde_json::from_str(json_text).ok()
}

fn extract_config_string(input: &str, marker: &str) -> Option<String> {
    let start = input.find(marker)? + marker.len();
    let rest = &input[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

async fn fetch_player_response(
    api_key: &str,
    video_id: &str,
    client_name: &str,
    client_version: &str,
) -> Result<Value> {
    let client = http_client()?;
    let response = client
        .post(format!(
            "https://www.youtube.com/youtubei/v1/player?key={api_key}"
        ))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&serde_json::json!({
            "videoId": video_id,
            "context": {
                "client": {
                    "clientName": client_name,
                    "clientVersion": client_version,
                }
            }
        }))
        .send()
        .await
        .map_err(|err| VesselError::Extractor(format!("youtube player request failed: {err}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(VesselError::Extractor(format!(
            "youtube player request returned http status {status}"
        )));
    }
    response.json::<Value>().await.map_err(|err| {
        VesselError::Extractor(format!("youtube player response decode failed: {err}"))
    })
}

fn merge_streaming_data(into: &mut Value, from: &Value) {
    let Some(streaming_data) = from.get("streamingData").cloned() else {
        return;
    };

    if let Some(into) = into.as_object_mut() {
        into.insert("streamingData".to_owned(), streaming_data);
    }
}

fn take_balanced_json(input: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&input[..=index]);
                }
            }
            _ => {}
        }
    }

    None
}

fn parse_channel_metadata(
    raw: &Value,
    about: Option<&Value>,
    url: &str,
    fetched_at: OffsetDateTime,
) -> Result<ChannelMetadata> {
    let metadata = find_object_with_key(raw, "channelMetadataRenderer")
        .ok_or_else(|| VesselError::Extractor("channelMetadataRenderer missing".to_owned()))?;
    let channel_id = metadata
        .get("externalId")
        .and_then(Value::as_str)
        .ok_or_else(|| VesselError::Extractor("channel externalId missing".to_owned()))?;
    let handle = metadata
        .get("vanityChannelUrl")
        .and_then(Value::as_str)
        .and_then(|value| value.rsplit('/').next())
        .filter(|value| value.starts_with('@'))
        .map(ToOwned::to_owned);
    let avatar_url = metadata
        .get("avatar")
        .and_then(|value| value.get("thumbnails"))
        .and_then(Value::as_array)
        .and_then(|items| items.last())
        .and_then(|item| item.get("url"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let title = metadata
        .get("title")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let description = metadata
        .get("description")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let subscriber_count = about
        .and_then(|value| find_page_header_metadata_count(value, &["subscribers", "suscriptores"]))
        .or_else(|| find_page_header_metadata_count(raw, &["subscribers", "suscriptores"]))
        .or_else(|| about.and_then(|value| find_count_in_metadata_rows(value, &["subscribers", "suscriptores"])));
    let video_count = about
        .and_then(|value| find_page_header_metadata_count(value, &["videos", "video"]))
        .or_else(|| find_page_header_metadata_count(raw, &["videos", "video"]))
        .or_else(|| about.and_then(|value| find_count_in_about_renderer(value, "videoCountText")))
        .or_else(|| about.and_then(|value| find_count_in_metadata_rows(value, &["videos", "video"])));
    let view_count = about
        .and_then(|value| find_count_in_about_renderer(value, "viewCountText"))
        .or_else(|| about.and_then(|value| find_count_in_metadata_rows(value, &["views", "vistas"])));
    let banner_url = find_banner_url(raw).or_else(|| about.and_then(find_banner_url));

    Ok(ChannelMetadata {
        platform: Platform::YouTube,
        channel_id: channel_id.to_owned(),
        handle,
        url: metadata
            .get("channelUrl")
            .and_then(Value::as_str)
            .unwrap_or(url)
            .to_owned(),
        title,
        description,
        subscriber_count,
        video_count,
        view_count,
        avatar_url,
        banner_url,
        fetched_at,
        raw: merge_raw_payloads(raw, about),
    })
}

fn parse_video_metadata(
    raw: &Value,
    initial_data: Option<&Value>,
    html: &str,
    url: &str,
    fetched_at: OffsetDateTime,
) -> Result<VideoMetadata> {
    let details = raw
        .get("videoDetails")
        .and_then(Value::as_object)
        .ok_or_else(|| VesselError::Extractor("player response missing videoDetails".to_owned()))?;
    let microformat = raw
        .get("microformat")
        .and_then(|v| v.get("playerMicroformatRenderer"));
    let streaming_data = raw.get("streamingData");

    let video_id = string_field(details, "videoId")
        .ok_or_else(|| VesselError::Extractor("videoDetails.videoId missing".to_owned()))?;
    let thumbnails = details
        .get("thumbnail")
        .and_then(|v| v.get("thumbnails"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(Thumbnail {
                        url: item.get("url")?.as_str()?.to_owned(),
                        width: item.get("width").and_then(Value::as_u64).map(|v| v as u32),
                        height: item.get("height").and_then(Value::as_u64).map(|v| v as u32),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let formats = streaming_data.map(parse_formats).unwrap_or_default();
    let subtitles = parse_subtitles(raw);
    let tags = parse_video_tags(details, html);
    let categories = parse_video_categories(microformat, html);
    let primary_category = categories.first().cloned();
    let like_count = string_field(details, "likeCount")
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            microformat
                .and_then(|value| value.get("likeCount"))
                .and_then(Value::as_str)
                .and_then(|value| value.parse().ok())
        })
        .or_else(|| initial_data.and_then(parse_like_count_from_initial_data));
    let comment_count = initial_data.and_then(parse_comment_count_from_initial_data);

    Ok(VideoMetadata {
        platform: Platform::YouTube,
        video_id,
        channel_id: string_field(details, "channelId"),
        url: url.to_owned(),
        title: string_field(details, "title"),
        description: string_field(details, "shortDescription"),
        duration_seconds: string_field(details, "lengthSeconds")
            .and_then(|value| value.parse().ok()),
        upload_date: microformat
            .and_then(|value| value.get("uploadDate"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        release_timestamp: None,
        tags,
        categories,
        primary_category,
        view_count: string_field(details, "viewCount").and_then(|value| value.parse().ok()),
        like_count,
        comment_count,
        availability: parse_availability(raw, details),
        formats,
        subtitles,
        thumbnails,
        fetched_at,
        raw: raw.clone(),
    })
}

fn parse_formats(streaming_data: &Value) -> Vec<MediaFormat> {
    let mut formats = Vec::new();
    let iter = streaming_data
        .get("formats")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            streaming_data
                .get("adaptiveFormats")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        );

    formats.extend(iter.filter_map(|item| {
        let format_id = item.get("itag").and_then(Value::as_u64)?.to_string();
        let mime = item.get("mimeType").and_then(Value::as_str);
        let direct_url = item
            .get("url")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let (video_codec, audio_codec) = mime
            .and_then(extract_codec)
            .map(|(video, audio)| (Some(video.to_owned()), audio.map(ToOwned::to_owned)))
            .unwrap_or((None, None));
        let has_video = mime.map(|m| m.starts_with("video/")).unwrap_or(false)
            || item.get("width").is_some()
            || item.get("height").is_some();
        let has_audio = mime.map(|m| m.starts_with("audio/")).unwrap_or(false)
            || item.get("audioQuality").is_some()
            || audio_codec.is_some();
        Some(MediaFormat {
            format_id,
            ext: mime.map(mime_to_extension).unwrap_or("bin").to_owned(),
            note: item
                .get("qualityLabel")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            video_codec,
            audio_codec,
            download_url: direct_url,
            protocol: Some("https".to_owned()),
            width: item.get("width").and_then(Value::as_u64).map(|v| v as u32),
            height: item.get("height").and_then(Value::as_u64).map(|v| v as u32),
            bitrate: item.get("bitrate").and_then(Value::as_u64),
            has_video,
            has_audio,
        })
    }));

    if let Some(url) = streaming_data
        .get("serverAbrStreamingUrl")
        .and_then(Value::as_str)
    {
        formats.push(MediaFormat {
            format_id: "server-abr".to_owned(),
            ext: "mp4".to_owned(),
            note: Some("server abr".to_owned()),
            video_codec: None,
            audio_codec: None,
            download_url: Some(url.to_owned()),
            protocol: Some("https".to_owned()),
            width: None,
            height: None,
            bitrate: None,
            has_video: true,
            has_audio: true,
        });
    }

    formats
}

async fn fetch_channel_tab(
    initial_data: &Value,
    api_key: &str,
    client_version: &str,
    visitor_data: &str,
    tab_name: &str,
) -> Result<Value> {
    let (browse_id, params) = find_tab_browse_endpoint(initial_data, tab_name).ok_or_else(|| {
        VesselError::Unsupported(format!("youtube channel did not expose a {tab_name} tab"))
    })?;
    fetch_browse_page(
        api_key,
        client_version,
        visitor_data,
        Some(&browse_id),
        params.as_deref(),
        None,
    )
    .await
}

async fn crawl_channel_tabs(
    initial_data: &Value,
    api_key: &str,
    client_version: &str,
    visitor_data: &str,
    existing_cursors: &[ChannelTabCursor],
) -> Result<ChannelVideoCrawlReport> {
    let tabs = ["videos", "shorts", "streams"];
    let mut videos_by_id = BTreeMap::<String, ChannelVideoRef>::new();
    let mut cursors_out = Vec::new();
    let mut videos_per_tab = BTreeMap::new();
    let mut tabs_visited = Vec::new();
    let mut tabs_completed = Vec::new();
    let mut tabs_resumed_from_checkpoint = Vec::new();
    let existing_map = existing_cursors
        .iter()
        .map(|cursor| (cursor.tab_name.clone(), cursor.clone()))
        .collect::<HashMap<_, _>>();

    for tab_name in tabs {
        let existing = existing_map.get(tab_name).cloned().unwrap_or_default();
        let mut seen_continuations = HashSet::new();
        let mut tab_videos = Vec::new();
        let mut last_seen_published_at = existing.last_seen_published_at.clone();
        let mut current_visitor_data = existing
            .visitor_data
            .clone()
            .unwrap_or_else(|| visitor_data.to_owned());
        let initial_tab_data = match fetch_channel_tab(
            initial_data,
            api_key,
            client_version,
            visitor_data,
            tab_name,
        )
        .await
        {
            Ok(data) => data,
            Err(VesselError::Unsupported(_)) => continue,
            Err(err) => return Err(err),
        };
        tabs_visited.push(tab_name.to_owned());
        collect_channel_video_refs(&initial_tab_data, tab_name, &mut tab_videos);
        if let Some(max_published) = tab_videos
            .iter()
            .filter_map(|video| video.published_at.clone())
            .max()
        {
            last_seen_published_at = Some(max_published);
        }

        let mut next_continuation = if existing.backfill_complete {
            None
        } else {
            existing
                .continuation_token
                .clone()
                .or_else(|| find_continuation_token(&initial_tab_data))
        };
        if existing.continuation_token.is_some() {
            tabs_resumed_from_checkpoint.push(tab_name.to_owned());
        }

        while let Some(token) = next_continuation.take() {
            if !seen_continuations.insert(token.clone()) {
                break;
            }
            let page = fetch_browse_page(
                api_key,
                client_version,
                &current_visitor_data,
                None,
                None,
                Some(&token),
            )
            .await?;
            if let Some(next_visitor_data) = find_string_value(&page, "visitorData") {
                current_visitor_data = next_visitor_data;
            }
            collect_channel_video_refs(&page, tab_name, &mut tab_videos);
            if let Some(max_published) = tab_videos
                .iter()
                .filter_map(|video| video.published_at.clone())
                .max()
            {
                last_seen_published_at = Some(max_published);
            }
            next_continuation = find_continuation_token(&page);
            if existing.backfill_complete {
                break;
            }
        }

        let mut unique_for_tab = 0usize;
        for video in tab_videos {
            if videos_by_id.insert(video.video_id.clone(), video).is_none() {
                unique_for_tab += 1;
            }
        }
        videos_per_tab.insert(tab_name.to_owned(), unique_for_tab);
        let backfill_complete = next_continuation.is_none();
        if backfill_complete {
            tabs_completed.push(tab_name.to_owned());
        }
        cursors_out.push(ChannelTabCursor {
            tab_name: tab_name.to_owned(),
            continuation_token: next_continuation,
            visitor_data: Some(current_visitor_data),
            delegated_session_id: existing.delegated_session_id,
            last_seen_published_at,
            backfill_complete,
        });
    }

    Ok(ChannelVideoCrawlReport {
        videos: videos_by_id.into_values().collect(),
        cursors: cursors_out,
        videos_per_tab,
        tabs_visited,
        tabs_completed,
        tabs_resumed_from_checkpoint,
    })
}

async fn fetch_browse_page(
    api_key: &str,
    client_version: &str,
    visitor_data: &str,
    browse_id: Option<&str>,
    params: Option<&str>,
    continuation: Option<&str>,
) -> Result<Value> {
    let client = http_client()?;
    let mut body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "WEB",
                "clientVersion": client_version,
                "visitorData": visitor_data,
            }
        }
    });
    if let Some(browse_id) = browse_id {
        body["browseId"] = Value::String(browse_id.to_owned());
    }
    if let Some(params) = params {
        body["params"] = Value::String(params.to_owned());
    }
    if let Some(continuation) = continuation {
        body["continuation"] = Value::String(continuation.to_owned());
    }

    let response = client
        .post(format!(
            "https://www.youtube.com/youtubei/v1/browse?key={api_key}"
        ))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|err| VesselError::Extractor(format!("youtube browse request failed: {err}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(VesselError::Extractor(format!(
            "youtube browse request returned http status {status}"
        )));
    }
    response.json::<Value>().await.map_err(|err| {
        VesselError::Extractor(format!("youtube browse response decode failed: {err}"))
    })
}

fn find_tab_browse_endpoint(value: &Value, target_tab: &str) -> Option<(String, Option<String>)> {
    let tabs = value
        .get("contents")
        .and_then(|value| value.get("twoColumnBrowseResultsRenderer"))
        .and_then(|value| value.get("tabs"))
        .and_then(Value::as_array)?;
    tabs.iter().find_map(|tab| {
        let tab_renderer = tab.get("tabRenderer").or_else(|| tab.get("expandableTabRenderer"))?;
        let title = tab_renderer.get("title").and_then(Value::as_str)?.to_ascii_lowercase();
        if title != target_tab {
            return None;
        }
        let endpoint = tab_renderer.get("endpoint")?.get("browseEndpoint")?;
        let browse_id = endpoint.get("browseId")?.as_str()?.to_owned();
        let params = endpoint
            .get("params")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        Some((browse_id, params))
    })
}

fn collect_channel_video_refs(value: &Value, tab_name: &str, output: &mut Vec<ChannelVideoRef>) {
    match value {
        Value::Object(map) => {
            for key in [
                "videoRenderer",
                "gridVideoRenderer",
                "playlistVideoRenderer",
                "reelItemRenderer",
                "shortsLockupViewModel",
                "lockupViewModel",
            ] {
                if let Some(renderer) = map.get(key) {
                    if let Some(video) = parse_channel_video_renderer(renderer, key, tab_name) {
                        output.push(video);
                    }
                }
            }
            for child in map.values() {
                collect_channel_video_refs(child, tab_name, output);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_channel_video_refs(child, tab_name, output);
            }
        }
        _ => {}
    }
}

fn parse_channel_video_renderer(
    renderer: &Value,
    renderer_key: &str,
    tab_name: &str,
) -> Option<ChannelVideoRef> {
    let video_id = match renderer_key {
        "reelItemRenderer" => renderer
            .get("videoId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                renderer
                    .get("navigationEndpoint")
                    .and_then(|value| value.get("reelWatchEndpoint"))
                    .and_then(|value| value.get("videoId"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })?,
        "shortsLockupViewModel" => renderer
            .get("onTap")
            .and_then(|value| value.get("innertubeCommand"))
            .and_then(|value| value.get("reelWatchEndpoint"))
            .and_then(|value| value.get("videoId"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                renderer
                    .get("entityId")
                    .and_then(Value::as_str)
                    .and_then(extract_video_id_from_entity_id)
            })?,
        "lockupViewModel" => {
            if renderer
                .get("contentType")
                .and_then(Value::as_str)
                .filter(|content_type| *content_type == "LOCKUP_CONTENT_TYPE_VIDEO")
                .is_none()
            {
                return None;
            }
            renderer
                .get("contentId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)?
        }
        _ => renderer
            .get("videoId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)?,
    };
    Some(ChannelVideoRef {
        video_id,
        tab_name: tab_name.to_owned(),
        title: renderer_title(renderer),
        published_at: renderer_published_text(renderer),
    })
}

fn renderer_title(renderer: &Value) -> Option<String> {
    find_text_at_path(renderer, &["title"])
        .or_else(|| find_text_at_path(renderer, &["metadata", "lockupMetadataViewModel", "title"]))
        .or_else(|| find_text_at_path(renderer, &["headline"]))
        .or_else(|| find_text_at_path(renderer, &["accessibilityText"]))
}

fn renderer_published_text(renderer: &Value) -> Option<String> {
    find_text_at_path(renderer, &["publishedTimeText"])
        .or_else(|| {
            renderer
                .get("metadata")
                .and_then(|value| value.get("lockupMetadataViewModel"))
                .and_then(|value| value.get("metadata"))
                .and_then(|value| value.get("contentMetadataViewModel"))
                .and_then(text_from_value)
        })
        .or_else(|| find_text_at_path(renderer, &["videoInfo"]))
        .or_else(|| find_text_at_path(renderer, &["timestampText"]))
}

fn find_continuation_token(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(command) = map.get("continuationCommand") {
                if let Some(token) = command.get("token").and_then(Value::as_str) {
                    return Some(token.to_owned());
                }
            }
            map.values().find_map(find_continuation_token)
        }
        Value::Array(items) => items.iter().find_map(find_continuation_token),
        _ => None,
    }
}

fn extract_video_id_from_entity_id(entity_id: &str) -> Option<String> {
    entity_id
        .rsplit_once(':')
        .map(|(_, suffix)| suffix.to_owned())
        .filter(|suffix| !suffix.is_empty())
}

fn merge_raw_payloads(primary: &Value, secondary: Option<&Value>) -> Value {
    if let Some(secondary) = secondary {
        serde_json::json!({
            "primary": primary,
            "secondary": secondary,
        })
    } else {
        primary.clone()
    }
}

fn find_banner_url(value: &Value) -> Option<String> {
    find_thumbnail_url(value, "banner")
        .or_else(|| find_thumbnail_url(value, "mobileBanner"))
        .or_else(|| find_thumbnail_url(value, "tvBanner"))
        .or_else(|| {
            value.get("header")
                .and_then(|header| header.get("pageHeaderRenderer"))
                .and_then(|value| value.get("content"))
                .and_then(|value| value.get("pageHeaderViewModel"))
                .and_then(|value| value.get("banner"))
                .and_then(|value| value.get("imageBannerViewModel"))
                .and_then(|value| value.get("image"))
                .and_then(|value| value.get("sources"))
                .and_then(Value::as_array)
                .and_then(|items| items.last())
                .and_then(|item| item.get("url"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .map(|url| uncrop_youtube_image(&url))
}

fn find_thumbnail_url(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(thumbnails) = map
                .get(key)
                .and_then(|value| value.get("thumbnails").or_else(|| value.get("sources")))
                .and_then(Value::as_array)
            {
                if let Some(url) = thumbnails
                    .last()
                    .and_then(|item| item.get("url"))
                    .and_then(Value::as_str)
                {
                    return Some(url.to_owned());
                }
            }
            map.values().find_map(|child| find_thumbnail_url(child, key))
        }
        Value::Array(items) => items.iter().find_map(|child| find_thumbnail_url(child, key)),
        _ => None,
    }
}

fn uncrop_youtube_image(url: &str) -> String {
    let base = url.split('=').next().unwrap_or(url);
    format!("{base}=s0")
}

fn find_count_in_about_renderer(value: &Value, key: &str) -> Option<u64> {
    find_object_with_key(value, "channelAboutFullMetadataRenderer")
        .and_then(|renderer| renderer.get(key))
        .and_then(text_from_value)
        .and_then(|text| parse_count_from_text(&text))
}

fn find_count_in_metadata_rows(value: &Value, needles: &[&str]) -> Option<u64> {
    let rows = find_object_with_key(value, "metadataRows")
        .and_then(Value::as_array)
        .or_else(|| {
            find_object_with_key(value, "metadataRows")
                .and_then(|rows| rows.get("metadataRows"))
                .and_then(Value::as_array)
        })?;
    rows.iter().find_map(|row| {
        let text = text_from_value(row)?;
        let lower = text.to_ascii_lowercase();
        if needles.iter().any(|needle| lower.contains(needle)) {
            parse_count_from_text(&text)
        } else {
            None
        }
    })
}

fn find_page_header_metadata_count(value: &Value, needles: &[&str]) -> Option<u64> {
    value
        .get("header")
        .and_then(|header| header.get("pageHeaderRenderer"))
        .and_then(|value| value.get("content"))
        .and_then(|value| value.get("pageHeaderViewModel"))
        .and_then(|value| value.get("metadata"))
        .and_then(|value| value.get("contentMetadataViewModel"))
        .and_then(|value| value.get("metadataRows"))
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter().find_map(|row| {
                row.get("metadataParts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .find_map(|part| {
                        let text = text_from_value(part)?;
                        let lower = text.to_ascii_lowercase();
                        if needles.iter().any(|needle| lower.contains(needle)) {
                            parse_count_from_text(&text)
                        } else {
                            None
                        }
                    })
            })
        })
}

fn parse_video_tags(details: &serde_json::Map<String, Value>, html: &str) -> Vec<String> {
    if let Some(tags) = details.get("keywords").and_then(Value::as_array) {
        let parsed = tags
            .iter()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    extract_meta_values(html, "og:video:tag")
}

fn parse_video_categories(microformat: Option<&Value>, html: &str) -> Vec<String> {
    if let Some(category) = microformat
        .and_then(|value| value.get("category"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return vec![category.to_owned()];
    }
    extract_meta_values(html, "genre")
}

fn extract_meta_values(html: &str, property: &str) -> Vec<String> {
    let needle = format!("property=\"{property}\" content=\"");
    let mut values = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find(&needle) {
        let slice = &rest[start + needle.len()..];
        if let Some(end) = slice.find('"') {
            values.push(slice[..end].to_owned());
            rest = &slice[end..];
        } else {
            break;
        }
    }
    if values.is_empty() {
        let needle = format!("name=\"{property}\" content=\"");
        let mut rest = html;
        while let Some(start) = rest.find(&needle) {
            let slice = &rest[start + needle.len()..];
            if let Some(end) = slice.find('"') {
                values.push(slice[..end].to_owned());
                rest = &slice[end..];
            } else {
                break;
            }
        }
    }
    values
}

fn parse_comment_count_from_initial_data(value: &Value) -> Option<u64> {
    let comments_section_ids = [
        "comment-item-section",
        "engagement-panel-comments-section",
    ];
    find_first_by_key(value, "commentsEntryPointHeaderRenderer")
        .and_then(|renderer| renderer.get("commentCount"))
        .and_then(text_from_value)
        .and_then(|text| parse_count_from_text(&text))
        .or_else(|| {
            find_array_entry_by_panel_id(value, &comments_section_ids)
                .and_then(|panel| panel.get("engagementPanelSectionListRenderer"))
                .and_then(|renderer| renderer.get("header"))
                .and_then(|header| header.get("engagementPanelTitleHeaderRenderer"))
                .and_then(|renderer| renderer.get("contextualInfo"))
                .and_then(text_from_value)
                .and_then(|text| parse_count_from_text(&text))
        })
}

fn parse_like_count_from_initial_data(value: &Value) -> Option<u64> {
    parse_like_count_accessibility(value)
        .or_else(|| find_string_value(value, "likeCount").and_then(|text| parse_count_from_text(&text)))
}

fn parse_like_count_accessibility(value: &Value) -> Option<u64> {
    let mut like_count = None;
    if let Some(renderer) = find_first_by_key(value, "videoPrimaryInfoRenderer") {
        for label in find_accessibility_labels(renderer) {
            let lower = label.to_ascii_lowercase();
            if like_count.is_none() && lower.contains("like") && !lower.contains("dislike") {
                like_count = parse_count_from_text(&label);
            }
        }
    }
    like_count
}

fn find_accessibility_labels(value: &Value) -> Vec<String> {
    let mut labels = Vec::new();
    collect_accessibility_labels(value, &mut labels);
    labels
}

fn collect_accessibility_labels(value: &Value, labels: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(label) = map
                .get("label")
                .and_then(Value::as_str)
                .filter(|label| label.to_ascii_lowercase().contains("like"))
            {
                labels.push(label.to_owned());
            }
            if let Some(text) = map
                .get("accessibilityText")
                .and_then(Value::as_str)
                .filter(|label| label.to_ascii_lowercase().contains("like"))
            {
                labels.push(text.to_owned());
            }
            for child in map.values() {
                collect_accessibility_labels(child, labels);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_accessibility_labels(child, labels);
            }
        }
        _ => {}
    }
}

fn parse_count_from_text(input: &str) -> Option<u64> {
    let normalized = input.replace('\u{a0}', " ");
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    let index = tokens
        .iter()
        .position(|part| part.chars().any(|ch| ch.is_ascii_digit()))?;
    let token = tokens[index].trim_matches(|ch: char| ch == ',' || ch == '.');
    let compact = if let Some(next) = tokens.get(index + 1) {
        let unit = next.trim_matches(|ch: char| ch == ',' || ch == '.').to_ascii_lowercase();
        if matches!(unit.as_str(), "k" | "m" | "b") {
            Some(format!("{token}{unit}"))
        } else if unit == "mil" {
            Some(format!("{token}k"))
        } else {
            None
        }
    } else {
        None
    };
    compact
        .as_deref()
        .and_then(parse_compact_count)
        .or_else(|| parse_compact_count(token))
        .or_else(|| {
        let digits = token
            .chars()
            .filter(|ch| ch.is_ascii_digit())
            .collect::<String>();
        digits.parse().ok()
    })
}

fn text_from_value(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    if let Some(simple) = value.get("simpleText").and_then(Value::as_str) {
        return Some(simple.to_owned());
    }
    if let Some(runs) = value.get("runs").and_then(Value::as_array) {
        let joined = runs
            .iter()
            .filter_map(|run| run.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        if !joined.is_empty() {
            return Some(joined);
        }
    }
    match value {
        Value::Object(map) => map.values().find_map(text_from_value),
        Value::Array(items) => items.iter().find_map(text_from_value),
        _ => None,
    }
}

fn find_text_at_path(value: &Value, keys: &[&str]) -> Option<String> {
    let mut current = value;
    for key in keys {
        current = current.get(*key)?;
    }
    text_from_value(current)
}

fn find_first_by_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(key) {
                return Some(found);
            }
            map.values().find_map(|child| find_first_by_key(child, key))
        }
        Value::Array(items) => items.iter().find_map(|child| find_first_by_key(child, key)),
        _ => None,
    }
}

fn find_array_entry_by_panel_id<'a>(value: &'a Value, ids: &[&str]) -> Option<&'a Value> {
    match value {
        Value::Array(items) => items.iter().find(|item| {
            item.get("engagementPanelSectionListRenderer")
                .and_then(|renderer| renderer.get("panelIdentifier"))
                .and_then(Value::as_str)
                .map(|panel_id| ids.iter().any(|candidate| candidate == &panel_id))
                .unwrap_or(false)
        }),
        Value::Object(map) => map.values().find_map(|child| find_array_entry_by_panel_id(child, ids)),
        _ => None,
    }
}

fn find_string_value(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(key).and_then(Value::as_str) {
                return Some(found.to_owned());
            }
            map.values().find_map(|child| find_string_value(child, key))
        }
        Value::Array(items) => items.iter().find_map(|child| find_string_value(child, key)),
        _ => None,
    }
}

fn mime_to_extension(mime: &str) -> &str {
    if mime.contains("mp4") {
        "mp4"
    } else if mime.contains("webm") {
        "webm"
    } else if mime.contains("mp3") {
        "mp3"
    } else if mime.contains("mpeg") {
        "mpeg"
    } else {
        "bin"
    }
}

fn extract_codec(mime: &str) -> Option<(&str, Option<&str>)> {
    let codecs = mime.split("codecs=\"").nth(1)?.split('"').next()?;
    let mut parts = codecs.split(',').map(str::trim);
    let first = parts.next()?;
    let second = parts.next();
    Some((first, second))
}

fn parse_availability(raw: &Value, details: &serde_json::Map<String, Value>) -> Availability {
    if details
        .get("isPrivate")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Availability::Private;
    }

    match raw
        .get("playabilityStatus")
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
    {
        Some("OK") => Availability::Public,
        Some("LOGIN_REQUIRED") => Availability::MembersOnly,
        Some("UNPLAYABLE") => Availability::Private,
        Some("ERROR") => Availability::Deleted,
        _ => Availability::Unknown,
    }
}

fn parse_subtitles(raw: &Value) -> Vec<SubtitleTrack> {
    raw.get("captions")
        .and_then(|value| value.get("playerCaptionsTracklistRenderer"))
        .and_then(|value| value.get("captionTracks"))
        .and_then(Value::as_array)
        .map(|tracks| {
            tracks
                .iter()
                .filter_map(|track| {
                    Some(SubtitleTrack {
                        language: track.get("languageCode")?.as_str()?.to_owned(),
                        url: track
                            .get("baseUrl")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                        is_auto_generated: track
                            .get("kind")
                            .and_then(Value::as_str)
                            .map(|kind| kind == "asr")
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn fetch_comment_page(
    api_key: &str,
    client_version: &str,
    visitor_data: &str,
    continuation: &str,
) -> Result<Value> {
    let client = http_client()?;
    let response = client
        .post(format!(
            "https://www.youtube.com/youtubei/v1/next?key={api_key}"
        ))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&serde_json::json!({
            "context": {
                "client": {
                    "clientName": "WEB",
                    "clientVersion": client_version,
                    "visitorData": visitor_data,
                }
            },
            "continuation": continuation,
        }))
        .send()
        .await
        .map_err(|err| VesselError::Extractor(format!("youtube comment request failed: {err}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(VesselError::Extractor(format!(
            "youtube comment request returned http status {status}"
        )));
    }
    response.json::<Value>().await.map_err(|err| {
        VesselError::Extractor(format!("youtube comment response decode failed: {err}"))
    })
}

fn find_first_comment_continuation(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(command) = map.get("continuationCommand") {
                let request = command.get("request").and_then(Value::as_str);
                let token = command.get("token").and_then(Value::as_str);
                if request == Some("CONTINUATION_REQUEST_TYPE_WATCH_NEXT") {
                    return token.map(ToOwned::to_owned);
                }
            }
            map.values().find_map(find_first_comment_continuation)
        }
        Value::Array(items) => items.iter().find_map(find_first_comment_continuation),
        _ => None,
    }
}

fn find_next_comment_continuation(page: &Value) -> Option<String> {
    page.get("onResponseReceivedEndpoints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|endpoint| endpoint.get("reloadContinuationItemsCommand"))
        .filter_map(|command| command.get("continuationItems"))
        .filter_map(Value::as_array)
        .flat_map(|items| items.iter())
        .find_map(|item| {
            item.get("continuationItemRenderer")
                .and_then(|value| value.get("continuationEndpoint"))
                .and_then(|value| value.get("continuationCommand"))
                .and_then(|value| value.get("token"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

fn parse_comment_page(
    video_id: &str,
    page: &Value,
    fetched_at: OffsetDateTime,
    max_comments: usize,
) -> Vec<CommentMetadata> {
    page.get("frameworkUpdates")
        .and_then(|value| value.get("entityBatchUpdate"))
        .and_then(|value| value.get("mutations"))
        .and_then(Value::as_array)
        .map(|mutations| {
            mutations
                .iter()
                .filter_map(|mutation| parse_comment_mutation(video_id, mutation, fetched_at))
                .take(max_comments)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_comment_mutation(
    video_id: &str,
    mutation: &Value,
    fetched_at: OffsetDateTime,
) -> Option<CommentMetadata> {
    let payload = mutation.get("payload")?.get("commentEntityPayload")?;
    let properties = payload.get("properties")?;
    let author = payload.get("author")?;
    let toolbar = payload.get("toolbar");

    Some(CommentMetadata {
        platform: Platform::YouTube,
        comment_id: properties.get("commentId")?.as_str()?.to_owned(),
        video_id: video_id.to_owned(),
        author_channel_id: author
            .get("channelId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        author_name: author
            .get("displayName")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        text: properties
            .get("content")
            .and_then(|value| value.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        like_count: toolbar
            .and_then(|value| value.get("likeCountLiked"))
            .and_then(Value::as_str)
            .and_then(parse_compact_count),
        reply_count: toolbar
            .and_then(|value| value.get("replyCount"))
            .and_then(Value::as_str)
            .and_then(parse_compact_count),
        published_at: properties
            .get("publishedTime")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        fetched_at,
        raw: payload.clone(),
    })
}

fn parse_compact_count(input: &str) -> Option<u64> {
    let normalized = input.trim().replace(',', "");
    if normalized.is_empty() {
        return None;
    }

    let last = normalized.chars().last()?;
    let multiplier = match last {
        'K' | 'k' => 1_000f64,
        'M' | 'm' => 1_000_000f64,
        'B' | 'b' => 1_000_000_000f64,
        _ if last.is_ascii_digit() => 1f64,
        _ => return None,
    };
    let number = if multiplier == 1f64 {
        normalized.parse::<f64>().ok()?
    } else {
        normalized[..normalized.len() - 1].parse::<f64>().ok()?
    };
    Some((number * multiplier).round() as u64)
}

fn find_object_with_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(key) {
                return Some(found);
            }
            map.values()
                .find_map(|child| find_object_with_key(child, key))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_object_with_key(child, key)),
        _ => None,
    }
}

fn string_field(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{
        collect_channel_video_refs, extract_embedded_json, find_next_comment_continuation,
        merge_streaming_data, parse_channel_metadata, parse_comment_page, parse_compact_count,
        parse_video_id_from_url, parse_video_metadata,
    };
    use time::OffsetDateTime;

    #[test]
    fn parses_watch_url_video_id() {
        let id = parse_video_id_from_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&ab_channel=RickAstley",
        )
        .expect("video id");
        assert_eq!(id, "dQw4w9WgXcQ");
    }

    #[test]
    fn parses_short_url_video_id() {
        let id = parse_video_id_from_url("https://youtu.be/dQw4w9WgXcQ").expect("video id");
        assert_eq!(id, "dQw4w9WgXcQ");
    }

    #[test]
    fn extracts_player_response_json_from_html() {
        let html = r#"
        <html><script>var ytInitialPlayerResponse = {"videoDetails":{"videoId":"abc123","title":"Title","shortDescription":"Desc","channelId":"chan","lengthSeconds":"42","viewCount":"123","thumbnail":{"thumbnails":[{"url":"https://i.ytimg.com/test.jpg","width":120,"height":90}]}},"playabilityStatus":{"status":"OK"},"microformat":{"playerMicroformatRenderer":{"uploadDate":"2024-01-01"}},"streamingData":{"formats":[{"itag":18,"mimeType":"video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"","qualityLabel":"360p"}]}};</script></html>
        "#;
        let json = extract_embedded_json(html, "var ytInitialPlayerResponse = ").expect("json");
        let video = parse_video_metadata(
            &json,
            None,
            html,
            "https://www.youtube.com/watch?v=abc123",
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("parsed metadata");
        assert_eq!(video.video_id, "abc123");
        assert_eq!(video.title.as_deref(), Some("Title"));
        assert_eq!(video.channel_id.as_deref(), Some("chan"));
        assert_eq!(video.duration_seconds, Some(42));
        assert_eq!(video.view_count, Some(123));
        assert_eq!(video.upload_date.as_deref(), Some("2024-01-01"));
        assert_eq!(video.formats.len(), 1);
        assert_eq!(video.thumbnails.len(), 1);
    }

    #[test]
    fn parses_channel_metadata_from_initial_data() {
        let raw = serde_json::json!({
            "metadata": {
                "channelMetadataRenderer": {
                    "title": "Example Channel",
                    "description": "Channel description",
                    "externalId": "UC1234567890",
                    "channelUrl": "https://www.youtube.com/channel/UC1234567890",
                    "vanityChannelUrl": "https://www.youtube.com/@example",
                    "avatar": {
                        "thumbnails": [
                            { "url": "https://example.com/a.jpg" }
                        ]
                    }
                }
            }
        });
        let channel = parse_channel_metadata(
            &raw,
            None,
            "https://www.youtube.com/@example",
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("channel metadata");
        assert_eq!(channel.channel_id, "UC1234567890");
        assert_eq!(channel.handle.as_deref(), Some("@example"));
        assert_eq!(channel.title.as_deref(), Some("Example Channel"));
    }

    #[test]
    fn collects_channel_videos_from_renderer_tree() {
        let page = serde_json::json!({
            "contents": [{
                "videoRenderer": {
                    "videoId": "abc123",
                    "title": { "runs": [{ "text": "First" }] },
                    "publishedTimeText": { "simpleText": "1 day ago" }
                }
            }, {
                "gridVideoRenderer": {
                    "videoId": "def456",
                    "title": { "simpleText": "Second" }
                }
            }]
        });
        let mut videos = Vec::new();
        collect_channel_video_refs(&page, "videos", &mut videos);
        assert_eq!(videos.len(), 2);
        assert_eq!(videos[0].video_id, "abc123");
        assert_eq!(videos[0].tab_name, "videos");
        assert_eq!(videos[1].title.as_deref(), Some("Second"));
    }

    #[test]
    fn parses_compact_counts() {
        assert_eq!(parse_compact_count("246K"), Some(246_000));
        assert_eq!(parse_compact_count("1.7M"), Some(1_700_000));
        assert_eq!(parse_compact_count("42"), Some(42));
    }

    #[test]
    fn parses_comment_entities_from_framework_updates() {
        let page = serde_json::json!({
            "frameworkUpdates": {
                "entityBatchUpdate": {
                    "mutations": [
                        {
                            "payload": {
                                "commentEntityPayload": {
                                    "properties": {
                                        "commentId": "comment-1",
                                        "content": { "content": "Hello world" },
                                        "publishedTime": "1 day ago"
                                    },
                                    "author": {
                                        "channelId": "author-1",
                                        "displayName": "@author"
                                    },
                                    "toolbar": {
                                        "likeCountLiked": "1.2K",
                                        "replyCount": "3"
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let comments = parse_comment_page("video-1", &page, OffsetDateTime::UNIX_EPOCH, 20);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].comment_id, "comment-1");
        assert_eq!(comments[0].video_id, "video-1");
        assert_eq!(comments[0].author_channel_id.as_deref(), Some("author-1"));
        assert_eq!(comments[0].author_name.as_deref(), Some("@author"));
        assert_eq!(comments[0].text, "Hello world");
        assert_eq!(comments[0].like_count, Some(1_200));
        assert_eq!(comments[0].reply_count, Some(3));
        assert_eq!(comments[0].published_at.as_deref(), Some("1 day ago"));
    }

    #[test]
    fn finds_next_comment_continuation_token() {
        let page = serde_json::json!({
            "onResponseReceivedEndpoints": [
                {
                    "reloadContinuationItemsCommand": {
                        "continuationItems": [
                            { "commentThreadRenderer": {} },
                            {
                                "continuationItemRenderer": {
                                    "continuationEndpoint": {
                                        "continuationCommand": {
                                            "token": "next-token"
                                        }
                                    }
                                }
                            }
                        ]
                    }
                }
            ]
        });

        assert_eq!(
            find_next_comment_continuation(&page).as_deref(),
            Some("next-token")
        );
    }

    #[test]
    fn merges_android_streaming_data_into_player_response() {
        let mut player = serde_json::json!({
            "videoDetails": { "videoId": "abc123" },
            "streamingData": {
                "formats": [
                    { "itag": 18, "signatureCipher": "s=encrypted&sp=sig&url=https%3A%2F%2Fexample.invalid%2Fv" }
                ]
            }
        });
        let android = serde_json::json!({
            "streamingData": {
                "formats": [
                    { "itag": 18, "url": "https://example.invalid/direct.mp4" }
                ],
                "adaptiveFormats": [
                    { "itag": 140, "url": "https://example.invalid/audio.m4a" }
                ]
            }
        });

        merge_streaming_data(&mut player, &android);
        let streaming = player.get("streamingData").expect("streaming data");
        assert_eq!(
            streaming["formats"][0]["url"].as_str(),
            Some("https://example.invalid/direct.mp4")
        );
        assert_eq!(streaming["adaptiveFormats"][0]["itag"].as_i64(), Some(140));
    }
}
