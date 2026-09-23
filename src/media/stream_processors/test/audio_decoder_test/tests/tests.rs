// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use audio_decoder_test_lib::cvsd::*;
use audio_decoder_test_lib::lc3::*;
use audio_decoder_test_lib::sbc::*;
use audio_decoder_test_lib::test_suite::*;

use decoder_test_data::*;
use fidl_fuchsia_media::*;
use fuchsia_async as fasync;
use fuchsia_bluetooth::assigned_numbers::ltv::*;
use std::rc::Rc;
use stream_processor_test::*;

const SBC_TEST_FILE: &str = "/pkg/data/s16le44100mono.sbc";

// INSTRUCTIONS FOR ADDING HASH TESTS
//
// 1. If adding a new encoded test file, check it into the `test_data` directory. It should only be
//    a few thousand PCM frames decoded.
// 2. Set the `output_file` field to write the decoded output into `/tmp/`. You can copy it to the
//    host with `fx scp [$(fx netaddr --fuchsia)]:/tmp/ ... `
// 3. Decode the test encoded stream with another decoder (for sbc, use sbcdec or
//    ffmpeg; for aac use faac)
// 4. Verify the output
//      a. If the codec should produce the exact same bytes for the same settings, `diff` the two
//         files.
//      b. If the codec is permitted to produce different encoded bytes for the same settings, do a
//         similarity check:
//           b1. Decode both the reference and our encoded stream (sbcdec, faad, etc)
//           b2. Import both tracks into Audacity
//           b3. Apply Effect > Invert to one track
//           b4. Select both tracks and Tracks > Mix > Mix and Render to New Track
//           b5. On the resulting track use Effect > Amplify and observe the new peak amplitude
// 5. If all looks good, commit the hash.

#[fuchsia::test]
fn sbc_decode() -> Result<()> {
    let output_format = FormatDetails {
        format_details_version_ordinal: Some(1),
        mime_type: Some("audio/pcm".to_string()),
        domain: Some(DomainFormat::Audio(AudioFormat::Uncompressed(AudioUncompressedFormat::Pcm(
            PcmFormat {
                pcm_mode: AudioPcmMode::Linear,
                bits_per_sample: 16,
                frames_per_second: 44100,
                channel_map: vec![AudioChannelId::Lf],
            },
        )))),
        ..Default::default()
    };

    let stream = Rc::new(TimestampedStream {
        source: SbcStream::from_file(
            SBC_TEST_FILE,
            /* codec_info */ &[0x82, 0x00, 0x00, 0x00],
            /* chunk_frames */ 1,
        )?,
        timestamps: 0..,
    });

    // One output packet per input packet, the decoder queues an output packet whenever an input packet is
    // exhausted.
    let sbc_tests = AudioDecoderTestCase {
        output_tests: vec![AudioDecoderOutputTest {
            output_file: None,
            stream: stream,
            expected_output_size: OutputSize::PacketCount(23),
            expected_digests: Some(vec![ExpectedDigest::new(
                "Pcm: 44.1kHz/16bit/Mono",
                "ff2e7afea51217886d3df15b9a623b4e49c9bd9bd79c58ac01bc94c5511e08d6",
            )]),
            expected_output_format: output_format,
        }],
    };

    fasync::TestExecutor::new().run_singlethreaded(sbc_tests.run())
}

#[fuchsia::test]
fn sbc_decode_large_input_chunk() -> Result<()> {
    let output_format2 = FormatDetails {
        format_details_version_ordinal: Some(1),
        mime_type: Some("audio/pcm".to_string()),
        domain: Some(DomainFormat::Audio(AudioFormat::Uncompressed(AudioUncompressedFormat::Pcm(
            PcmFormat {
                pcm_mode: AudioPcmMode::Linear,
                bits_per_sample: 16,
                frames_per_second: 44100,
                channel_map: vec![AudioChannelId::Lf],
            },
        )))),
        ..Default::default()
    };

    let large_input_chunk_stream = Rc::new(TimestampedStream {
        source: SbcStream::from_file(
            SBC_TEST_FILE,
            /* codec_info */ &[0x82, 0x00, 0x00, 0x00],
            /* chunk_frames */ 23,
        )?,
        timestamps: 0..,
    });

    // Output space is large (min 10k) so only 2 output frames are needed (this is good) -
    // decoder will continue filling an output frame as long as there is space
    let sbc_tests = AudioDecoderTestCase {
        output_tests: vec![AudioDecoderOutputTest {
            output_file: None,
            stream: large_input_chunk_stream,
            expected_output_size: OutputSize::PacketCount(1),
            expected_digests: Some(vec![ExpectedDigest::new(
                "Large chunk Pcm: 44.1kHz/16bit/Mono",
                "ff2e7afea51217886d3df15b9a623b4e49c9bd9bd79c58ac01bc94c5511e08d6",
            )]),
            expected_output_format: output_format2,
        }],
    };

    fasync::TestExecutor::new().run_singlethreaded(sbc_tests.run())
}

