//! The shipping recording editor's sampled estimates and encoded comparison.
//! Workers receive immutable export specs; no GPUI objects or source writes.
use captures_media::{
    CancelToken, EditSpec, ExportFormat, ExportSpec, MediaToolchain, QualityPreset,
    estimate_sample_windows, export_preserves_source_bytes, extrapolate_sampled_size,
    visual_edit_is_identity,
};
use std::path::Path;

pub struct Result {
    pub estimate: Option<(u64, bool)>,
    pub frames: Option<(image::RgbaImage, image::RgbaImage)>,
}

pub fn render(
    input: &Path,
    edit: &EditSpec,
    export: &ExportSpec,
    at_ms: Option<u64>,
    cancel: &CancelToken,
) -> anyhow::Result<Result> {
    let tools = MediaToolchain::from_command_names();
    let probe = tools.probe(input)?;
    let duration = probe
        .metadata
        .duration_ms
        .ok_or_else(|| anyhow::anyhow!("Recording duration is unavailable"))?;
    let start = edit.trim_start_ms.min(duration.saturating_sub(1));
    let end = edit
        .trim_end_ms
        .unwrap_or(duration)
        .min(duration)
        .max(start + 1);
    let trimmed = end - start;
    let scratch = tempfile::tempdir()?;
    let extension = if export.format == ExportFormat::Gif {
        "gif"
    } else {
        "mp4"
    };
    let estimate = if export.max_size_bytes.is_some() || export.format == ExportFormat::WebM {
        None
    } else if export_preserves_source_bytes(&probe, edit, export) {
        Some((probe.metadata.size_bytes, true))
    } else if export.format == ExportFormat::Mp4
        && export.quality == QualityPreset::Preserve
        && visual_edit_is_identity(&probe, edit)
    {
        Some((probe.metadata.size_bytes, false))
    } else {
        let windows = estimate_sample_windows(start, trimmed);
        let mut bytes = 0_u64;
        let mut milliseconds = 0_u64;
        for (i, (at, length)) in windows.iter().copied().enumerate() {
            anyhow::ensure!(!cancel.is_cancelled(), "Estimate cancelled");
            let sample = EditSpec {
                trim_start_ms: at,
                trim_end_ms: Some(at + length),
                ..edit.clone()
            };
            let output = scratch.path().join(format!("estimate-{i}.{extension}"));
            let result = tools.export(input, &output, &sample, export, cancel, |_| {})?;
            bytes = bytes.saturating_add(result.size_bytes);
            milliseconds = milliseconds.saturating_add(length);
        }
        Some((
            extrapolate_sampled_size(bytes, milliseconds, trimmed),
            windows.len() == 1,
        ))
    };
    let frames = if let Some(at) = at_ms {
        anyhow::ensure!(!cancel.is_cancelled(), "Preview cancelled");
        let at = at.clamp(start, end - 1);
        let length = 1_500.min(trimmed);
        let sample_start = at.saturating_sub(1_000).min(end - length).max(start);
        let sample = EditSpec {
            trim_start_ms: sample_start,
            trim_end_ms: Some(sample_start + length),
            ..edit.clone()
        };
        let output = scratch.path().join(format!("preview.{extension}"));
        tools.export(
            input,
            &output,
            &sample,
            &sample_export(export, length, trimmed),
            cancel,
            |_| {},
        )?;
        let before = scratch.path().join("before.png");
        let after = scratch.path().join("after.png");
        tools.extract_edited_frame(input, edit, export, at, &before, cancel)?;
        tools.extract_frame(&output, at - sample_start, &after, cancel)?;
        Some((
            image::open(before)?.to_rgba8(),
            image::open(after)?.to_rgba8(),
        ))
    } else {
        None
    };
    Ok(Result { estimate, frames })
}

fn sample_export(export: &ExportSpec, length: u64, duration: u64) -> ExportSpec {
    let mut sample = export.clone();
    sample.max_size_bytes = export.max_size_bytes.map(|cap| {
        u64::try_from(u128::from(cap) * u128::from(length) / u128::from(duration.max(1)))
            .unwrap_or(cap)
            .max(1)
    });
    sample
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn preserve_mp4() -> ExportSpec {
        ExportSpec {
            format: ExportFormat::Mp4,
            quality: QualityPreset::Preserve,
            max_size_bytes: None,
            frames_per_second: None,
            gif_max_colors: None,
        }
    }

    #[test]
    fn sample_cap_is_proportional_without_overflow() {
        let export = ExportSpec {
            max_size_bytes: Some(7_000_000),
            ..preserve_mp4()
        };
        assert_eq!(
            sample_export(&export, 1_500, 10_000).max_size_bytes,
            Some(1_050_000)
        );
        let export = ExportSpec {
            max_size_bytes: Some(u64::MAX),
            ..export
        };
        assert_eq!(sample_export(&export, 3, 3).max_size_bytes, Some(u64::MAX));
    }

    #[test]
    fn real_encoded_estimate_and_comparison_preserve_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.mp4");
        assert!(
            Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=160x90:rate=10",
                    "-t",
                    "2",
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p"
                ])
                .arg(&source)
                .status()
                .unwrap()
                .success()
        );
        let original = std::fs::read(&source).unwrap();
        let identity = render(
            &source,
            &EditSpec::default(),
            &preserve_mp4(),
            None,
            &CancelToken::default(),
        )
        .unwrap();
        assert_eq!(identity.estimate, Some((original.len() as u64, true)));
        let edit = EditSpec {
            trim_start_ms: 300,
            trim_end_ms: Some(1_700),
            crop: Some(captures_media::CropRect {
                x: 12,
                y: 8,
                width: 120,
                height: 70,
            }),
            output_width: Some(96),
            output_height: Some(56),
            ..Default::default()
        };
        let export = ExportSpec {
            quality: QualityPreset::Tiny,
            ..preserve_mp4()
        };
        let result = render(&source, &edit, &export, Some(900), &CancelToken::default()).unwrap();
        let destination = dir.path().join("full.mp4");
        let actual = MediaToolchain::from_command_names()
            .export(
                &source,
                &destination,
                &edit,
                &export,
                &CancelToken::default(),
                |_| {},
            )
            .unwrap();
        assert_eq!(result.estimate, Some((actual.size_bytes, true)));
        let (before, after) = result.frames.unwrap();
        assert_eq!(before.dimensions(), (96, 56));
        assert_eq!(after.dimensions(), (96, 56));
        assert_ne!(
            before, after,
            "comparison must show the encoded pixels, not the same image twice"
        );
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let cancel = CancelToken::default();
        cancel.cancel();
        assert!(render(&source, &edit, &export, Some(900), &cancel).is_err());
    }
}
