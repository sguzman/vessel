use std::path::{Path, PathBuf};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;

use vessel_core::models::{MediaFormat, VideoMetadata};
use vessel_core::{Result, VesselError};
use vessel_formats::FormatSelector;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadPlan {
    pub video_id: String,
    pub format_id: String,
    pub url: String,
    pub output_path: PathBuf,
    pub temp_path: PathBuf,
    pub selector: FormatSelector,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadResult {
    pub format_id: String,
    pub output_path: PathBuf,
    pub temp_path: PathBuf,
    pub bytes_written: u64,
    pub resumed: bool,
}

pub trait DownloadPlanner: Send + Sync {
    fn plan(
        &self,
        item: &VideoMetadata,
        selector: FormatSelector,
        output_template: &str,
    ) -> Result<DownloadPlan>;
}

#[derive(Debug, Default)]
pub struct BasicDownloadPlanner;

impl DownloadPlanner for BasicDownloadPlanner {
    fn plan(
        &self,
        item: &VideoMetadata,
        selector: FormatSelector,
        output_template: &str,
    ) -> Result<DownloadPlan> {
        let format = select_format(item, &selector)?;
        let output_path = render_output_path(item, format, output_template);
        let temp_path = output_path.with_extension(format!("{}.part", format.ext));
        Ok(DownloadPlan {
            video_id: item.video_id.clone(),
            format_id: format.format_id.clone(),
            url: format.download_url.clone().ok_or_else(|| {
                VesselError::Unsupported("selected format has no download url".to_owned())
            })?,
            output_path,
            temp_path,
            selector,
        })
    }
}

pub async fn execute_download(plan: &DownloadPlan) -> Result<DownloadResult> {
    if let Some(parent) = plan.output_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let existing_bytes = fs::metadata(&plan.temp_path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    let client = Client::builder()
        .user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36",
        )
        .build()
        .map_err(|err| VesselError::Extractor(format!("http client build failed: {err}")))?;
    let mut request = client.get(&plan.url);
    if existing_bytes > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing_bytes}-"));
    }
    let response = request
        .send()
        .await
        .map_err(|err| VesselError::Extractor(format!("download request failed: {err}")))?;
    let status = response.status();
    if !(status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT) {
        return Err(VesselError::Extractor(format!(
            "download returned http status {status}"
        )));
    }

    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT && existing_bytes > 0;
    let append = resumed;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!append)
        .append(append)
        .open(&plan.temp_path)
        .await?;

    let mut bytes_written = if resumed { existing_bytes } else { 0 };
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| VesselError::Extractor(format!("download stream failed: {err}")))?
    {
        file.write_all(&chunk).await?;
        bytes_written += chunk.len() as u64;
    }
    file.flush().await?;
    drop(file);

    fs::rename(&plan.temp_path, &plan.output_path).await?;

    Ok(DownloadResult {
        format_id: plan.format_id.clone(),
        output_path: plan.output_path.clone(),
        temp_path: plan.temp_path.clone(),
        bytes_written,
        resumed,
    })
}

fn select_format<'a>(
    item: &'a VideoMetadata,
    selector: &FormatSelector,
) -> Result<&'a MediaFormat> {
    match selector {
        FormatSelector::Best => best_downloadable_format(&item.formats)
            .ok_or_else(|| VesselError::Unsupported("no downloadable format found".to_owned())),
        FormatSelector::Worst => worst_downloadable_format(&item.formats)
            .ok_or_else(|| VesselError::Unsupported("no downloadable format found".to_owned())),
        FormatSelector::BestAudio => best_audio_format(&item.formats).ok_or_else(|| {
            VesselError::Unsupported("no downloadable audio format found".to_owned())
        }),
        FormatSelector::BestVideo => best_video_format(&item.formats).ok_or_else(|| {
            VesselError::Unsupported("no downloadable video format found".to_owned())
        }),
        FormatSelector::ExactFormatId(format_id) => item
            .formats
            .iter()
            .find(|format| format.format_id == *format_id && format.download_url.is_some())
            .ok_or_else(|| {
                VesselError::Unsupported(format!(
                    "requested format `{format_id}` is not downloadable"
                ))
            }),
        FormatSelector::Filtered { base, predicate } => {
            let base_format = select_format(item, base)?;
            if format_matches_predicate(base_format, predicate)? {
                return Ok(base_format);
            }

            let candidates = candidate_formats(item, base);
            best_from_candidates(
                candidates
                    .into_iter()
                    .filter(|format| format_matches_predicate(format, predicate).unwrap_or(false)),
                selector_kind(base),
            )
            .ok_or_else(|| {
                VesselError::Unsupported(format!(
                    "no downloadable format matched predicate `{predicate}`"
                ))
            })
        }
        FormatSelector::Fallback(selectors) => {
            let mut last_error = None;
            for branch in selectors {
                match select_format(item, branch) {
                    Ok(format) => return Ok(format),
                    Err(err) => last_error = Some(err),
                }
            }
            Err(last_error.unwrap_or_else(|| {
                VesselError::Unsupported("fallback selector had no branches".to_owned())
            }))
        }
        FormatSelector::Merge(_, _) => Err(VesselError::Unsupported(
            "merge selectors require postprocessing and are not executable yet".to_owned(),
        )),
    }
}

