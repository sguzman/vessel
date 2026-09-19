use reqwest::Client;
use serde_json::Value;
use url::Url;

use vessel_core::models::{SubtitleTrack, VideoMetadata};
use vessel_core::{
    Result, TranscriptCandidate, TranscriptDerivation, TranscriptSegment, VesselError,
    YoutubeTranscriptPolicyV1,
};

pub async fn acquire_best_caption_candidate(
    video: &VideoMetadata,
    policy: &YoutubeTranscriptPolicyV1,
) -> Result<Option<TranscriptCandidate>> {
    if !policy.enabled {
        return Ok(None);
    }

    let providers = [
        (
            TranscriptDerivation::CreatorSubtitles,
            policy.allow_creator_subtitles,
            false,
        ),
        (
            TranscriptDerivation::PlatformAutoCaption,
            policy.allow_auto_captions,
            true,
        ),
    ];

    for (derivation, allowed, auto_generated) in providers {
        if !allowed {
            continue;
        }

        let tracks = video
            .subtitles
            .iter()
            .filter(|track| track.is_auto_generated == auto_generated)
            .collect::<Vec<_>>();
        let Some(track) = choose_track(&tracks, &policy.preferred_languages) else {
            continue;
        };

        let Some(base_url) = track.url.as_deref() else {
            continue;
        };

        return fetch_caption_candidate(base_url, &track.language, derivation, &http_client()?)
            .await
            .map(Some);
    }

    Ok(None)
}

fn choose_track<'a>(
    tracks: &[&'a SubtitleTrack],
    preferred_languages: &[String],
) -> Option<&'a SubtitleTrack> {
    for preferred in preferred_languages {
        if let Some(track) = tracks
            .iter()
            .copied()
            .find(|track| language_matches(&track.language, preferred))
        {
            return Some(track);
        }
    }

    tracks.first().copied()
}

fn language_matches(actual: &str, preferred: &str) -> bool {
    actual.eq_ignore_ascii_case(preferred)
        || actual
            .split_once('-')
            .map(|(base, _)| base.eq_ignore_ascii_case(preferred))
            .unwrap_or(false)
        || preferred
            .split_once('-')
            .map(|(base, _)| actual.eq_ignore_ascii_case(base))
            .unwrap_or(false)
}

async fn fetch_caption_candidate(
    base_url: &str,
    language: &str,
    derivation: TranscriptDerivation,
    client: &Client,
) -> Result<TranscriptCandidate> {
    let url = json3_url(base_url)?;
    let response = client.get(url).send().await.map_err(|error| {
        VesselError::Extractor(format!("youtube caption request failed: {error}"))
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(VesselError::Extractor(format!(
            "youtube caption request returned http status {status}"
        )));
    }

    let raw = response.text().await.map_err(|error| {
        VesselError::Extractor(format!("youtube caption response decode failed: {error}"))
    })?;
    let mut candidate = parse_youtube_json3(&raw, derivation, language)?;
    candidate.validate()?;
    Ok(candidate)
}

fn json3_url(base_url: &str) -> Result<Url> {
    let mut url = Url::parse(base_url)
        .map_err(|error| VesselError::Extractor(format!("invalid youtube caption URL: {error}")))?;
    let retained = url
        .query_pairs()
        .filter(|(key, _)| key != "fmt")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in retained {
            query.append_pair(&key, &value);
        }
        query.append_pair("fmt", "json3");
    }
    Ok(url)
}

pub fn parse_youtube_json3(
    raw: &str,
    derivation: TranscriptDerivation,
    language: &str,
) -> Result<TranscriptCandidate> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        VesselError::Extractor(format!("invalid youtube json3 captions: {error}"))
    })?;
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| VesselError::Extractor("youtube json3 captions missing events".into()))?;

    let mut segments = Vec::new();
    for event in events {
        let Some(start_ms) = event.get("tStartMs").and_then(number_as_u64) else {
            continue;
        };
        let Some(parts) = event.get("segs").and_then(Value::as_array) else {
            continue;
        };

        let mut text = String::new();
        for part in parts {
            if let Some(fragment) = part.get("utf8").and_then(Value::as_str) {
                text.push_str(fragment);
            }
        }

        let text = normalize_caption_text(&text);
        if text.is_empty() {
            continue;
        }

        segments.push(TranscriptSegment {
            start_seconds: Some(start_ms / 1_000),
            text,
        });
    }

    let candidate = TranscriptCandidate {
        derivation,
        language: Some(language.to_owned()),
        timestamps: true,
        engine: None,
        model: None,
        segments,
    };
    candidate.validate()?;
    Ok(candidate)
}

