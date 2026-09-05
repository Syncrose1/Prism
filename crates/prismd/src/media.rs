//! Media probing and on-the-fly transcoding.
//!
//! Most files stream directly from `/api/files/raw` with range support, which
//! is always preferable: no CPU, no GPU, instant seeking, and the original
//! quality. Transcoding exists only for what a browser genuinely cannot decode
//! — an MKV container, HEVC video, AC3 or DTS audio — where the alternative is
//! a black rectangle.
//!
//! The decision is made from the file's actual streams via `ffprobe`, not from
//! its extension. An `.mkv` holding H.264 and AAC needs only a container change
//! and is remuxed without re-encoding; guessing from the name would burn the
//! GPU on a file that never needed it.
//!
//! Encoding uses NVENC when present. On this host that is close to free, which
//! matters: the operator streams to a phone from a machine that is often also
//! generating images, and a software encode would compete with the workload
//! Prism is meant to be protecting.

use serde::Serialize;
use std::path::Path;

/// What a browser would need in order to play a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Playability {
    /// Stream the original bytes.
    Direct,
    /// Container change only — no re-encode, so it costs almost nothing.
    Remux { video: String, audio: String },
    /// At least one stream must be re-encoded.
    Transcode {
        video: String,
        audio: String,
        /// Which streams actually need work, for the UI to explain the wait.
        reencode_video: bool,
        reencode_audio: bool,
    },
    /// Not a media file, or unreadable.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaInfo {
    pub duration_secs: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub container: Option<String>,
    pub playability: Playability,
}

/// Containers a browser will accept.
///
/// Deliberately narrow. Chrome will sometimes play more than this, but a false
/// positive means a black rectangle and no explanation, whereas a false
/// negative costs a remux that is nearly free.
fn container_ok(format_names: &str) -> bool {
    format_names
        .split(',')
        .any(|f| matches!(f.trim(), "mov" | "mp4" | "m4a" | "3gp" | "webm" | "matroska"))
        // matroska appears in the same list as webm; only webm is safe.
        && !format_names.split(',').all(|f| f.trim() == "matroska")
}

fn video_ok(codec: &str) -> bool {
    matches!(codec, "h264" | "vp8" | "vp9" | "av1")
}

fn audio_ok(codec: &str) -> bool {
    matches!(codec, "aac" | "mp3" | "opus" | "vorbis" | "flac")
}

/// Ask ffprobe what is actually in the file.
pub async fn probe(path: &Path) -> MediaInfo {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .await;

    let Ok(output) = output else {
        return unknown();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return unknown();
    };

    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let find = |kind: &str| {
        streams
            .iter()
            .find(|s| s["codec_type"].as_str() == Some(kind))
            .cloned()
    };

    let video = find("video");
    let audio = find("audio");
    let container = json["format"]["format_name"].as_str().unwrap_or("").to_string();

    let video_codec = video
        .as_ref()
        .and_then(|v| v["codec_name"].as_str())
        .map(str::to_string);
    let audio_codec = audio
        .as_ref()
        .and_then(|a| a["codec_name"].as_str())
        .map(str::to_string);

    // A file with neither is not media.
    if video_codec.is_none() && audio_codec.is_none() {
        return unknown();
    }

    let v_ok = video_codec.as_deref().is_none_or(video_ok);
    let a_ok = audio_codec.as_deref().is_none_or(audio_ok);
    let c_ok = container_ok(&container);

    let playability = if v_ok && a_ok && c_ok {
        Playability::Direct
    } else if v_ok && a_ok {
        // Only the wrapper is wrong: copy both streams into MP4.
        Playability::Remux {
            video: "copy".into(),
            audio: "copy".into(),
        }
    } else {
        Playability::Transcode {
            video: if v_ok { "copy".into() } else { "h264".into() },
            audio: if a_ok { "copy".into() } else { "aac".into() },
            reencode_video: !v_ok,
            reencode_audio: !a_ok,
        }
    };

    MediaInfo {
        duration_secs: json["format"]["duration"]
            .as_str()
            .and_then(|d| d.parse().ok()),
        width: video.as_ref().and_then(|v| v["width"].as_u64()).map(|w| w as u32),
        height: video.as_ref().and_then(|v| v["height"].as_u64()).map(|h| h as u32),
        video_codec,
        audio_codec,
        container: Some(container),
        playability,
    }
}

fn unknown() -> MediaInfo {
    MediaInfo {
        duration_secs: None,
        width: None,
        height: None,
        video_codec: None,
        audio_codec: None,
        container: None,
        playability: Playability::Unknown,
    }
}

