// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use audio_encoder_test_lib::pcm_audio::*;
use fidl_fuchsia_media::*;
use log::info;
use std::rc::Rc;
use stream_processor_decoder_factory::*;
use stream_processor_encoder_factory::*;
use stream_processor_test::*;

#[derive(Clone, Debug)]
pub struct EncoderStream {
    pub pcm_data: Vec<i16>,
    /// The number of PCM input frames per encoded frame.
    pub pcm_framelength: usize,
    pub frames_per_second: u32,
    pub channel_count: usize,
}

pub struct Expectations {
    pub encoded_output_data_size: usize,
    pub decoded_output_data_size: usize,
    pub format_details: FormatDetails,
}

#[allow(unused)]
pub struct RoundTripTestSuite {
    /// Encoder settings.
    // This is a function because FIDL unions are not Copy or Clone.
    encoder_settings: EncoderSettings,
    encoder_stream: EncoderStream,
    decoder_stream: fn(Vec<u8>) -> Rc<dyn ElementaryStream>,

    encoded_golden: Vec<u8>,

    expectations: Expectations,
    target_rmse: f64,
    rmse_diff_tolerance: f64,
}

/// Consumes all the packets in the output with preserved order to return
/// raw data bytes.
fn output_packet_data(output: Vec<Output>) -> Vec<u8> {
    output
        .into_iter()
        .filter_map(|output| match output {
            Output::Packet(packet) => Some(packet),
            _ => None,
        })
        .flat_map(|packet| packet.data)
        .collect()
}

#[allow(unused)]
impl RoundTripTestSuite {
    pub fn new(
        encoder_settings: EncoderSettings,
        encoder_stream: EncoderStream,
        decoder_stream: fn(Vec<u8>) -> Rc<dyn ElementaryStream>,
        encoded_golden: Vec<u8>,
        decoded_golden: Vec<i16>,
        expectations: Expectations,
        rmse_diff_tolerance: f64,
    ) -> Result<Self> {
        let target_rmse = calculate_rmse(
            encoder_stream.pcm_data.as_slice(),
            decoded_golden.as_slice(),
            encoder_stream.frames_per_second,
        )?;
        info!(
            "Target RMSE calculated from input PCM and golden transcoded PCM is: {target_rmse}. RMSE difference tolerance is: {rmse_diff_tolerance}"
        );
        Ok(Self {
            encoder_settings,
            encoder_stream,
            decoder_stream,
            encoded_golden,
            expectations,
            target_rmse,
            rmse_diff_tolerance,
        })
    }

    pub async fn run(self) -> Result<()> {
        self.test_roundtrip().await?;
        self.test_oneway().await
    }

    /// Using the codec under test, transcode (encode and then decode) the input
    /// PCM audio and compare its RMSE against the target RMSE.
    async fn test_roundtrip(&self) -> Result<()> {
        let encoded_output = self.run_through_encoder().await?;
        let encoded_data = output_packet_data(encoded_output);
        self.run_through_decoder(encoded_data).await
    }

    /// Using the codec under test, decode the golden encoded data to ensure
    /// that the codec works according to the well known spec. The
    /// RMSE against the target RMSE to ensure that the decoded PCM has similar
    /// differences as the golden data.
    async fn test_oneway(mut self) -> Result<()> {
        // Since we are consuming self, replace self.encoded_golden so we don't
        // need to clone it.
        let encoded_data = std::mem::replace(&mut self.encoded_golden, vec![]);
        self.run_through_decoder(encoded_data).await
    }

    async fn run_through_encoder(&self) -> Result<TestCaseOutputs> {
        let easy_framelength = self.encoder_stream.pcm_framelength;
        let pcm_audio = PcmAudio::create_from_data(
            PcmFormat {
                pcm_mode: AudioPcmMode::Linear,
                bits_per_sample: 16,
                frames_per_second: self.encoder_stream.frames_per_second,
                channel_map: vec![AudioChannelId::Cf],
            },
            self.encoder_stream.pcm_data.as_slice(),
        );
        let settings = self.encoder_settings.clone();
        let case = TestCase {
            name: "Audio encoder test",
            stream: Rc::new(PcmAudioStream {
                pcm_audio,
                encoder_settings: settings,
                frames_per_packet: (0..).map(move |_| easy_framelength),
                timebase: None,
            }),
            validators: vec![
                Rc::new(TerminatesWithValidator {
                    expected_terminal_output: Output::Eos { stream_lifetime_ordinal: 1 },
                }),
                Rc::new(OutputDataSizeValidator {
                    expected_output_data_size: self.expectations.encoded_output_data_size,
                }),
            ],
            stream_options: Some(StreamOptions {
                queue_format_details: false,
                ..StreamOptions::default()
            }),
        };
        let spec = TestSpec {
            cases: vec![case],
            relation: CaseRelation::Serial,
            stream_processor_factory: Rc::new(EncoderFactory),
        };
        let mut output_data =
            spec.run().await?.expect("Output data should be returned from serial test");
        assert_eq!(output_data.len(), 1);
        Ok(output_data.swap_remove(0))
    }

    async fn run_through_decoder(&self, data: Vec<u8>) -> Result<()> {
        let case = TestCase {
            name: "Audio decoder RMSE test",
            stream: (self.decoder_stream)(data),
            validators: vec![
                Rc::new(TerminatesWithValidator {
                    expected_terminal_output: Output::Eos { stream_lifetime_ordinal: 1 },
                }),
                Rc::new(OutputDataSizeValidator {
                    expected_output_data_size: self.expectations.decoded_output_data_size,
                }),
                Rc::new(FormatValidator {
                    expected_format: self.expectations.format_details.clone(),
                }),
                Rc::new(RmseValidator::<i16> {
                    output_file: None,
                    expected_data: self.encoder_stream.pcm_data.clone(),
                    expected_rmse: self.target_rmse,
                    rmse_diff_tolerance: self.rmse_diff_tolerance,
                    data_len_diff_tolerance: self.encoder_stream.frames_per_second,
                    output_converter: |data: Vec<u8>| {
                        data.chunks_exact(2)
                            .into_iter()
                            .map(|d| i16::from_ne_bytes([d[0], d[1]]))
                            .collect()
                    },
                }),
            ],
            stream_options: Some(StreamOptions {
                queue_format_details: false,
                ..StreamOptions::default()
            }),
        };
        let spec = TestSpec {
            cases: vec![case],
            relation: CaseRelation::Serial,
            stream_processor_factory: Rc::new(DecoderFactory),
        };

        spec.run().await.map(|_| ())
    }
}
