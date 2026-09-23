// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use async_trait::async_trait;
use fidl_fuchsia_images2 as images2;
use fidl_fuchsia_media::*;
use fuchsia_image_format::sysmem1_image_format_from_images2_image_format;
use std::rc::Rc;
use stream_processor_test::*;

#[derive(Debug)]
pub struct VideoFrame {
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    format: images2::ImageFormat,
    data: Vec<u8>,
}

/// Container for raw video frame and associated format
impl VideoFrame {
    /// Generates a frame with specified `format`.
    /// `step` is for diagonally shifting the checkerboard pattern.
    pub fn create(format: images2::ImageFormat, step: usize) -> Self {
        // For 4:2:0 YUV, the UV data is 1/2 the size of the Y data,
        // so the size of the frame is 3/2 the size of the Y plane.
        let width = *format.bytes_per_row.as_ref().unwrap() as usize;
        let height = format.size.as_ref().unwrap().height as usize;
        let frame_size = width * height * 3usize / 2usize;
        let mut data = vec![128; frame_size];

        // generate checkerboard
        const NUM_BLOCKS: usize = 8usize;
        let block_size = width / NUM_BLOCKS;
        let mut y_on = true;
        let mut x_on = true;
        for y in 0..height {
            if y % block_size == 0 {
                y_on = !y_on;
                x_on = y_on;
            }
            for x in 0..width {
                if x % block_size == 0 {
                    x_on = !x_on;
                }
                let luma = if x_on { 255 } else { 0 };
                let y_s = (y + step) % height;
                let x_s = (x + step) % width;
                data[y_s * width + x_s] = luma;
            }
        }

        Self { format, data }
    }
}

/// Generates timestamps according to a timebase and rate of playback of uncompressed video.
///
/// Since the rate is constant, this can also be used to extrapolate timestamps.
pub struct TimestampGenerator {
    pub timebase: u64,
    pub frames_per_second: usize,
}

impl TimestampGenerator {
    pub fn timestamp_at(&self, input_index: usize) -> u64 {
        let fps = self.frames_per_second as u64;
        (input_index as u64) * self.timebase / fps
    }
}

/// Validates that timestamps match the expected output from the specified generator
pub struct TimestampValidator {
    pub generator: Option<TimestampGenerator>,
}

#[async_trait(?Send)]
impl OutputValidator for TimestampValidator {
    async fn validate(&self, output: &[Output]) -> Result<()> {
        let packets = output_packets(output);
        for (pos, packet) in packets.enumerate() {
            if packet.packet.timestamp_ish.is_none() {
                return Err(format_err!("Missing timestamp"));
            }

            if let Some(generator) = &self.generator {
                let current_ts = packet.packet.timestamp_ish.unwrap();
                let expected_ts = generator.timestamp_at(pos);

                if current_ts != expected_ts {
                    return Err(format_err!(
                        "Unexpected timestamp {} (expected {})",
                        current_ts,
                        expected_ts
                    ));
                }
            }
        }
        Ok(())
    }
}

fn new_video_uncompressed_format(image_format: images2::ImageFormat) -> VideoUncompressedFormat {
    VideoUncompressedFormat {
        image_format: sysmem1_image_format_from_images2_image_format(&image_format).unwrap(),
        fourcc: 0,
        primary_width_pixels: 0,
        primary_height_pixels: 0,
        secondary_width_pixels: 0,
        secondary_height_pixels: 0,
        planar: false,
        swizzled: false,
        primary_line_stride_bytes: 0,
        secondary_line_stride_bytes: 0,
        primary_start_offset: 0,
        secondary_start_offset: 0,
        tertiary_start_offset: 0,
        primary_pixel_stride: 0,
        secondary_pixel_stride: 0,
        primary_display_width_pixels: 0,
        primary_display_height_pixels: 0,
        has_pixel_aspect_ratio: false,
        pixel_aspect_ratio_width: 0,
        pixel_aspect_ratio_height: 0,
    }
}

/// Implements an `ElementaryStream` of raw video frames with the specified `format`, intended to be
/// fed to an encoder StreamProcessor.
pub struct VideoFrameStream {
    pub num_frames: usize,
    pub format: images2::ImageFormat,
    pub encoder_settings: Rc<dyn Fn() -> EncoderSettings>,
    pub frames_per_second: usize,
    pub timebase: Option<u64>,
    pub mime_type: String,
}

impl VideoFrameStream {
    pub fn create(
        format: images2::ImageFormat,
        num_frames: usize,
        encoder_settings: Rc<dyn Fn() -> EncoderSettings>,
        frames_per_second: usize,
        timebase: Option<u64>,
        mime_type: &str,
    ) -> Result<Self> {
        Ok(Self {
            num_frames,
            format,
            encoder_settings,
            frames_per_second,
            timebase,
            mime_type: mime_type.to_string(),
        })
    }