#[fuchsia::test]
fn cvsd_simple_decode() -> Result<()> {
    let output_format = FormatDetails {
        format_details_version_ordinal: Some(1),
        mime_type: Some("audio/pcm".to_string()),
        domain: Some(DomainFormat::Audio(AudioFormat::Uncompressed(AudioUncompressedFormat::Pcm(
            PcmFormat {
                pcm_mode: AudioPcmMode::Linear,
                bits_per_sample: 16,
                frames_per_second: 64000,
                channel_map: vec![AudioChannelId::Lf],
            },
        )))),
        ..Default::default()
    };

    let cvsd_tests = AudioDecoderTestCase {
        output_tests: vec![AudioDecoderOutputTest {
            output_file: None,
            stream: Rc::new(TimestampedStream {
                source: CvsdStream::from_data(vec![0b01010101], 1),
                timestamps: 0..,
            }),
            // Total number of expected decoded output bytes is 16.
            // Since the minimum output buffer size is 16 bytes, all the input
            // should have been decoded in 1 output packet.
            expected_output_size: OutputSize::PacketCount(1),
            expected_digests: Some(vec![ExpectedDigest::new_from_raw(
                "Simple test case",
                // Equivalent to eight int16 elements [10, 0, 9, -1, 9, -1, 9, -1].
                vec![10, 0, 0, 0, 9, 0, 255, 255, 9, 0, 255, 255, 9, 0, 255, 255],
            )]),
            expected_output_format: output_format,
        }],
    };

    fasync::TestExecutor::new().run_singlethreaded(cvsd_tests.run())
}

#[test]
fn lc3_simple_decode() -> Result<()> {
    const BITS_PER_SAMPLE: u32 = 16;
    const FRAMES_PER_SECOND: u32 = 32000;
    const FRAME_SIZE: u32 = 240;
    const NBYTES: usize = 58;

    let oob_bytes = CodecSpecificConfigLTV {
        sampling_frequency: Some(SamplingFrequency::F32000Hz),
        frame_duration: Some(FrameDuration::D7p5Ms),
        audio_channel_alloc: Some(AudioLocation::FRONT_LEFT),
        octets_per_codec_frame: Some(58),
        ..Default::default()
    }
    .to_be_bytes();

    let output_format = FormatDetails {
        format_details_version_ordinal: Some(1),
        mime_type: Some("audio/pcm".to_string()),
        domain: Some(DomainFormat::Audio(AudioFormat::Uncompressed(AudioUncompressedFormat::Pcm(
            PcmFormat {
                pcm_mode: AudioPcmMode::Linear,
                bits_per_sample: BITS_PER_SAMPLE,
                frames_per_second: FRAMES_PER_SECOND,
                channel_map: vec![AudioChannelId::Lf],
            },
        )))),
        ..FormatDetails::default()
    };

    let lc3_data: Vec<u8> = LC3_TEST_S16LE32000MONO.to_vec();

    let lc3_tests = AudioDecoderTestCase {
        output_tests: vec![AudioDecoderOutputTest {
            output_file: None,
            stream: Rc::new(TimestampedStream {
                source: Lc3Stream::from_data(lc3_data, oob_bytes, NBYTES),
                timestamps: 0..,
            }),
            // Frame size for output PCM is 240 samples. There are 48 output frames
            // worth of input and each sample is 2 bytes long.
            expected_output_size: OutputSize::RawBytesCount(
                (FRAME_SIZE * 48 * 2).try_into().unwrap(),
            ),
            expected_digests: None,
            expected_output_format: output_format,
        }],
    };

    fasync::TestExecutor::new().run_singlethreaded(lc3_tests.run())
}