fn best_downloadable_format(formats: &[MediaFormat]) -> Option<&MediaFormat> {
    best_from_candidates(formats.iter().filter(|format| format.download_url.is_some()), SelectorKind::Best)
}

fn worst_downloadable_format(formats: &[MediaFormat]) -> Option<&MediaFormat> {
    formats
        .iter()
        .filter(|format| format.download_url.is_some())
        .min_by_key(|format| {
            (
                u8::from(format.has_video && format.has_audio),
                format.height.unwrap_or(0),
                format.bitrate.unwrap_or(0),
            )
        })
}

fn best_audio_format(formats: &[MediaFormat]) -> Option<&MediaFormat> {
    best_from_candidates(
        formats
            .iter()
            .filter(|format| format.download_url.is_some() && format.has_audio),
        SelectorKind::BestAudio,
    )
}

fn best_video_format(formats: &[MediaFormat]) -> Option<&MediaFormat> {
    best_from_candidates(
        formats
            .iter()
            .filter(|format| format.download_url.is_some() && format.has_video),
        SelectorKind::BestVideo,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectorKind {
    Best,
    BestAudio,
    BestVideo,
}

fn selector_kind(selector: &FormatSelector) -> SelectorKind {
    match selector {
        FormatSelector::BestAudio => SelectorKind::BestAudio,
        FormatSelector::BestVideo => SelectorKind::BestVideo,
        FormatSelector::Filtered { base, .. } => selector_kind(base),
        _ => SelectorKind::Best,
    }
}

fn candidate_formats<'a>(
    item: &'a VideoMetadata,
    selector: &FormatSelector,
) -> Vec<&'a MediaFormat> {
    item.formats
        .iter()
        .filter(|format| format.download_url.is_some())
        .filter(|format| format_matches_selector_shape(format, selector))
        .collect()
}

fn format_matches_selector_shape(format: &MediaFormat, selector: &FormatSelector) -> bool {
    match selector {
        FormatSelector::Best | FormatSelector::Worst | FormatSelector::ExactFormatId(_) => true,
        FormatSelector::BestAudio => format.has_audio,
        FormatSelector::BestVideo => format.has_video,
        FormatSelector::Filtered { base, predicate } => {
            format_matches_selector_shape(format, base)
                && format_matches_predicate(format, predicate).unwrap_or(false)
        }
        FormatSelector::Fallback(_) | FormatSelector::Merge(_, _) => true,
    }
}

fn best_from_candidates<'a>(
    candidates: impl Iterator<Item = &'a MediaFormat>,
    kind: SelectorKind,
) -> Option<&'a MediaFormat> {
    candidates.max_by_key(|format| match kind {
        SelectorKind::Best => (
            u64::from(u8::from(format.has_video && format.has_audio)),
            u64::from(u8::from(format.has_video)),
            u64::from(u8::from(format.has_audio)),
            u64::from(format.height.unwrap_or(0)),
            u64::from(format.width.unwrap_or(0)),
            format.bitrate.unwrap_or(0),
        ),
        SelectorKind::BestAudio => (
            u64::from(u8::from(format.has_audio && !format.has_video)),
            u64::from(u8::from(format.has_audio)),
            format.bitrate.unwrap_or(0),
            u64::from(format.height.unwrap_or(0)),
            u64::from(format.width.unwrap_or(0)),
            0,
        ),
        SelectorKind::BestVideo => (
            u64::from(u8::from(format.has_video && !format.has_audio)),
            u64::from(u8::from(format.has_video)),
            u64::from(format.height.unwrap_or(0)),
            u64::from(format.width.unwrap_or(0)),
            format.bitrate.unwrap_or(0),
            0,
        ),
    })
}

