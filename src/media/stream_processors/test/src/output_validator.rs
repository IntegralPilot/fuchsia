// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::FatalError;
use anyhow::Error;
use async_trait::async_trait;
use fidl_fuchsia_media::{FormatDetails, StreamOutputFormat};
use fidl_table_validation::*;
use fuchsia_stream_processors::*;
use hex::{decode, encode};
use log::info;
use mundane::hash::{Digest, Hasher, Sha256};
use num_traits::PrimInt;
use std::fmt;
use std::io::Write;
use std::rc::Rc;

#[derive(ValidFidlTable, Debug, PartialEq)]
#[fidl_table_src(StreamOutputFormat)]
pub struct ValidStreamOutputFormat {
    pub stream_lifetime_ordinal: u64,
    pub format_details: FormatDetails,
}

/// An output packet from the stream.
#[derive(Debug, PartialEq)]
pub struct OutputPacket {
    pub data: Vec<u8>,
    pub format: Rc<ValidStreamOutputFormat>,
    pub packet: ValidPacket,
}

/// Returns all the packets in the output with preserved order.
pub fn output_packets(output: &[Output]) -> impl Iterator<Item = &OutputPacket> {
    output.iter().filter_map(|output| match output {
        Output::Packet(packet) => Some(packet),
        _ => None,
    })
}

/// Output represents any output from a stream we might want to validate programmatically.
///
/// This may extend to contain not just explicit events but certain stream control behaviors or
/// even errors.
#[derive(Debug, PartialEq)]
pub enum Output {
    Packet(OutputPacket),
    Eos { stream_lifetime_ordinal: u64 },
    CodecChannelClose,
}

/// Checks all output packets, which are provided to the validator in the order in which they
/// were received from the stream processor.
///
/// Failure should be indicated by returning an error, not by panic, so that the full context of
/// the error will be available in the failure output.
#[async_trait(?Send)]
pub trait OutputValidator {
    async fn validate(&self, output: &[Output]) -> Result<(), Error>;
}

/// Validates that the output contains the expected number of packets.
pub struct OutputPacketCountValidator {
    pub expected_output_packet_count: usize,
}

#[async_trait(?Send)]
impl OutputValidator for OutputPacketCountValidator {
    async fn validate(&self, output: &[Output]) -> Result<(), Error> {
        let actual_output_packet_count: usize = output
            .iter()
            .filter(|output| match output {
                Output::Packet(_) => true,
                _ => false,
            })
            .count();

        if actual_output_packet_count != self.expected_output_packet_count {
            return Err(FatalError(format!(
                "actual output packet count: {}; expected output packet count: {}",
                actual_output_packet_count, self.expected_output_packet_count
            ))
            .into());
        }

        Ok(())
    }
}

/// Validates that the output contains the expected number of bytes.
pub struct OutputDataSizeValidator {
    pub expected_output_data_size: usize,
}

#[async_trait(?Send)]
impl OutputValidator for OutputDataSizeValidator {
    async fn validate(&self, output: &[Output]) -> Result<(), Error> {
        let actual_output_data_size: usize = output
            .iter()
            .map(|output| match output {
                Output::Packet(p) => p.data.len(),
                _ => 0,
            })
            .sum();

        if actual_output_data_size != self.expected_output_data_size {
            return Err(FatalError(format!(
                "actual output data size: {}; expected output data size: {}",
                actual_output_data_size, self.expected_output_data_size
            ))
            .into());
        }

        Ok(())
    }
}

/// Validates that a stream terminates with Eos.
pub struct TerminatesWithValidator {
    pub expected_terminal_output: Output,
}

#[async_trait(?Send)]
impl OutputValidator for TerminatesWithValidator {
    async fn validate(&self, output: &[Output]) -> Result<(), Error> {
        let actual_terminal_output = output.last().ok_or_else(|| {
            FatalError(format!(
                "In terminal output: expected {:?}; found: None",
                Some(&self.expected_terminal_output)
            ))
        })?;

        if *actual_terminal_output == self.expected_terminal_output {
            Ok(())
        } else {
            Err(FatalError(format!(
                "In terminal output: expected {:?}; found: {:?}",
                Some(&self.expected_terminal_output),
                actual_terminal_output
            ))
            .into())
        }
    }
}

