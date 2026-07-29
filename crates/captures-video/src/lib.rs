#![forbid(unsafe_code)]

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use mp4::{
    AvcConfig, FourCC, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig, TrackType,
};
use openh264::{
    Timestamp,
    encoder::{
        BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod,
        Profile, QpRange, RateControlMode, UsageType, VuiConfig,
    },
    formats::{RgbSliceU8, YUVBuffer},
};
use thiserror::Error;

const MP4_TIMESCALE: u32 = 90_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct H264Mp4Info {
    pub duration_ms: u64,
    pub encoded_frames: u64,
    pub skipped_frames: u64,
}

#[derive(Debug, Error)]
pub enum H264Mp4Error {
    #[error("video dimensions must be non-zero, even, and no larger than 3840 by 2160")]
    InvalidDimensions,
    #[error("video frame rate must be between 1 and 60 FPS")]
    InvalidFrameRate,
    #[error("an RGB frame did not match the configured video dimensions")]
    InvalidFrame,
    #[error("the H.264 encoder did not emit its required SPS and PPS headers")]
    MissingParameterSets,
    #[error("the H.264 encoder produced a malformed NAL unit")]
    MalformedNalUnit,
    #[error("the recording did not contain a complete video frame")]
    EmptyRecording,
    #[error("H.264 encoding failed: {0}")]
    Encoder(String),
    #[error("MP4 writing failed: {0}")]
    Mp4(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

struct PendingSample {
    start_ticks: u64,
    is_sync: bool,
    bytes: mp4::Bytes,
}

pub struct H264Mp4Writer {
    output_path: PathBuf,
    width: u32,
    height: u32,
    frame_rate: u16,
    encoder: Encoder,
    yuv: YUVBuffer,
    writer: Option<Mp4Writer<File>>,
    pending: Option<PendingSample>,
    encoded_frames: u64,
    skipped_frames: u64,
}

impl H264Mp4Writer {
    pub fn create(
        output_path: &Path,
        width: u32,
        height: u32,
        frame_rate: u16,
        bitrate_bps: u32,
    ) -> Result<Self, H264Mp4Error> {
        if width == 0
            || height == 0
            || !width.is_multiple_of(2)
            || !height.is_multiple_of(2)
            || !((width <= 3_840 && height <= 2_160) || (width <= 2_160 && height <= 3_840))
        {
            return Err(H264Mp4Error::InvalidDimensions);
        }
        if !(1..=60).contains(&frame_rate) {
            return Err(H264Mp4Error::InvalidFrameRate);
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::remove_file(output_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .max_frame_rate(FrameRate::from_hz(f32::from(frame_rate)))
            .bitrate(BitRate::from_bps(bitrate_bps.max(250_000)))
            .rate_control_mode(RateControlMode::Bitrate)
            .skip_frames(true)
            .complexity(Complexity::Low)
            .profile(Profile::Main)
            .qp(QpRange::new(10, 42))
            .adaptive_quantization(false)
            .background_detection(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(u32::from(frame_rate) * 2))
            .vui(VuiConfig::bt709_full());
        let encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
            .map_err(|error| H264Mp4Error::Encoder(error.to_string()))?;
        let yuv = YUVBuffer::new(width as usize, height as usize);

        Ok(Self {
            output_path: output_path.to_path_buf(),
            width,
            height,
            frame_rate,
            encoder,
            yuv,
            writer: None,
            pending: None,
            encoded_frames: 0,
            skipped_frames: 0,
        })
    }

    pub fn encode_rgb(&mut self, rgb: &[u8], timestamp_ms: u64) -> Result<bool, H264Mp4Error> {
        let expected = usize::try_from(self.width)
            .ok()
            .and_then(|width| {
                usize::try_from(self.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or(H264Mp4Error::InvalidFrame)?;
        if rgb.len() != expected {
            return Err(H264Mp4Error::InvalidFrame);
        }

        self.yuv.read_rgb8(RgbSliceU8::new(
            rgb,
            (self.width as usize, self.height as usize),
        ));
        let bitstream = self
            .encoder
            .encode_at(&self.yuv, Timestamp::from_millis(timestamp_ms))
            .map_err(|error| H264Mp4Error::Encoder(error.to_string()))?;
        let frame_type = bitstream.frame_type();
        if matches!(frame_type, FrameType::Skip | FrameType::Invalid) {
            self.skipped_frames = self.skipped_frames.saturating_add(1);
            return Ok(false);
        }

        let mut sequence_parameter_set = None;
        let mut picture_parameter_set = None;
        let mut sample_bytes = Vec::new();
        for layer_index in 0..bitstream.num_layers() {
            let Some(layer) = bitstream.layer(layer_index) else {
                continue;
            };
            for nal_index in 0..layer.nal_count() {
                let nal = layer
                    .nal_unit(nal_index)
                    .ok_or(H264Mp4Error::MalformedNalUnit)?;
                let payload = strip_annex_b_start_code(nal)?;
                match payload[0] & 0x1f {
                    7 => sequence_parameter_set = Some(payload.to_vec()),
                    8 => picture_parameter_set = Some(payload.to_vec()),
                    _ => {
                        let length = u32::try_from(payload.len())
                            .map_err(|_| H264Mp4Error::MalformedNalUnit)?;
                        sample_bytes.extend_from_slice(&length.to_be_bytes());
                        sample_bytes.extend_from_slice(payload);
                    }
                }
            }
        }
        if sample_bytes.is_empty() {
            self.skipped_frames = self.skipped_frames.saturating_add(1);
            return Ok(false);
        }

        if self.writer.is_none() {
            let sequence_parameter_set =
                sequence_parameter_set.ok_or(H264Mp4Error::MissingParameterSets)?;
            let picture_parameter_set =
                picture_parameter_set.ok_or(H264Mp4Error::MissingParameterSets)?;
            self.writer = Some(create_mp4_writer(
                &self.output_path,
                self.width,
                self.height,
                sequence_parameter_set,
                picture_parameter_set,
            )?);
        }

        let start_ticks = milliseconds_to_ticks(timestamp_ms);
        if let Some(pending) = self.pending.take() {
            let duration = sample_duration(pending.start_ticks, start_ticks, self.frame_rate);
            self.write_pending(pending, duration)?;
        }
        self.pending = Some(PendingSample {
            start_ticks,
            is_sync: matches!(frame_type, FrameType::IDR | FrameType::I),
            bytes: sample_bytes.into(),
        });
        self.encoded_frames = self.encoded_frames.saturating_add(1);
        Ok(true)
    }

    pub fn finish(mut self, end_timestamp_ms: u64) -> Result<H264Mp4Info, H264Mp4Error> {
        let Some(pending) = self.pending.take() else {
            let _ = fs::remove_file(&self.output_path);
            return Err(H264Mp4Error::EmptyRecording);
        };
        let end_ticks = milliseconds_to_ticks(end_timestamp_ms);
        let duration = sample_duration(pending.start_ticks, end_ticks, self.frame_rate);
        self.write_pending(pending, duration)?;
        let mut writer = self.writer.take().ok_or(H264Mp4Error::EmptyRecording)?;
        writer
            .write_end()
            .map_err(|error| H264Mp4Error::Mp4(error.to_string()))?;
        writer.into_writer().flush()?;
        Ok(H264Mp4Info {
            duration_ms: end_timestamp_ms.max(1),
            encoded_frames: self.encoded_frames,
            skipped_frames: self.skipped_frames,
        })
    }

    fn write_pending(&mut self, pending: PendingSample, duration: u32) -> Result<(), H264Mp4Error> {
        let sample = Mp4Sample {
            start_time: pending.start_ticks,
            duration,
            rendering_offset: 0,
            is_sync: pending.is_sync,
            bytes: pending.bytes,
        };
        self.writer
            .as_mut()
            .ok_or(H264Mp4Error::EmptyRecording)?
            .write_sample(1, &sample)
            .map_err(|error| H264Mp4Error::Mp4(error.to_string()))
    }
}

fn create_mp4_writer(
    output_path: &Path,
    width: u32,
    height: u32,
    sequence_parameter_set: Vec<u8>,
    picture_parameter_set: Vec<u8>,
) -> Result<Mp4Writer<File>, H264Mp4Error> {
    let file = File::create(output_path)?;
    let config = Mp4Config {
        major_brand: FourCC::from(*b"isom"),
        minor_version: 512,
        compatible_brands: vec![
            FourCC::from(*b"isom"),
            FourCC::from(*b"iso2"),
            FourCC::from(*b"avc1"),
            FourCC::from(*b"mp41"),
        ],
        timescale: MP4_TIMESCALE,
    };
    let mut writer = Mp4Writer::write_start(file, &config)
        .map_err(|error| H264Mp4Error::Mp4(error.to_string()))?;
    writer
        .add_track(&TrackConfig {
            track_type: TrackType::Video,
            timescale: MP4_TIMESCALE,
            language: "und".to_owned(),
            media_conf: MediaConfig::AvcConfig(AvcConfig {
                width: u16::try_from(width).map_err(|_| H264Mp4Error::InvalidDimensions)?,
                height: u16::try_from(height).map_err(|_| H264Mp4Error::InvalidDimensions)?,
                seq_param_set: sequence_parameter_set,
                pic_param_set: picture_parameter_set,
            }),
        })
        .map_err(|error| H264Mp4Error::Mp4(error.to_string()))?;
    Ok(writer)
}

fn strip_annex_b_start_code(nal: &[u8]) -> Result<&[u8], H264Mp4Error> {
    let mut zeroes = 0;
    for (index, byte) in nal.iter().copied().enumerate() {
        match byte {
            0 => zeroes += 1,
            1 if zeroes >= 2 => {
                let payload = nal
                    .get(index + 1..)
                    .filter(|payload| !payload.is_empty())
                    .ok_or(H264Mp4Error::MalformedNalUnit)?;
                return Ok(payload);
            }
            _ => zeroes = 0,
        }
    }
    Err(H264Mp4Error::MalformedNalUnit)
}

fn milliseconds_to_ticks(milliseconds: u64) -> u64 {
    milliseconds.saturating_mul(u64::from(MP4_TIMESCALE)) / 1_000
}

fn sample_duration(start_ticks: u64, end_ticks: u64, frame_rate: u16) -> u32 {
    let minimum = u64::from(MP4_TIMESCALE) / u64::from(frame_rate);
    u32::try_from(end_ticks.saturating_sub(start_ticks).max(minimum)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use mp4::{MediaType, Mp4Reader};
    use tempfile::tempdir;

    use super::{H264Mp4Writer, strip_annex_b_start_code};

    #[test]
    fn strips_three_and_four_byte_annex_b_prefixes() {
        assert_eq!(
            strip_annex_b_start_code(&[0, 0, 1, 0x67, 1]).expect("three-byte prefix"),
            &[0x67, 1]
        );
        assert_eq!(
            strip_annex_b_start_code(&[0, 0, 0, 1, 0x68, 2]).expect("four-byte prefix"),
            &[0x68, 2]
        );
    }

    #[test]
    fn writes_a_playable_h264_mp4_track() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("sample.mp4");
        let mut writer = H264Mp4Writer::create(&path, 64, 64, 30, 500_000).expect("writer starts");
        for frame_index in 0..6_u64 {
            let mut rgb = vec![0_u8; 64 * 64 * 3];
            for pixel in rgb.chunks_exact_mut(3) {
                pixel[0] = u8::try_from(frame_index * 24).unwrap_or(u8::MAX);
                pixel[1] = 80;
                pixel[2] = 160;
            }
            writer
                .encode_rgb(&rgb, frame_index * 33)
                .expect("frame encodes");
        }
        let info = writer.finish(200).expect("writer finishes");
        assert!(info.encoded_frames > 0);
        assert!(path.metadata().expect("recording metadata").len() > 0);

        let file = File::open(path).expect("recording opens");
        let size = file.metadata().expect("recording metadata").len();
        let reader = Mp4Reader::read_header(file, size).expect("MP4 header parses");
        let track = reader.tracks().values().next().expect("video track");
        assert_eq!(track.media_type().expect("media type"), MediaType::H264);
        assert_eq!((track.width(), track.height()), (64, 64));
        assert!(track.sample_count() > 0);
    }
}
