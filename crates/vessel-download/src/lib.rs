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
        FormatSelector::ExactFormatId(format_id) => item
            .formats
            .iter()
            .find(|format| format.format_id == *format_id && format.download_url.is_some())
            .ok_or_else(|| {
                VesselError::Unsupported(format!(
                    "requested format `{format_id}` is not downloadable"
                ))
            }),
        _ => best_downloadable_format(&item.formats).ok_or_else(|| {
            VesselError::Unsupported("selector not implemented for download".to_owned())
        }),
    }
}

fn best_downloadable_format(formats: &[MediaFormat]) -> Option<&MediaFormat> {
    formats
        .iter()
        .filter(|format| format.download_url.is_some())
        .max_by_key(|format| {
            (
                u8::from(format.has_video && format.has_audio),
                format.height.unwrap_or(0),
                format.bitrate.unwrap_or(0),
            )
        })
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