/// Validates that an output's format matches expected
pub struct FormatValidator {
    pub expected_format: FormatDetails,
}

#[async_trait(?Send)]
impl OutputValidator for FormatValidator {
    async fn validate(&self, output: &[Output]) -> Result<(), Error> {
        let packets: Vec<&OutputPacket> = output_packets(output).collect();
        let format = &packets
            .first()
            .ok_or_else(|| FatalError(String::from("No packets in output")))?
            .format
            .format_details;

        if self.expected_format != *format {
            return Err(FatalError(format!(
                "Expected {:?}; got {:?}",
                self.expected_format, format
            ))
            .into());
        }

        Ok(())
    }
}

/// Validates that an output's data exactly matches an expected hash, including oob_bytes
pub struct BytesValidator {
    pub output_file: Option<&'static str>,
    pub expected_digests: Vec<ExpectedDigest>,
}

impl BytesValidator {
    fn write_and_hash(
        &self,
        mut writer: impl Write,
        oob: &[u8],
        packets: &[&OutputPacket],
    ) -> Result<(), Error> {
        let mut hasher = Sha256::default();

        hasher.update(oob);

        for packet in packets {
            writer.write_all(&packet.data)?;
            hasher.update(&packet.data);
        }
        writer.flush()?;

        let digest = hasher.finish().bytes();

        if let None = self.expected_digests.iter().find(|e| e.bytes == digest) {
            return Err(FatalError(format!(
                "Expected one of {:?}; got {}",
                self.expected_digests,
                encode(digest)
            ))
            .into());
        }

        Ok(())
    }
}

fn output_writer(output_file: Option<&'static str>) -> Result<impl Write, Error> {
    Ok(if let Some(file) = output_file {
        Box::new(std::fs::File::create(file)?) as Box<dyn Write>
    } else {
        Box::new(std::io::sink()) as Box<dyn Write>
    })
}

#[async_trait(?Send)]
impl OutputValidator for BytesValidator {
    async fn validate(&self, output: &[Output]) -> Result<(), Error> {
        let packets: Vec<&OutputPacket> = output_packets(output).collect();
        let oob = packets
            .first()
            .ok_or_else(|| FatalError(String::from("No packets in output")))?
            .format
            .format_details
            .oob_bytes
            .clone()
            .unwrap_or(vec![]);

        self.write_and_hash(output_writer(self.output_file)?, oob.as_slice(), &packets)
    }
}

#[derive(Clone)]
pub struct ExpectedDigest {
    pub label: &'static str,
    pub bytes: <<Sha256 as Hasher>::Digest as Digest>::Bytes,
    pub per_frame_bytes: Option<Vec<<<Sha256 as Hasher>::Digest as Digest>::Bytes>>,
}

impl ExpectedDigest {
    pub fn new(label: &'static str, hex: impl AsRef<[u8]>) -> Self {
        Self {
            label,
            bytes: decode(hex)
                .expect("Decoding static compile-time test hash as valid hex")
                .as_slice()
                .try_into()
                .expect("Taking 32 bytes from compile-time test hash"),
            per_frame_bytes: None,
        }
    }
    pub fn new_with_per_frame_digest(
        label: &'static str,
        hex: impl AsRef<[u8]>,
        per_frame_hexen: Vec<impl AsRef<[u8]>>,
    ) -> Self {
        Self {
            per_frame_bytes: Some(
                per_frame_hexen
                    .into_iter()
                    .map(|per_frame_hex| {
                        decode(per_frame_hex)
                            .expect("Decoding static compile-time test hash as valid hex")
                            .as_slice()
                            .try_into()
                            .expect("Taking 32 bytes from compile-time test hash")
                    })
                    .collect(),
            ),
            ..Self::new(label, hex)
        }
    }