    pub fn timestamp_generator(&self) -> Option<TimestampGenerator> {
        self.timebase.map(|timebase| TimestampGenerator {
            frames_per_second: self.frames_per_second,
            timebase,
        })
    }
}

impl ElementaryStream for VideoFrameStream {
    fn format_details(&self, format_details_version_ordinal: u64) -> FormatDetails {
        FormatDetails {
            domain: Some(DomainFormat::Video(VideoFormat::Uncompressed(
                new_video_uncompressed_format(self.format.clone()),
            ))),
            encoder_settings: Some((self.encoder_settings)()),
            format_details_version_ordinal: Some(format_details_version_ordinal),
            mime_type: Some(self.mime_type.to_string()),
            oob_bytes: None,
            pass_through_parameters: None,
            timebase: self.timebase,
            ..Default::default()
        }
    }

    fn is_access_units(&self) -> bool {
        false
    }

    fn stream<'a>(&'a self) -> Box<dyn Iterator<Item = ElementaryStreamChunk> + 'a> {
        Box::new((0..self.num_frames).map(move |input_index| {
            let frame = VideoFrame::create(self.format.clone(), input_index);
            ElementaryStreamChunk {
                start_access_unit: false,
                known_end_access_unit: false,
                data: frame.data,
                significance: Significance::Video(VideoSignificance::Picture),
                timestamp: self
                    .timestamp_generator()
                    .as_ref()
                    .map(|timestamp_generator| timestamp_generator.timestamp_at(input_index)),
            }
        }))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl_fuchsia_math::{RectU, SizeU};
    use fuchsia_image_format::images2_image_format_from_sysmem_image_format;

    #[derive(Debug, Copy, Clone)]
    struct TestSpec {
        pixel_format: images2::PixelFormat,
        coded_width: usize,
        coded_height: usize,
        display_width: usize,
        display_height: usize,
        bytes_per_row: usize,
    }

    impl Into<VideoUncompressedFormat> for TestSpec {
        fn into(self) -> VideoUncompressedFormat {
            new_video_uncompressed_format(images2::ImageFormat {
                pixel_format: Some(self.pixel_format),
                size: Some(SizeU {
                    width: self.coded_width.try_into().unwrap(),
                    height: self.coded_height.try_into().unwrap(),
                }),
                bytes_per_row: Some(self.bytes_per_row as u32),
                display_rect: Some(RectU {
                    x: 0,
                    y: 0,
                    width: self.display_width as u32,
                    height: self.display_height as u32,
                }),
                color_space: Some(images2::ColorSpace::Rec709),
                ..Default::default()
            })
        }
    }

    #[fuchsia::test]
    fn stream_timestamps() {
        let test_spec = TestSpec {
            pixel_format: images2::PixelFormat::Nv12,
            coded_width: 16,
            coded_height: 16,
            display_height: 12,
            display_width: 16,
            bytes_per_row: 16,
        };

        let format: VideoUncompressedFormat = test_spec.into();
        let stream = VideoFrameStream::create(
            images2_image_format_from_sysmem_image_format(&format.image_format).unwrap(),
            /*num_frames=*/ 2,
            Rc::new(move || -> EncoderSettings {
                EncoderSettings::H264(H264EncoderSettings {
                    bit_rate: Some(200000),
                    frame_rate: Some(60),
                    ..Default::default()
                })
            }),
            /*frames_per_second=*/ 60,
            /*timebase=*/ Some(zx::MonotonicDuration::from_seconds(1).into_nanos() as u64),
            /*mime_type=*/ "video/h264",
        )
        .expect("stream");
        let mut chunks = stream.stream();

        assert_eq!(chunks.next().and_then(|chunk| chunk.timestamp), Some(0));
        assert_eq!(
            chunks.next().and_then(|chunk| chunk.timestamp),
            Some(zx::MonotonicDuration::from_seconds(1).into_nanos() as u64 / 60)
        );
    }

    #[fuchsia::test]
    fn pattern_check() {
        let test_spec = TestSpec {
            pixel_format: images2::PixelFormat::Nv12,
            coded_width: 8,
            coded_height: 8,
            display_height: 8,
            display_width: 8,
            bytes_per_row: 8,
        };

        let format: VideoUncompressedFormat = test_spec.into();
        let frame = VideoFrame::create(
            images2_image_format_from_sysmem_image_format(&format.image_format).unwrap(),
            0,
        );
        assert_eq!(
            frame.data,
            vec![
                255, 0, 255, 0, 255, 0, 255, 0, 0, 255, 0, 255, 0, 255, 0, 255, 255, 0, 255, 0,
                255, 0, 255, 0, 0, 255, 0, 255, 0, 255, 0, 255, 255, 0, 255, 0, 255, 0, 255, 0, 0,
                255, 0, 255, 0, 255, 0, 255, 255, 0, 255, 0, 255, 0, 255, 0, 0, 255, 0, 255, 0,
                255, 0, 255, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
                128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
                128, 128
            ]
        );
    }
}