/// Build the ffmpeg arguments for streaming a file the browser can play.
///
/// `start_secs` seeks before decoding, which is what makes seeking in a
/// transcoded stream tolerable: ffmpeg jumps to the nearest keyframe instead of
/// decoding from the beginning. The cost is that seeks land on a keyframe
/// rather than exactly where asked, which is the usual trade and the one every
/// streaming server makes.
pub fn transcode_args(
    path: &Path,
    info: &MediaInfo,
    start_secs: f64,
    nvenc: bool,
) -> Vec<String> {
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into()];

    if start_secs > 0.0 {
        args.push("-ss".into());
        args.push(format!("{start_secs:.3}"));
    }
    args.push("-i".into());
    args.push(path.display().to_string());

    let (v, a) = match &info.playability {
        Playability::Remux { .. } => ("copy".to_string(), "copy".to_string()),
        Playability::Transcode {
            reencode_video,
            reencode_audio,
            ..
        } => (
            if *reencode_video {
                if nvenc { "h264_nvenc".into() } else { "libx264".to_string() }
            } else {
                "copy".to_string()
            },
            if *reencode_audio { "aac".to_string() } else { "copy".to_string() },
        ),
        // Direct and Unknown should not reach here, but copying is the
        // harmless answer rather than a panic.
        _ => ("copy".to_string(), "copy".to_string()),
    };

    args.push("-c:v".into());
    args.push(v.clone());
    if v.contains("nvenc") {
        // Latency over ratio: this is being watched now, not archived.
        args.push("-preset".into());
        args.push("p4".into());
        args.push("-tune".into());
        args.push("ll".into());
    }
    args.push("-c:a".into());
    args.push(a);

    // Fragmented MP4, so playback can begin before the file is finished — a
    // normal MP4 puts its index at the end and would have to be fully
    // transcoded first.
    args.push("-movflags".into());
    args.push("frag_keyframe+empty_moov+default_base_moof".into());
    args.push("-f".into());
    args.push("mp4".into());
    args.push("pipe:1".into());
    args
}

pub fn have_nvenc() -> bool {
    std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("h264_nvenc"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_friendly_codecs_are_recognised() {
        assert!(video_ok("h264") && video_ok("vp9") && video_ok("av1"));
        assert!(!video_ok("hevc") && !video_ok("mpeg2video") && !video_ok("wmv3"));
        assert!(audio_ok("aac") && audio_ok("opus"));
        assert!(!audio_ok("ac3") && !audio_ok("dts"));
    }

    #[test]
    fn matroska_alone_is_not_playable_but_webm_is() {
        // ffprobe reports "matroska,webm" for both; only webm is safe, so the
        // shared name must not be taken as permission.
        assert!(!container_ok("matroska"));
        assert!(container_ok("mov,mp4,m4a,3gp,3g2,mj2"));
    }

    #[test]
    fn transcode_args_seek_before_decoding() {
        // -ss after -i decodes from the start, which on a two-hour file is the
        // difference between instant and unusable.
        let info = MediaInfo {
            playability: Playability::Transcode {
                video: "h264".into(),
                audio: "aac".into(),
                reencode_video: true,
                reencode_audio: true,
            },
            ..unknown()
        };
        let args = transcode_args(Path::new("/x.mkv"), &info, 90.0, true);
        let ss = args.iter().position(|a| a == "-ss").expect("seeks");
        let i = args.iter().position(|a| a == "-i").expect("has input");
        assert!(ss < i, "-ss must precede -i: {args:?}");
    }

    #[test]
    fn a_remux_never_re_encodes() {
        // The whole point: an MKV of H.264 and AAC costs a container change,
        // not a GPU.
        let info = MediaInfo {
            playability: Playability::Remux {
                video: "copy".into(),
                audio: "copy".into(),
            },
            ..unknown()
        };
        let args = transcode_args(Path::new("/x.mkv"), &info, 0.0, true);
        let joined = args.join(" ");
        assert!(joined.contains("-c:v copy"), "{joined}");
        assert!(joined.contains("-c:a copy"), "{joined}");
        assert!(!joined.contains("nvenc"), "{joined}");
    }

    #[test]
    fn only_the_stream_that_needs_it_is_re_encoded() {
        // H.264 video with AC3 audio: keep the video, convert the audio.
        let info = MediaInfo {
            playability: Playability::Transcode {
                video: "copy".into(),
                audio: "aac".into(),
                reencode_video: false,
                reencode_audio: true,
            },
            ..unknown()
        };
        let joined = transcode_args(Path::new("/x.mkv"), &info, 0.0, true).join(" ");
        assert!(joined.contains("-c:v copy"), "{joined}");
        assert!(joined.contains("-c:a aac"), "{joined}");
    }

    #[test]
    fn software_encoding_is_used_when_nvenc_is_absent() {
        let info = MediaInfo {
            playability: Playability::Transcode {
                video: "h264".into(),
                audio: "copy".into(),
                reencode_video: true,
                reencode_audio: false,
            },
            ..unknown()
        };
        let joined = transcode_args(Path::new("/x.mkv"), &info, 0.0, false).join(" ");
        assert!(joined.contains("libx264"), "{joined}");
    }

    #[test]
    fn output_is_fragmented_so_playback_can_start_early() {
        // A plain MP4 puts its index at the end, so the whole file would have
        // to be transcoded before anything played.
        let info = MediaInfo {
            playability: Playability::Remux {
                video: "copy".into(),
                audio: "copy".into(),
            },
            ..unknown()
        };
        let joined = transcode_args(Path::new("/x.mkv"), &info, 0.0, true).join(" ");
        assert!(joined.contains("frag_keyframe"), "{joined}");
        assert!(joined.contains("empty_moov"), "{joined}");
    }

    #[tokio::test]
    async fn a_missing_file_probes_as_unknown() {
        let info = probe(Path::new("/nonexistent/xyzzy.mkv")).await;
        assert_eq!(info.playability, Playability::Unknown);
    }

    #[tokio::test]
    async fn a_non_media_file_probes_as_unknown() {
        let dir = std::env::temp_dir().join(format!("prism-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("notes.txt");
        std::fs::write(&f, b"not media").unwrap();
        assert_eq!(probe(&f).await.playability, Playability::Unknown);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
