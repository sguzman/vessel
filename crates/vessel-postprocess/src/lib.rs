use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use vessel_core::{Result, VesselError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PostprocessStep {
    Merge,
    Remux { container: String },
    ExtractAudio { format: String },
    EmbedMetadata,
    EmbedThumbnail,
    ConvertSubtitles { format: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostprocessPlan {
    pub steps: Vec<PostprocessStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostprocessRequest {
    pub video_title: Option<String>,
    pub channel_id: Option<String>,
    pub description: Option<String>,
    pub final_output_path: PathBuf,
    pub media_inputs: Vec<PathBuf>,
    pub thumbnail_path: Option<PathBuf>,
    pub subtitle_paths: Vec<PathBuf>,
    pub remux_video: Option<String>,
    pub extract_audio: Option<String>,
    pub embed_metadata: bool,
    pub embed_thumbnail: bool,
    pub convert_subtitles: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedArtifact {
    pub kind: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostprocessExecution {
    pub final_media_path: Option<PathBuf>,
    pub generated_artifacts: Vec<GeneratedArtifact>,
}

pub fn build_plan(request: &PostprocessRequest) -> PostprocessPlan {
    let mut steps = Vec::new();
    if request.media_inputs.len() > 1 {
        steps.push(PostprocessStep::Merge);
    }
    if let Some(container) = &request.remux_video {
        steps.push(PostprocessStep::Remux {
            container: container.clone(),
        });
    }
    if let Some(format) = &request.extract_audio {
        steps.push(PostprocessStep::ExtractAudio {
            format: format.clone(),
        });
    }
    if request.embed_metadata {
        steps.push(PostprocessStep::EmbedMetadata);
    }
    if request.embed_thumbnail {
        steps.push(PostprocessStep::EmbedThumbnail);
    }
    if let Some(format) = &request.convert_subtitles {
        steps.push(PostprocessStep::ConvertSubtitles {
            format: format.clone(),
        });
    }
    PostprocessPlan { steps }
}

pub async fn execute_plan(
    request: &PostprocessRequest,
    plan: &PostprocessPlan,
) -> Result<PostprocessExecution> {
    let mut current_media = request.media_inputs.first().cloned();
    let mut generated_artifacts = Vec::new();

    for (index, step) in plan.steps.iter().enumerate() {
        match step {
            PostprocessStep::Merge => {
                if request.media_inputs.len() < 2 {
                    return Err(VesselError::Unsupported(
                        "merge postprocess requires at least two media inputs".to_owned(),
                    ));
                }
                let output_path = media_step_output_path(
                    request,
                    plan,
                    index,
                    &merge_intermediate_extension(&request.media_inputs),
                );
                ensure_parent(&output_path).await?;
                merge_inputs(&request.media_inputs, &output_path).await?;
                cleanup_inputs(&request.media_inputs, &output_path).await?;
                current_media = Some(output_path);
            }
            PostprocessStep::Remux { container } => {
                let input = current_media.clone().ok_or_else(|| {
                    VesselError::Unsupported("remux postprocess requires a media input".to_owned())
                })?;
                let output_path =
                    media_step_output_path(request, plan, index, container.as_str());
                remux_input(&input, &output_path).await?;
                cleanup_input(&input, &output_path).await?;
                current_media = Some(output_path);
            }
            PostprocessStep::ExtractAudio { format } => {
                let input = current_media.clone().ok_or_else(|| {
                    VesselError::Unsupported(
                        "audio extraction postprocess requires a media input".to_owned(),
                    )
                })?;
                let output_path = media_step_output_path(request, plan, index, format.as_str());
                extract_audio(&input, &output_path, format).await?;
                cleanup_input(&input, &output_path).await?;
                current_media = Some(output_path);
            }
            PostprocessStep::EmbedMetadata => {
                let input = current_media.clone().ok_or_else(|| {
                    VesselError::Unsupported(
                        "metadata embedding postprocess requires a media input".to_owned(),
                    )
                })?;
                let ext = extension_or_bin(&input);
                let output_path = media_step_output_path(request, plan, index, ext.as_str());
                embed_metadata(&input, &output_path, request).await?;
                cleanup_input(&input, &output_path).await?;
                current_media = Some(output_path);
            }
            PostprocessStep::EmbedThumbnail => {
                let input = current_media.clone().ok_or_else(|| {
                    VesselError::Unsupported(
                        "thumbnail embedding postprocess requires a media input".to_owned(),
                    )
                })?;
                let thumbnail_path = request.thumbnail_path.as_ref().ok_or_else(|| {
                    VesselError::Unsupported(
                        "thumbnail embedding requested but no thumbnail was provided".to_owned(),
                    )
                })?;
                let ext = extension_or_bin(&input);
                let output_path = media_step_output_path(request, plan, index, ext.as_str());
                embed_thumbnail(&input, thumbnail_path, &output_path).await?;
                cleanup_input(&input, &output_path).await?;
                current_media = Some(output_path);
            }
            PostprocessStep::ConvertSubtitles { format } => {
                for subtitle_path in &request.subtitle_paths {
                    let output_path = subtitle_path.with_extension(format);
                    convert_subtitle(subtitle_path, &output_path).await?;
                    generated_artifacts.push(GeneratedArtifact {
                        kind: "subtitle".to_owned(),
                        path: output_path,
                    });
                }
            }
        }
    }

    Ok(PostprocessExecution {
        final_media_path: current_media,
        generated_artifacts,
    })
}

async fn merge_inputs(inputs: &[PathBuf], output_path: &Path) -> Result<()> {
    let mut args = vec![OsString::from("-y")];
    for input in inputs {
        args.push(OsString::from("-i"));
        args.push(input.as_os_str().to_os_string());
    }
    args.extend([
        OsString::from("-map"),
        OsString::from("0:v:0"),
        OsString::from("-map"),
        OsString::from("1:a:0"),
        OsString::from("-c"),
        OsString::from("copy"),
        output_path.as_os_str().to_os_string(),
    ]);
    run_ffmpeg(args).await
}

async fn remux_input(input: &Path, output_path: &Path) -> Result<()> {
    run_ffmpeg(vec![
        OsString::from("-y"),
        OsString::from("-i"),
        input.as_os_str().to_os_string(),
        OsString::from("-map"),
        OsString::from("0"),
        OsString::from("-c"),
        OsString::from("copy"),
        output_path.as_os_str().to_os_string(),
    ])
    .await
}

async fn extract_audio(input: &Path, output_path: &Path, format: &str) -> Result<()> {
    let codec_args = audio_codec_args(format)?;
    let mut args = vec![
        OsString::from("-y"),
        OsString::from("-i"),
        input.as_os_str().to_os_string(),
        OsString::from("-vn"),
    ];
    args.extend(codec_args.into_iter().map(OsString::from));
    args.push(output_path.as_os_str().to_os_string());
    run_ffmpeg(args).await
}

async fn embed_metadata(
    input: &Path,
    output_path: &Path,
    request: &PostprocessRequest,
) -> Result<()> {
    let mut args = vec![
        OsString::from("-y"),
        OsString::from("-i"),
        input.as_os_str().to_os_string(),
        OsString::from("-map"),
        OsString::from("0"),
        OsString::from("-c"),
        OsString::from("copy"),
    ];
    if let Some(title) = &request.video_title {
        args.push(OsString::from("-metadata"));
        args.push(OsString::from(format!("title={title}")));
    }
    if let Some(channel_id) = &request.channel_id {
        args.push(OsString::from("-metadata"));
        args.push(OsString::from(format!("artist={channel_id}")));
    }
    if let Some(description) = &request.description {
        args.push(OsString::from("-metadata"));
        args.push(OsString::from(format!("comment={description}")));
    }
    args.push(output_path.as_os_str().to_os_string());
    run_ffmpeg(args).await
}

async fn embed_thumbnail(input: &Path, thumbnail_path: &Path, output_path: &Path) -> Result<()> {
    let ext = extension_or_bin(output_path);
    let mut args = vec![
        OsString::from("-y"),
        OsString::from("-i"),
        input.as_os_str().to_os_string(),
        OsString::from("-i"),
        thumbnail_path.as_os_str().to_os_string(),
    ];
    if matches!(ext.as_str(), "mp3") {
        args.extend([
            OsString::from("-map"),
            OsString::from("0"),
            OsString::from("-map"),
            OsString::from("1"),
            OsString::from("-c"),
            OsString::from("copy"),
            OsString::from("-id3v2_version"),
            OsString::from("3"),
            OsString::from("-metadata:s:v"),
            OsString::from("title=Album cover"),
            OsString::from("-metadata:s:v"),
            OsString::from("comment=Cover (front)"),
        ]);
    } else {
        args.extend([
            OsString::from("-map"),
            OsString::from("0"),
            OsString::from("-map"),
            OsString::from("1"),
            OsString::from("-c"),
            OsString::from("copy"),
            OsString::from("-c:v:1"),
            OsString::from("mjpeg"),
            OsString::from("-disposition:v:1"),
            OsString::from("attached_pic"),
        ]);
    }
    args.push(output_path.as_os_str().to_os_string());
    run_ffmpeg(args).await
}

async fn convert_subtitle(input: &Path, output_path: &Path) -> Result<()> {
    run_ffmpeg(vec![
        OsString::from("-y"),
        OsString::from("-i"),
        input.as_os_str().to_os_string(),
        output_path.as_os_str().to_os_string(),
    ])
    .await
}

fn audio_codec_args(format: &str) -> Result<Vec<&'static str>> {
    match format {
        "mp3" => Ok(vec!["-c:a", "libmp3lame", "-b:a", "192k"]),
        "m4a" => Ok(vec!["-c:a", "aac", "-b:a", "192k"]),
        "opus" => Ok(vec!["-c:a", "libopus", "-b:a", "160k"]),
        "flac" => Ok(vec!["-c:a", "flac"]),
        other => Err(VesselError::Unsupported(format!(
            "unsupported audio extraction format `{other}`"
        ))),
    }
}

async fn run_ffmpeg(args: Vec<OsString>) -> Result<()> {
    let output = Command::new("ffmpeg").args(&args).output().await?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(VesselError::Extractor(format!(
        "ffmpeg postprocess failed: {}",
        stderr.trim()
    )))
}

async fn cleanup_inputs(inputs: &[PathBuf], keep: &Path) -> Result<()> {
    for input in inputs {
        cleanup_input(input, keep).await?;
    }
    Ok(())
}

async fn cleanup_input(input: &Path, keep: &Path) -> Result<()> {
    if input == keep {
        return Ok(());
    }
    match tokio::fs::remove_file(input).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

async fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(())
}

fn step_output_path(final_output_path: &Path, index: usize, extension: &str) -> PathBuf {
    let stem = final_output_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    final_output_path.with_file_name(format!("{stem}.vessel-step-{index}.{extension}"))
}

fn extension_or_bin(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bin")
        .to_owned()
}

fn media_step_output_path(
    request: &PostprocessRequest,
    plan: &PostprocessPlan,
    index: usize,
    extension: &str,
) -> PathBuf {
    if has_later_media_step(plan, index) {
        step_output_path(&request.final_output_path, index, extension)
    } else {
        request.final_output_path.clone()
    }
}

fn has_later_media_step(plan: &PostprocessPlan, index: usize) -> bool {
    plan.steps
        .iter()
        .skip(index + 1)
        .any(|step| !matches!(step, PostprocessStep::ConvertSubtitles { .. }))
}

fn merge_intermediate_extension(inputs: &[PathBuf]) -> String {
    let left = inputs
        .first()
        .and_then(|path| path.extension())
        .and_then(|value| value.to_str())
        .unwrap_or("mkv");
    let right = inputs
        .get(1)
        .and_then(|path| path.extension())
        .and_then(|value| value.to_str())
        .unwrap_or(left);
    if left == right && matches!(left, "mp4" | "webm" | "mkv") {
        left.to_owned()
    } else if matches!(left, "mp4" | "m4a") && matches!(right, "mp4" | "m4a") {
        "mp4".to_owned()
    } else {
        "mkv".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{PostprocessRequest, PostprocessStep, build_plan, execute_plan};
    use std::path::PathBuf;
    use std::process::Command;
    use uuid::Uuid;

    fn base_request() -> PostprocessRequest {
        PostprocessRequest {
            video_title: Some("Title".to_owned()),
            channel_id: Some("Channel".to_owned()),
            description: Some("Description".to_owned()),
            final_output_path: PathBuf::from("downloads/video.mp4"),
            media_inputs: vec![PathBuf::from("downloads/video.f137.mp4")],
            thumbnail_path: None,
            subtitle_paths: Vec::new(),
            remux_video: None,
            extract_audio: None,
            embed_metadata: false,
            embed_thumbnail: false,
            convert_subtitles: None,
        }
    }

    #[test]
    fn builds_merge_and_remux_steps_deterministically() {
        let mut request = base_request();
        request.media_inputs.push(PathBuf::from("downloads/video.f251.webm"));
        request.remux_video = Some("mkv".to_owned());
        let plan = build_plan(&request);
        assert_eq!(
            plan.steps,
            vec![
                PostprocessStep::Merge,
                PostprocessStep::Remux {
                    container: "mkv".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn builds_full_optional_plan() {
        let mut request = base_request();
        request.extract_audio = Some("mp3".to_owned());
        request.embed_metadata = true;
        request.embed_thumbnail = true;
        request.convert_subtitles = Some("srt".to_owned());
        request.thumbnail_path = Some(PathBuf::from("thumb.jpg"));
        request.subtitle_paths = vec![PathBuf::from("subtitles/en.vtt")];
        let plan = build_plan(&request);
        assert_eq!(
            plan.steps,
            vec![
                PostprocessStep::ExtractAudio {
                    format: "mp3".to_owned(),
                },
                PostprocessStep::EmbedMetadata,
                PostprocessStep::EmbedThumbnail,
                PostprocessStep::ConvertSubtitles {
                    format: "srt".to_owned(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn executes_merge_audio_extract_and_subtitle_conversion() {
        let fixture_dir = std::env::temp_dir().join(format!("vessel-pp-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&fixture_dir).unwrap();

        let video_path = fixture_dir.join("video.mp4");
        let audio_path = fixture_dir.join("audio.m4a");
        let thumb_path = fixture_dir.join("thumb.jpg");
        let subtitle_path = fixture_dir.join("en.vtt");
        let final_output_path = fixture_dir.join("output.mp3");

        ffmpeg_fixture(&[
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=1",
            "-t",
            "1",
            "-an",
            video_path.to_str().unwrap(),
        ]);
        ffmpeg_fixture(&[
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=44100",
            "-t",
            "1",
            "-vn",
            "-c:a",
            "aac",
            audio_path.to_str().unwrap(),
        ]);
        ffmpeg_fixture(&[
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=64x64",
            "-frames:v",
            "1",
            thumb_path.to_str().unwrap(),
        ]);
        std::fs::write(
            &subtitle_path,
            "WEBVTT\n\n00:00:00.000 --> 00:00:00.500\nhello world\n",
        )
        .unwrap();

        let request = PostprocessRequest {
            video_title: Some("Title".to_owned()),
            channel_id: Some("Channel".to_owned()),
            description: Some("Description".to_owned()),
            final_output_path,
            media_inputs: vec![video_path, audio_path],
            thumbnail_path: Some(thumb_path),
            subtitle_paths: vec![subtitle_path.clone()],
            remux_video: None,
            extract_audio: Some("mp3".to_owned()),
            embed_metadata: true,
            embed_thumbnail: true,
            convert_subtitles: Some("srt".to_owned()),
        };
        let plan = build_plan(&request);
        let execution = execute_plan(&request, &plan).await.unwrap();

        let final_media = execution.final_media_path.unwrap();
        assert!(final_media.exists());
        assert_eq!(final_media.extension().and_then(|value| value.to_str()), Some("mp3"));
        let converted = subtitle_path.with_extension("srt");
        assert!(converted.exists());
        assert_eq!(execution.generated_artifacts.len(), 1);
    }

    fn ffmpeg_fixture(args: &[&str]) {
        let status = Command::new("ffmpeg").args(args).status().unwrap();
        assert!(status.success());
    }
}
