// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_media::*;
use fidl_fuchsia_sysmem2::{BufferCollectionConstraints, BufferMemoryConstraints};
use std::rc::Rc;
use stream_processor_decoder_factory::*;
use stream_processor_test::*;

pub struct AudioDecoderTestCase {
    pub output_tests: Vec<AudioDecoderOutputTest>,
}

/// A hash test runs audio through the encoder and checks that all that data emitted when hashed
/// sequentially results in the expected digest. Oob bytes are hashed first.
pub struct AudioDecoderOutputTest {
    /// If provided, the output will also be written to this file. Use this to verify new files
    /// with a decoder before using their digest in tests.
    pub output_file: Option<&'static str>,
    pub stream: Rc<dyn ElementaryStream>,
    pub expected_output_size: OutputSize,
    pub expected_digests: Option<Vec<ExpectedDigest>>,
    pub expected_output_format: FormatDetails,
}

fn test_buffer_collection_constraints() -> BufferCollectionConstraints {
    BufferCollectionConstraints {
        buffer_memory_constraints: Some(BufferMemoryConstraints {
            // Chosen to be larger than most decoder tests requirements, and not particularly
            // an even size of output frames (at 16 bits per sample, an odd number satisfies this)
            min_size_bytes: Some(10001),
            ..Default::default()
        }),
        ..buffer_collection_constraints_default()
    }
}

impl AudioDecoderTestCase {
    pub async fn run(self) -> Result<()> {
        self.test_hashes().await
    }

    async fn test_hashes(self) -> Result<()> {
        let mut cases = vec![];
        for (output_test, stream_lifetime_ordinal) in
            self.output_tests.into_iter().zip(OrdinalPattern::Odd.into_iter())
        {
            let mut validators: Vec<Rc<dyn OutputValidator>> =
                vec![Rc::new(TerminatesWithValidator {
                    expected_terminal_output: Output::Eos { stream_lifetime_ordinal },
                })];
            match output_test.expected_output_size {
                OutputSize::PacketCount(v) => {
                    validators.push(Rc::new(OutputPacketCountValidator {
                        expected_output_packet_count: v,
                    }));
                }
                OutputSize::RawBytesCount(v) => {
                    validators
                        .push(Rc::new(OutputDataSizeValidator { expected_output_data_size: v }));
                }
            };
            validators.push(Rc::new(FormatValidator {
                expected_format: output_test.expected_output_format,
            }));
            if let Some(digests) = output_test.expected_digests {
                validators.push(Rc::new(BytesValidator {
                    output_file: output_test.output_file,
                    expected_digests: digests,
                }));
            }
            cases.push(TestCase {
                name: "Audio decoder output test",
                stream: output_test.stream,
                validators,
                stream_options: Some(StreamOptions {
                    queue_format_details: false,
                    // Set the buffer constraints slightly off-kilter to test fenceposting
                    output_buffer_collection_constraints: Some(test_buffer_collection_constraints()),
                    ..StreamOptions::default()
                }),
            });
        }

        let spec = TestSpec {
            cases,
            relation: CaseRelation::Serial,
            stream_processor_factory: Rc::new(DecoderFactory),
        };

        spec.run().await.map(|_| ())
    }
}
