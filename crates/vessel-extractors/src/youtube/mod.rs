use async_trait::async_trait;
use quick_xml::Reader;
use quick_xml::events::Event;
use reqwest::Client;
use serde_json::Value;
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
    pub title: Option<String>,
    pub published_at: Option<String>,
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
    parse_video_metadata(&player_response, &url, OffsetDateTime::now_utc())
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
        .or_else(|| extract_embedded_json(&html, "ytInitialData = "));
    let metadata = extract_embedded_json(&html, "var ytInitialData = ")
        .or_else(|| extract_embedded_json(&html, "ytInitialData = "));
    let raw = metadata.or(initial_data).ok_or_else(|| {
        VesselError::Extractor("failed to locate ytInitialData in channel page".to_owned())
    })?;
    parse_channel_metadata(&raw, &canonical_url, OffsetDateTime::now_utc())
}

pub async fn list_channel_videos(input: &InputRef) -> Result<Vec<ChannelVideoRef>> {
    let channel = extract_channel(input).await?;
    let feed_url = format!(
        "https://www.youtube.com/feeds/videos.xml?channel_id={}",
        channel.channel_id
    );
    let xml = fetch_text(&feed_url).await?;
    parse_channel_feed(&xml)
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

    Ok(ChannelMetadata {
        platform: Platform::YouTube,
        channel_id: channel_id.to_owned(),
        handle,
        url: metadata
            .get("channelUrl")
            .and_then(Value::as_str)
            .unwrap_or(url)
            .to_owned(),
        title: metadata
            .get("title")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        description: metadata
            .get("description")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        subscriber_count: None,
        video_count: None,
        view_count: None,
        avatar_url,
        banner_url: None,
        fetched_at,
        raw: raw.clone(),
    })
}

fn parse_video_metadata(
    raw: &Value,
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
        view_count: string_field(details, "viewCount").and_then(|value| value.parse().ok()),
        like_count: None,
        comment_count: None,
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

fn parse_channel_feed(xml: &str) -> Result<Vec<ChannelVideoRef>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut videos = Vec::new();
    let mut current_video_id: Option<String> = None;
    let mut current_title: Option<String> = None;
    let mut current_published: Option<String> = None;
    let mut in_entry = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref event)) => match event.name().as_ref() {
                b"entry" => {
                    in_entry = true;
                    current_video_id = None;
                    current_title = None;
                    current_published = None;
                }
                b"yt:videoId" if in_entry => {
                    current_video_id = Some(
                        reader
                            .read_text(event.name())
                            .map_err(|err| {
                                VesselError::Extractor(format!(
                                    "failed to parse feed video id: {err}"
                                ))
                            })?
                            .into_owned(),
                    );
                }
                b"title" if in_entry => {
                    current_title = Some(
                        reader
                            .read_text(event.name())
                            .map_err(|err| {
                                VesselError::Extractor(format!("failed to parse feed title: {err}"))
                            })?
                            .into_owned(),
                    );
                }
                b"published" if in_entry => {
                    current_published = Some(
                        reader
                            .read_text(event.name())
                            .map_err(|err| {
                                VesselError::Extractor(format!(
                                    "failed to parse feed publish date: {err}"
                                ))
                            })?
                            .into_owned(),
                    );
                }
                _ => {}
            },
            Ok(Event::End(ref event)) if event.name().as_ref() == b"entry" => {
                if let Some(video_id) = current_video_id.take() {
                    videos.push(ChannelVideoRef {
                        video_id,
                        title: current_title.take(),
                        published_at: current_published.take(),
                    });
                }
                in_entry = false;
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(VesselError::Extractor(format!(
                    "failed to parse channel feed xml: {err}"
                )));
            }
            _ => {}
        }
    }

    Ok(videos)
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
        extract_embedded_json, find_next_comment_continuation, merge_streaming_data,
        parse_channel_feed, parse_channel_metadata, parse_comment_page, parse_compact_count,
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
            "https://www.youtube.com/@example",
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("channel metadata");
        assert_eq!(channel.channel_id, "UC1234567890");
        assert_eq!(channel.handle.as_deref(), Some("@example"));
        assert_eq!(channel.title.as_deref(), Some("Example Channel"));
    }

    #[test]
    fn parses_channel_feed_entries() {
        let xml = r#"
        <feed xmlns:yt="http://www.youtube.com/xml/schemas/2015">
          <entry>
            <yt:videoId>abc123</yt:videoId>
            <title>First</title>
            <published>2024-01-01T00:00:00+00:00</published>
          </entry>
          <entry>
            <yt:videoId>def456</yt:videoId>
            <title>Second</title>
            <published>2024-01-02T00:00:00+00:00</published>
          </entry>
        </feed>
        "#;
        let videos = parse_channel_feed(xml).expect("feed parse");
        assert_eq!(videos.len(), 2);
        assert_eq!(videos[0].video_id, "abc123");
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