    pub fn new_from_raw(label: &'static str, raw_data: Vec<u8>) -> Self {
        Self {
            label,
            bytes: <Sha256 as Hasher>::hash(raw_data.as_slice()).bytes(),
            per_frame_bytes: None,
        }
    }
}

impl fmt::Display for ExpectedDigest {
    fn fmt(&self, w: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(w, "{:?}", self)
    }
}

impl fmt::Debug for ExpectedDigest {
    fn fmt(&self, w: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(w, "ExpectedDigest {{\n")?;
        write!(w, "\tlabel: {}", self.label)?;
        write!(w, "\tbytes: {}", encode(self.bytes))?;
        write!(w, "}}")
    }
}

/// Validates that the RMSE of output data and the expected data
/// falls within an acceptable range.
#[allow(unused)]
pub struct RmseValidator<T> {
    pub output_file: Option<&'static str>,
    pub expected_data: Vec<T>,
    pub expected_rmse: f64,
    // By how much percentage should we allow the calculated RMSE value to
    // differ from the expected RMSE.
    pub rmse_diff_tolerance: f64,
    pub data_len_diff_tolerance: u32,
    pub output_converter: fn(Vec<u8>) -> Vec<T>,
}

pub fn calculate_rmse<T: PrimInt + std::fmt::Debug>(
    expected_data: &[T],
    actual_data: &[T],
    acceptable_len_diff: u32,
) -> Result<f64, Error> {
    // There could be a slight difference to the length of the expected data
    // and the actual data due to the way some codecs deal with left over data
    // at the end of the stream. This can be caused by minimum block size and
    // how some codecs may choose to pad out the last block or insert a silence
    // data at the start. Ensure the difference in length between expected and
    // actual data is not too much.
    let compare_len = std::cmp::min(expected_data.len(), actual_data.len());
    if std::cmp::max(expected_data.len(), actual_data.len()) - compare_len
        > acceptable_len_diff.try_into().unwrap()
    {
        return Err(FatalError(format!(
            "Expected data (len {}) and the actual data (len {}) have significant length difference and cannot be compared.",
            expected_data.len(), actual_data.len(),
        )).into());
    }
    let expected_data = &expected_data[..compare_len];
    let actual_data = &actual_data[..compare_len];

    let mut rmse = 0.0;
    let mut n = 0;
    for data in std::iter::zip(actual_data.iter(), expected_data.iter()) {
        let b1: f64 = num_traits::cast::cast(*data.0).unwrap();
        let b2: f64 = num_traits::cast::cast(*data.1).unwrap();
        rmse += (b1 - b2).powi(2);
        n += 1;
    }
    Ok((rmse / n as f64).sqrt())
}

impl<T: PrimInt + std::fmt::Debug> RmseValidator<T> {
    fn write_and_calc_rsme(
        &self,
        mut writer: impl Write,
        packets: &[&OutputPacket],
    ) -> Result<(), Error> {
        let mut output_data: Vec<u8> = Vec::new();
        for packet in packets {
            writer.write_all(&packet.data)?;
            packet.data.iter().for_each(|item| output_data.push(*item));
        }

        let actual_data = (self.output_converter)(output_data);

        let rmse = calculate_rmse(
            self.expected_data.as_slice(),
            actual_data.as_slice(),
            self.data_len_diff_tolerance,
        )?;
        info!("RMSE is {}", rmse);
        if (rmse - self.expected_rmse).abs() > self.rmse_diff_tolerance {
            return Err(FatalError(format!(
                "expected rmse: {}; actual rmse: {}; rmse diff tolerance {}",
                self.expected_rmse, rmse, self.rmse_diff_tolerance,
            ))
            .into());
        }
        Ok(())
    }
}

#[async_trait(?Send)]
impl<T: PrimInt + std::fmt::Debug> OutputValidator for RmseValidator<T> {
    async fn validate(&self, output: &[Output]) -> Result<(), Error> {
        let packets: Vec<&OutputPacket> = output_packets(output).collect();
        self.write_and_calc_rsme(output_writer(self.output_file)?, &packets)
    }
}