fn format_matches_predicate(format: &MediaFormat, predicate: &str) -> Result<bool> {
    let ops = ["<=", ">=", "!=", "=", "<", ">"];
    let (field, op, value) = ops
        .iter()
        .find_map(|op| predicate.split_once(op).map(|(field, value)| (field, *op, value)))
        .ok_or_else(|| {
            VesselError::Unsupported(format!("unsupported selector predicate `{predicate}`"))
        })?;

    let field = field.trim();
    let value = value.trim();
    if field.is_empty() || value.is_empty() {
        return Err(VesselError::Unsupported(format!(
            "unsupported selector predicate `{predicate}`"
        )));
    }

    match field {
        "ext" => compare_string(&format.ext, op, value),
        "protocol" => Ok(compare_string_option(format.protocol.as_deref(), op, value)),
        "format_id" => compare_string(&format.format_id, op, value),
        "acodec" => Ok(compare_string_option(format.audio_codec.as_deref(), op, value)),
        "vcodec" => Ok(compare_string_option(format.video_codec.as_deref(), op, value)),
        "height" => compare_u32_option(format.height, op, value),
        "width" => compare_u32_option(format.width, op, value),
        "tbr" | "bitrate" => compare_u64_option(format.bitrate, op, value),
        _ => Err(VesselError::Unsupported(format!(
            "unsupported selector field `{field}`"
        ))),
    }
}

fn compare_string(left: &str, op: &str, right: &str) -> Result<bool> {
    match op {
        "=" => Ok(left == right),
        "!=" => Ok(left != right),
        _ => Err(VesselError::Unsupported(format!(
            "unsupported string comparison operator `{op}`"
        ))),
    }
}

fn compare_string_option(left: Option<&str>, op: &str, right: &str) -> bool {
    match op {
        "=" => left == Some(right),
        "!=" => left != Some(right),
        _ => false,
    }
}

fn compare_u32_option(left: Option<u32>, op: &str, right: &str) -> Result<bool> {
    compare_u64_option(left.map(u64::from), op, right)
}

fn compare_u64_option(left: Option<u64>, op: &str, right: &str) -> Result<bool> {
    let right = right.parse::<u64>().map_err(|_| {
        VesselError::Unsupported(format!("numeric selector comparison requires a number: `{right}`"))
    })?;
    let Some(left) = left else {
        return Ok(false);
    };

    let result = match op {
        "=" => left == right,
        "!=" => left != right,
        "<" => left < right,
        "<=" => left <= right,
        ">" => left > right,
        ">=" => left >= right,
        _ => {
            return Err(VesselError::Unsupported(format!(
                "unsupported numeric comparison operator `{op}`"
            )))
        }
    };
    Ok(result)
}

fn render_output_path(item: &VideoMetadata, format: &MediaFormat, template: &str) -> PathBuf {
    let mut output = template.to_owned();
    output = output.replace("%(id)s", &sanitize_component(&item.video_id));
    output = output.replace(
        "%(title)s",
        &sanitize_component(item.title.as_deref().unwrap_or(&item.video_id)),
    );
    output = output.replace(
        "%(channel)s",
        &sanitize_component(item.channel_id.as_deref().unwrap_or("unknown-channel")),
    );
    output = output.replace(
        "%(upload_date)s",
        &sanitize_component(item.upload_date.as_deref().unwrap_or("unknown-date")),
    );
    output = output.replace("%(ext)s", &format.ext);
    Path::new(&output).to_path_buf()
}

