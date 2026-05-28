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
    Availability, ChannelMetadata, InputKind, InputRef, MediaFormat, Platform, Thumbnail,
    VideoMetadata,
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
    fn name(&self) -> &'static str {
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
    let player_response = extract_embedded_json(&html, "var ytInitialPlayerResponse = ")
        .or_else(|| extract_embedded_json(&html, "ytInitialPlayerResponse = "))
        .ok_or_else(|| {
            VesselError::Extractor(
                "failed to locate ytInitialPlayerResponse in watch page".to_owned(),
            )
        })?;
    parse_video_metadata(&player_response, &url, OffsetDateTime::now_utc())
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
        subtitles: Vec::new(),
        thumbnails,
        fetched_at,
        raw: raw.clone(),
    })
}

fn parse_formats(streaming_data: &Value) -> Vec<MediaFormat> {
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

    iter.filter_map(|item| {
        let format_id = item.get("itag").and_then(Value::as_u64)?.to_string();
        Some(MediaFormat {
            format_id,
            ext: item
                .get("mimeType")
                .and_then(Value::as_str)
                .map(mime_to_extension)
                .unwrap_or("bin")
                .to_owned(),
            note: item
                .get("qualityLabel")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            video_codec: item
                .get("mimeType")
                .and_then(Value::as_str)
                .and_then(extract_codec)
                .map(|(video, _)| video.to_owned()),
            audio_codec: item
                .get("mimeType")
                .and_then(Value::as_str)
                .and_then(extract_codec)
                .and_then(|(_, audio)| audio.map(ToOwned::to_owned)),
        })
    })
    .collect()
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
        extract_embedded_json, parse_channel_feed, parse_channel_metadata, parse_video_id_from_url,
        parse_video_metadata,
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
}