fn number_as_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn normalize_caption_text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn http_client() -> Result<Client> {
    Client::builder()
        .user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36",
        )
        .build()
        .map_err(|error| VesselError::Extractor(format!("http client build failed: {error}")))
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use vessel_core::models::{Availability, Platform};

    use super::*;

    fn video_with_tracks(tracks: Vec<SubtitleTrack>) -> VideoMetadata {
        VideoMetadata {
            platform: Platform::YouTube,
            video_id: "abc123".into(),
            channel_id: Some("UCexample".into()),
            url: "https://www.youtube.com/watch?v=abc123".into(),
            title: Some("Example".into()),
            description: None,
            duration_seconds: Some(60),
            upload_date: Some("20260918".into()),
            release_timestamp: None,
            tags: Vec::new(),
            categories: Vec::new(),
            primary_category: None,
            view_count: None,
            like_count: None,
            comment_count: None,
            availability: Availability::Public,
            formats: Vec::new(),
            subtitles: tracks,
            thumbnails: Vec::new(),
            fetched_at: OffsetDateTime::UNIX_EPOCH,
            raw: Value::Null,
        }
    }

    #[test]
    fn parses_json3_into_timestamped_segments() {
        let raw = r#"{
          "events": [
            {"tStartMs": 3250, "segs": [{"utf8": "Hello "}, {"utf8": "world"}]},
            {"tStartMs": "8100", "segs": [{"utf8": "second\nsegment"}]},
            {"tStartMs": 9000}
          ]
        }"#;

        let candidate = parse_youtube_json3(raw, TranscriptDerivation::PlatformAutoCaption, "en")
            .expect("parse captions");

        assert_eq!(candidate.language.as_deref(), Some("en"));
        assert_eq!(candidate.segments.len(), 2);
        assert_eq!(candidate.segments[0].start_seconds, Some(3));
        assert_eq!(candidate.segments[0].text, "Hello world");
        assert_eq!(candidate.segments[1].start_seconds, Some(8));
        assert_eq!(candidate.segments[1].text, "second segment");
    }

    #[test]
    fn chooses_preferred_language_with_regional_matching() {
        let en_gb = SubtitleTrack {
            language: "en-GB".into(),
            url: Some("https://example.test/en".into()),
            is_auto_generated: false,
        };
        let es = SubtitleTrack {
            language: "es".into(),
            url: Some("https://example.test/es".into()),
            is_auto_generated: false,
        };
        let tracks = vec![&es, &en_gb];

        let selected = choose_track(&tracks, &["en".into()]).expect("track");
        assert_eq!(selected.language, "en-GB");
    }

    #[test]
    fn creator_and_auto_tracks_remain_distinguishable() {
        let video = video_with_tracks(vec![
            SubtitleTrack {
                language: "en".into(),
                url: Some("https://example.test/auto".into()),
                is_auto_generated: true,
            },
            SubtitleTrack {
                language: "es".into(),
                url: Some("https://example.test/manual".into()),
                is_auto_generated: false,
            },
        ]);

        let manual = video
            .subtitles
            .iter()
            .filter(|track| !track.is_auto_generated)
            .collect::<Vec<_>>();
        let automatic = video
            .subtitles
            .iter()
            .filter(|track| track.is_auto_generated)
            .collect::<Vec<_>>();

        assert_eq!(choose_track(&manual, &[]).unwrap().language, "es");
        assert_eq!(choose_track(&automatic, &[]).unwrap().language, "en");
    }

    #[test]
    fn json3_url_replaces_existing_format_parameter() {
        let url = json3_url("https://example.test/timedtext?lang=en&fmt=vtt").expect("url");
        let pairs = url.query_pairs().collect::<Vec<_>>();
        assert!(
            pairs
                .iter()
                .any(|(key, value)| key == "lang" && value == "en")
        );
        assert_eq!(
            pairs
                .iter()
                .filter(|(key, _)| key == "fmt")
                .map(|(_, value)| value.as_ref())
                .collect::<Vec<_>>(),
            vec!["json3"]
        );
    }
}