fn sanitize_component(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ if ch.is_control() => '_',
            _ => ch,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{BasicDownloadPlanner, DownloadPlanner};
    use vessel_core::models::{Availability, MediaFormat, Platform, VideoMetadata};
    use vessel_formats::parse_selector;

    fn sample_video() -> VideoMetadata {
        VideoMetadata {
            platform: Platform::YouTube,
            video_id: "video123".to_owned(),
            channel_id: Some("channel123".to_owned()),
            url: "https://www.youtube.com/watch?v=video123".to_owned(),
            title: Some("Test Video".to_owned()),
            description: None,
            duration_seconds: Some(10),
            upload_date: Some("20260101".to_owned()),
            release_timestamp: None,
            view_count: None,
            like_count: None,
            comment_count: None,
            availability: Availability::Public,
            formats: vec![
                MediaFormat {
                    format_id: "18".to_owned(),
                    ext: "mp4".to_owned(),
                    note: None,
                    video_codec: Some("avc1".to_owned()),
                    audio_codec: Some("mp4a".to_owned()),
                    download_url: Some("https://example.test/18".to_owned()),
                    protocol: Some("https".to_owned()),
                    width: Some(640),
                    height: Some(360),
                    bitrate: Some(500),
                    has_video: true,
                    has_audio: true,
                },
                MediaFormat {
                    format_id: "137".to_owned(),
                    ext: "mp4".to_owned(),
                    note: None,
                    video_codec: Some("avc1".to_owned()),
                    audio_codec: None,
                    download_url: Some("https://example.test/137".to_owned()),
                    protocol: Some("https".to_owned()),
                    width: Some(1920),
                    height: Some(1080),
                    bitrate: Some(2500),
                    has_video: true,
                    has_audio: false,
                },
                MediaFormat {
                    format_id: "22".to_owned(),
                    ext: "mp4".to_owned(),
                    note: None,
                    video_codec: Some("avc1".to_owned()),
                    audio_codec: Some("mp4a".to_owned()),
                    download_url: Some("https://example.test/22".to_owned()),
                    protocol: Some("https".to_owned()),
                    width: Some(1280),
                    height: Some(720),
                    bitrate: Some(1800),
                    has_video: true,
                    has_audio: true,
                },
                MediaFormat {
                    format_id: "251".to_owned(),
                    ext: "webm".to_owned(),
                    note: None,
                    video_codec: None,
                    audio_codec: Some("opus".to_owned()),
                    download_url: Some("https://example.test/251".to_owned()),
                    protocol: Some("https".to_owned()),
                    width: None,
                    height: None,
                    bitrate: Some(160),
                    has_video: false,
                    has_audio: true,
                },
            ],
            subtitles: Vec::new(),
            thumbnails: Vec::new(),
            fetched_at: time::OffsetDateTime::UNIX_EPOCH,
            raw: serde_json::Value::Null,
        }
    }

    #[test]
    fn fallback_selector_resolves_to_best_when_merge_is_not_executable() {
        let planner = BasicDownloadPlanner;
        let selector = parse_selector("bestvideo+bestaudio/best").unwrap();
        let plan = planner.plan(&sample_video(), selector, "%(id)s.%(ext)s").unwrap();
        assert_eq!(plan.format_id, "22");
    }

    #[test]
    fn filtered_best_prefers_matching_ext() {
        let planner = BasicDownloadPlanner;
        let selector = parse_selector("best[ext=mp4]").unwrap();
        let plan = planner.plan(&sample_video(), selector, "%(id)s.%(ext)s").unwrap();
        assert_eq!(plan.format_id, "22");
    }

    #[test]
    fn filtered_bestvideo_honors_height_cap() {
        let planner = BasicDownloadPlanner;
        let selector = parse_selector("bestvideo[height<=720]").unwrap();
        let plan = planner.plan(&sample_video(), selector, "%(id)s.%(ext)s").unwrap();
        assert_eq!(plan.format_id, "22");
    }

    #[test]
    fn bestaudio_prefers_audio_only_track() {
        let planner = BasicDownloadPlanner;
        let selector = parse_selector("bestaudio").unwrap();
        let plan = planner.plan(&sample_video(), selector, "%(id)s.%(ext)s").unwrap();
        assert_eq!(plan.format_id, "251");
    }
}
