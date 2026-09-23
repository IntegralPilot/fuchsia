// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod test_suite;

use crate::test_suite::*;
use fidl_fuchsia_math::{RectU, SizeU};
use fidl_fuchsia_media::*;
use h264_stream::*;
use std::rc::Rc;
use stream_processor_test::*;
use {fidl_fuchsia_images2 as images2, fuchsia_async as fasync};

// Instructions for capturing output of encoder:
// 1. Set the `output_file` field to write the encoded output into "/tmp/".
// 2. Add exec.run_singlethreaded(future::pending()) at the end of the test so it doesn't exit.
// 3. Use `ffx component copy` to copy the file to the host.

#[fuchsia::test]
fn h264_stream_output_generated() -> Result<()> {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 240;
    let mut exec = fasync::TestExecutor::new();

    let test_case = H264EncoderTestCase {
        input_format: images2::ImageFormat {
            pixel_format: Some(images2::PixelFormat::Nv12),
            color_space: Some(images2::ColorSpace::Rec601Pal),
            size: Some(SizeU { width: WIDTH, height: HEIGHT }),
            display_rect: Some(RectU { x: 0, y: 0, width: WIDTH, height: HEIGHT }),
            bytes_per_row: Some(WIDTH),
            ..Default::default()
        },
        num_frames: 6,
        settings: Rc::new(move || -> EncoderSettings {
            EncoderSettings::H264(H264EncoderSettings {
                bit_rate: Some(2000000),
                frame_rate: Some(30),
                gop_size: Some(2),
                ..Default::default()
            })
        }),
        expected_nals: Some(vec![
            H264NalKind::SPS,
            H264NalKind::PPS,
            H264NalKind::IDR,
            H264NalKind::NonIDR,
            H264NalKind::NonIDR,
            H264NalKind::SPS,
            H264NalKind::PPS,
            H264NalKind::IDR,
            H264NalKind::NonIDR,
            H264NalKind::NonIDR,
        ]),
        decode_output: true,
        normalized_sad_threshold: Some(2.0),
        output_file: None,
    };
    exec.run_singlethreaded(test_case.run())?;
    Ok(())
}
