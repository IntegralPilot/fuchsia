// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use audio_decoder_test_lib::cvsd::*;
use audio_roundtrip_test_lib::test_suite::*;
use fidl_fuchsia_media::*;
use fuchsia_async as fasync;
use roundtrip_test_data::*;
use std::fs;
use std::rc::Rc;
use stream_processor_test::*;

const CVSD_TEST_FILE: &str = "/pkg/data/test_s16le64000mono.cvsd";

#[fuchsia::test]
fn cvsd_test_suite() -> Result<()> {
    with_large_stack(|| {
        let output_format = FormatDetails {
            format_details_version_ordinal: Some(1),
            mime_type: Some("audio/pcm".to_string()),
            domain: Some(DomainFormat::Audio(AudioFormat::Uncompressed(
                AudioUncompressedFormat::Pcm(PcmFormat {
                    pcm_mode: AudioPcmMode::Linear,
                    bits_per_sample: 16,
                    frames_per_second: 64000,
                    channel_map: vec![AudioChannelId::Lf],
                }),
            ))),
            ..Default::default()
        };

        // For CVSD, we are comparing the transcoded PCM to the original input PCM data.
        // RMSE difference tolerance is arbitrarily set to 1% of the range of i16.
        let rmse_diff_tolerance = (i16::MAX as f64 - i16::MIN as f64) * 0.01;
        let cvsd_tests = RoundTripTestSuite::new(
            EncoderSettings::Cvsd(CvsdEncoderSettings::default()),
            EncoderStream {
                pcm_data: INPUT_TEST_S16LE64000MONO.to_vec(),
                pcm_framelength: 8,
                frames_per_second: 64000,
                channel_count: 1,
            },
            |data: Vec<u8>| {
                Rc::new(TimestampedStream {
                    source: CvsdStream::from_data(data, 1),
                    timestamps: 0..,
                })
            },
            fs::read(CVSD_TEST_FILE)?,
            TRANSCODED_TEST_S16LE64000MONO.to_vec(),
            Expectations {
                // Input PCM is [i16; 31949]. Since CVSD does 2 byte to 1 bit
                // compression, the resulting data size is 3994 bytes.
                encoded_output_data_size: 3994,
                // CVSD decoder decompresses every 1 bit to 2 bytes. Since
                // the expected encoded output is 3994 bytes, the resulting
                // decompressed, decoded audio would be 63904 bytes.
                decoded_output_data_size: 63904,
                format_details: output_format,
            },
            rmse_diff_tolerance,
        )?;

        fasync::TestExecutor::new().run_singlethreaded(cvsd_tests.run())
    })
}
