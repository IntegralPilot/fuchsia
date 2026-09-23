// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::packet_encoding::{Decodable, Encodable};
use bt_common::{codable_as_bitmask, decodable_enum, Uuid};
use bt_gatt::types::Handle;
use thiserror::Error;

use crate::client::ClientError;
use crate::server::ServerError;
use bt_common::core::ltv::LtValue;
use bt_common::core::CodecId;
use bt_common::generic_audio::metadata_ltv::Metadata;
use bt_gatt::types::Error as BtGattError;

/// Error type
#[derive(Debug, Error)]
pub enum Error {
    #[error("ASCS Server error: {0}")]
    Server(#[from] ServerError),
    #[error("ASCS Client error: {0}")]
    Client(#[from] ClientError),
    #[error("GATT operation error: {0}")]
    Gatt(#[from] BtGattError),
    #[error("Internal error occurred: {0}")]
    Internal(String),
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseCode {
    Success { ase_id: AseId },
    UnsupportedOpcode { opcode_byte: u8 },
    InvalidLength { opcode_byte: u8 },
    InvalidAseId { value: u8 },
    InvalidAseStateMachineTransition { ase_id: AseId },
    InvalidAseDirection { ase_id: AseId },
    UnsupportedAudioCapablities { ase_id: AseId },
    ConfigurationParameterValue { ase_id: AseId, issue: ResponseIssue, reason: ResponseReason },
    Metadata { ase_id: AseId, issue: ResponseIssue, type_value: u8 },
    InsufficientResources { ase_id: AseId },
    UnspecifiedError { ase_id: AseId },
}

impl ResponseCode {
    pub(crate) fn to_code(&self) -> u8 {
        match self {
            ResponseCode::Success { .. } => 0x00,
            ResponseCode::UnsupportedOpcode { .. } => 0x01,
            ResponseCode::InvalidLength { .. } => 0x02,
            ResponseCode::InvalidAseId { .. } => 0x03,
            ResponseCode::InvalidAseStateMachineTransition { .. } => 0x04,
            ResponseCode::InvalidAseDirection { .. } => 0x05,
            ResponseCode::UnsupportedAudioCapablities { .. } => 0x06,
            ResponseCode::ConfigurationParameterValue {
                issue: ResponseIssue::Unsupported, ..
            } => 0x07,
            ResponseCode::ConfigurationParameterValue {
                issue: ResponseIssue::Rejected, ..
            } => 0x08,
            ResponseCode::ConfigurationParameterValue { issue: ResponseIssue::Invalid, .. } => 0x09,
            ResponseCode::Metadata { issue: ResponseIssue::Unsupported, .. } => 0x0A,
            ResponseCode::Metadata { issue: ResponseIssue::Rejected, .. } => 0x0B,
            ResponseCode::Metadata { issue: ResponseIssue::Invalid, .. } => 0x0C,
            ResponseCode::InsufficientResources { .. } => 0x0D,
            ResponseCode::UnspecifiedError { .. } => 0x0E,
        }
    }

    pub fn invalid_length() -> Self {
        Self::invalid_length_opcode(0)
    }

    pub fn invalid_length_opcode(opcode_byte: u8) -> Self {
        Self::InvalidLength { opcode_byte }
    }

    pub fn unsupported_opcode(opcode_byte: u8) -> Self {
        Self::UnsupportedOpcode { opcode_byte }
    }

    pub(crate) fn is_invalid_length(&self) -> bool {
        matches!(self, Self::InvalidLength { .. })
    }

    fn reason_byte(&self) -> u8 {
        match self {
            ResponseCode::ConfigurationParameterValue { reason, .. } => (*reason).into(),
            ResponseCode::Metadata { type_value, .. } => *type_value,
            _ => 0x00,
        }
    }

    /// Get the ASE_ID value for this ResponseCode.  This is included in the
    /// control point notification in response to an operation.  For
    /// UnsupportedOpcode and InvaildLength, the ASE_ID is defined by the
    /// spec to be 0x00.
    pub(crate) fn ase_id_value(&self) -> u8 {
        match self {
            ResponseCode::UnsupportedOpcode { .. } | ResponseCode::InvalidLength { .. } => 0x00,
            ResponseCode::InvalidAseId { value } => *value,
            ResponseCode::Success { ase_id }
            | ResponseCode::InvalidAseStateMachineTransition { ase_id }
            | ResponseCode::InvalidAseDirection { ase_id }
            | ResponseCode::UnsupportedAudioCapablities { ase_id }
            | ResponseCode::ConfigurationParameterValue { ase_id, .. }
            | ResponseCode::Metadata { ase_id, .. }
            | ResponseCode::InsufficientResources { ase_id }
            | ResponseCode::UnspecifiedError { ase_id } => (*ase_id).into(),
        }
    }

    pub(crate) fn error_notify_value(&self) -> Vec<u8> {
        match self {
            Self::UnsupportedOpcode { opcode_byte } | Self::InvalidLength { opcode_byte } => {
                [*opcode_byte, 0xFFu8].into_iter().chain(self.notify_value()).collect()
            }
            _ => vec![],
        }
    }

    pub(crate) fn notify_value(&self) -> Vec<u8> {
        [self.ase_id_value(), self.to_code(), self.reason_byte()].into()
    }

    pub fn ase_id(&self) -> AseId {
        AseId(self.ase_id_value())
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    pub fn decode_response(
        buf: &[u8],
        opcode: u8,
    ) -> Result<Self, bt_common::packet_encoding::Error> {
        if buf.len() < 3 {
            return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
        }
        let ase_id_val = buf[0];
        let code = buf[1];
        let reason = buf[2];
        let ase_id = AseId(ase_id_val);

        match code {
            0x00 => Ok(Self::Success { ase_id }),
            0x01 => {
                if ase_id_val != 0 {
                    return Err(bt_common::packet_encoding::Error::OutOfRange);
                }
                Ok(Self::UnsupportedOpcode { opcode_byte: opcode })
            }
            0x02 => {
                if ase_id_val != 0 {
                    return Err(bt_common::packet_encoding::Error::OutOfRange);
                }
                Ok(Self::InvalidLength { opcode_byte: opcode })
            }
            0x03 => Ok(Self::InvalidAseId { value: ase_id_val }),
            0x04 => Ok(Self::InvalidAseStateMachineTransition { ase_id }),
            0x05 => Ok(Self::InvalidAseDirection { ase_id }),
            0x06 => Ok(Self::UnsupportedAudioCapablities { ase_id }),
            0x07 => {
                let reason = ResponseReason::try_from(reason)
                    .map_err(|_| bt_common::packet_encoding::Error::OutOfRange)?;
                Ok(Self::ConfigurationParameterValue {
                    ase_id,
                    issue: ResponseIssue::Unsupported,
                    reason,
                })
            }
            0x08 => {
                let reason = ResponseReason::try_from(reason)
                    .map_err(|_| bt_common::packet_encoding::Error::OutOfRange)?;
                Ok(Self::ConfigurationParameterValue {
                    ase_id,
                    issue: ResponseIssue::Rejected,
                    reason,
                })
            }
            0x09 => {
                let reason = ResponseReason::try_from(reason)
                    .map_err(|_| bt_common::packet_encoding::Error::OutOfRange)?;
                Ok(Self::ConfigurationParameterValue {
                    ase_id,
                    issue: ResponseIssue::Invalid,
                    reason,
                })
            }
            0x0A => {
                Ok(Self::Metadata { ase_id, issue: ResponseIssue::Unsupported, type_value: reason })
            }
            0x0B => {
                Ok(Self::Metadata { ase_id, issue: ResponseIssue::Rejected, type_value: reason })
            }
            0x0C => {
                Ok(Self::Metadata { ase_id, issue: ResponseIssue::Invalid, type_value: reason })
            }
            0x0D => Ok(Self::InsufficientResources { ase_id }),
            0x0E => Ok(Self::UnspecifiedError { ase_id }),
            _ => Err(bt_common::packet_encoding::Error::OutOfRange),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseIssue {
    Unsupported,
    Rejected,
    Invalid,
}

decodable_enum! {
#[non_exhaustive]
pub enum ResponseReason<u8, bt_common::packet_encoding::Error, OutOfRange> {
    CodecId = 0x01,
    CodecSpecificConfiguration = 0x02,
    SduInterval = 0x03,
    Framing = 0x04,
    Phy = 0x05,
    MaximumSduSize = 0x06,
    RetransmissionNumber = 0x07,
    MaxTransportLatency = 0x08,
    PresentationDelay = 0x09,
    InvalidAseCisMapping = 0x0A,
}
}

decodable_enum! {

#[derive(Default)]
pub enum AseState<u8, bt_common::packet_encoding::Error, OutOfRange> {
    #[default]
    Idle = 0x00,
    CodecConfigured = 0x01,
    QosConfigured = 0x02,
    Enabling = 0x03,
    Streaming = 0x04,
    Disabling = 0x05,
    Releasing = 0x06,
}
}

/// Audio Stream Endpoint Identifier
/// Exposed by the server in ASE characteristics
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AseId(pub u8);

impl AseId {
    const BYTE_SIZE: usize = 1;
}

impl TryFrom<u8> for AseId {
    type Error = ResponseCode;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value == 0 {
            return Err(ResponseCode::InvalidAseId { value });
        }
        Ok(Self(value))
    }
}

impl From<AseId> for u8 {
    fn from(value: AseId) -> Self {
        value.0
    }
}

impl Decodable for AseId {
    type Error = ResponseCode;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < 1 {
            return (Err(ResponseCode::invalid_length()), buf.len());
        }
        (buf[0].try_into(), 1)
    }
}

impl Encodable for AseId {
    type Error = bt_common::packet_encoding::Error;
    fn encoded_len(&self) -> usize {
        1
    }
    fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.is_empty() {
            return Err(Self::Error::BufferTooSmall);
        }
        buf[0] = self.0;
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum AseAdditionalParameters {
    None,
    CodecConfigured {
        framing: Framing,
        preferred_phys: Vec<Phy>,
        preferred_retransmission_number: u8,
        max_transport_latency: MaxTransportLatency,
        presentation_delay_range: PresentationDelayRange,
        codec_id: CodecId,
        codec_config: Vec<u8>,
    },
    QosConfigured {
        configuration: QosConfiguration,
    },
    Streaming {
        cig_id: CigId,
        cis_id: CisId,
        metadata: Vec<Metadata>,
        qos_configured: Option<QosConfiguration>,
    },
}

impl AseAdditionalParameters {
    pub fn char_size(&self) -> usize {
        match self {
            AseAdditionalParameters::None => 0,
            AseAdditionalParameters::CodecConfigured { codec_config, .. } => {
                23 + codec_config.len()
            }
            AseAdditionalParameters::QosConfigured { .. } => 15,
            AseAdditionalParameters::Streaming { metadata, .. } => {
                metadata.iter().fold(3, |total, m| total + m.encoded_len() as usize)
            }
        }
    }

    pub fn into_char_value(&self) -> Vec<u8> {
        match self {
            AseAdditionalParameters::None => Vec::new(),
            AseAdditionalParameters::CodecConfigured {
                framing,
                preferred_phys,
                preferred_retransmission_number,
                max_transport_latency,
                presentation_delay_range,
                codec_id,
                codec_config,
            } => {
                let mut value = Vec::with_capacity(self.char_size());
                value.resize(self.char_size() - codec_config.len(), 0);
                value[0] = (*framing) as u8;
                value[1] = Phy::to_bits(preferred_phys.iter());
                value[2] = *preferred_retransmission_number;
                max_transport_latency.encode(&mut value[3..]).unwrap();
                presentation_delay_range.encode(&mut value[5..]).unwrap();
                codec_id.encode(&mut value[17..]).unwrap();
                value[22] = codec_config.len() as u8;
                value.extend(codec_config.clone());
                value
            }
            AseAdditionalParameters::QosConfigured {
                configuration:
                    QosConfiguration {
                        cig_id,
                        cis_id,
                        sdu_interval,
                        framing,
                        phy,
                        max_sdu,
                        retransmission_number,
                        max_transport_latency,
                        presentation_delay,
                        ..
                    },
            } => {
                let mut value = Vec::with_capacity(self.char_size());
                value.resize(self.char_size(), 0);
                cig_id.encode(&mut value[0..]).unwrap();
                cis_id.encode(&mut value[1..]).unwrap();
                sdu_interval.encode(&mut value[2..]).unwrap();
                framing.encode(&mut value[5..]).unwrap();
                value[6] = Phy::to_bits(phy.iter());
                max_sdu.encode(&mut value[7..]).unwrap();
                value[9] = *retransmission_number;
                max_transport_latency.encode(&mut value[10..]).unwrap();
                presentation_delay.encode(&mut value[12..]).unwrap();
                value
            }
            AseAdditionalParameters::Streaming { cig_id, cis_id, metadata, .. } => {
                let mut value = Vec::with_capacity(self.char_size());
                value.resize(self.char_size(), 0);
                cig_id.encode(&mut value[0..]).unwrap();
                cis_id.encode(&mut value[1..]).unwrap();
                value[2] = metadata.iter().fold(0usize, |acc, i| acc + i.encoded_len()) as u8;
                LtValue::encode_all(metadata.clone().into_iter(), &mut value[3..]).unwrap();
                value
            }
        }
    }

    pub fn decode(
        ase_id: AseId,
        state: &AseState,
        buf: &[u8],
    ) -> (Result<Self, ResponseCode>, usize) {
        match state {
            AseState::Idle | AseState::Releasing => (Ok(Self::None), 0),
            AseState::CodecConfigured => {
                if buf.len() < 23 {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                }
                let Ok(framing) = Framing::try_from(buf[0]) else {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                };
                let preferred_phys = Phy::from_bits(buf[1]).collect();
                let preferred_retransmission_number = buf[2];
                let Ok(max_transport_latency) = MaxTransportLatency::decode(&buf[3..5]).0 else {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                };
                let Ok(presentation_delay_range) = PresentationDelayRange::decode(&buf[5..17]).0
                else {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                };
                let Ok(codec_id) = CodecId::decode(&buf[17..22]).0 else {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                };
                let codec_specific_configuration_len = buf[22] as usize;
                let total_len = 23 + codec_specific_configuration_len;
                if buf.len() < total_len {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                }
                let codec_config = buf[23..total_len].to_vec();
                (
                    Ok(Self::CodecConfigured {
                        framing,
                        preferred_phys,
                        preferred_retransmission_number,
                        max_transport_latency,
                        presentation_delay_range,
                        codec_id,
                        codec_config,
                    }),
                    total_len,
                )
            }
            AseState::QosConfigured => {
                if buf.len() < 15 {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                }
                let mut temp_buf = Vec::with_capacity(16);
                temp_buf.push(ase_id.into());
                temp_buf.extend_from_slice(&buf[0..15]);
                let (config_res, _) = QosConfiguration::decode(&temp_buf);
                match config_res {
                    Ok(configuration) => (Ok(Self::QosConfigured { configuration }), 15),
                    Err(e) => (Err(e), buf.len()),
                }
            }
            AseState::Enabling | AseState::Streaming | AseState::Disabling => {
                if buf.len() < 3 {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                }
                let cig_id = match CigId::try_from(buf[0]) {
                    Ok(id) => id,
                    Err(_) => return (Err(ResponseCode::invalid_length()), buf.len()),
                };
                let cis_id = match CisId::try_from(buf[1]) {
                    Ok(id) => id,
                    Err(_) => return (Err(ResponseCode::invalid_length()), buf.len()),
                };
                let metadata_length = buf[2] as usize;
                let total_len = 3 + metadata_length;
                if buf.len() < total_len {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                }

                use bt_common::core::ltv::Error as LtvError;
                use bt_common::core::ltv::LtValue;
                let (metadata_results, consumed) =
                    Metadata::decode_all(&buf[3..3 + metadata_length]);
                if consumed != metadata_length {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                }
                let metadata_result: Result<Vec<Metadata>, LtvError<<Metadata as LtValue>::Type>> =
                    metadata_results.into_iter().collect();
                let Ok(metadata) = metadata_result else {
                    match metadata_result.unwrap_err() {
                        LtvError::MissingType | LtvError::MissingData(_) => {
                            return (Err(ResponseCode::invalid_length()), buf.len());
                        }
                        LtvError::UnrecognizedType(_, type_value) => {
                            return (
                                Err(ResponseCode::Metadata {
                                    ase_id,
                                    issue: ResponseIssue::Unsupported,
                                    type_value,
                                }),
                                total_len,
                            );
                        }
                        LtvError::LengthOutOfRange(_, t, _)
                        | LtvError::TypeFailedToDecode(t, _) => {
                            return (
                                Err(ResponseCode::Metadata {
                                    ase_id,
                                    issue: ResponseIssue::Invalid,
                                    type_value: t.into(),
                                }),
                                total_len,
                            );
                        }
                    }
                };
                (Ok(Self::Streaming { cig_id, cis_id, metadata, qos_configured: None }), total_len)
            }
        }
    }
}

impl From<QosConfiguration> for AseAdditionalParameters {
    fn from(value: QosConfiguration) -> Self {
        Self::QosConfigured { configuration: value }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDirection {
    Sink,
    Source,
}

impl From<&AudioDirection> for bt_common::Uuid {
    fn from(value: &AudioDirection) -> Self {
        match value {
            AudioDirection::Sink => Uuid::from_u16(0x2BC4),
            AudioDirection::Source => Uuid::from_u16(0x2BC5),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStreamEndpoint {
    pub handle: Handle,
    pub direction: AudioDirection,
    pub ase_id: AseId,
    pub state: AseState,
    pub additional: AseAdditionalParameters,
}

impl AudioStreamEndpoint {
    /// Decodes an `AudioStreamEndpoint` from an ASE characteristic value.
    ///
    /// # Arguments
    /// * `handle` - The GATT handle of the ASE characteristic.
    /// * `direction` - The direction of the ASE (Sink or Source).
    /// * `char_value_buf` - The characteristic value bytes read from the
    ///   server.
    ///
    /// # Expected `char_value_buf` Layout (ASCS v1.0 Section 4.3):
    /// * **Octet 0:** `ASE_ID` (1 octet)
    /// * **Octet 1:** `ASE_State` (1 octet)
    /// * **Octet 2+:** `Additional_ASE_Parameters` (variable octets, see ASCS
    ///   v1.0 Table 4.2)
    pub fn from_char_value(
        handle: Handle,
        direction: AudioDirection,
        char_value_buf: &[u8],
    ) -> Result<Self, ResponseCode> {
        if char_value_buf.len() < 2 {
            return Err(ResponseCode::invalid_length());
        }
        let ase_id = AseId::try_from(char_value_buf[0])?;
        let Ok(state) = AseState::try_from(char_value_buf[1]) else {
            return Err(ResponseCode::invalid_length());
        };
        let (additional_res, _) =
            AseAdditionalParameters::decode(ase_id, &state, &char_value_buf[2..]);
        let additional = additional_res?;

        Ok(Self { handle, direction, ase_id, state, additional })
    }

    /// Encodes this endpoint's current state into a GATT characteristic value
    /// buffer.
    pub fn into_char_value(&self) -> Vec<u8> {
        let mut buf = vec![0; self.encoded_len()];
        self.encode(&mut buf).unwrap();
        buf
    }

    pub fn get_cis(&self) -> Option<(CigId, CisId)> {
        match &self.additional {
            AseAdditionalParameters::QosConfigured { configuration } => {
                Some((configuration.cig_id, configuration.cis_id))
            }
            AseAdditionalParameters::Streaming { cig_id, cis_id, .. } => Some((*cig_id, *cis_id)),
            _ => None,
        }
    }

    pub fn qos_parameters(&self) -> Option<QosParameters> {
        let AseAdditionalParameters::CodecConfigured {
            framing,
            preferred_phys,
            preferred_retransmission_number,
            max_transport_latency,
            presentation_delay_range,
            ..
        } = &self.additional
        else {
            return None;
        };
        Some(QosParameters {
            framing: *framing,
            preferred_phys: preferred_phys.clone(),
            preferred_retransmission_number: *preferred_retransmission_number,
            max_transport_latency: *max_transport_latency,
            presentation_delay_range: presentation_delay_range.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QosParameters {
    pub framing: Framing,
    pub preferred_phys: Vec<Phy>,
    pub preferred_retransmission_number: u8,
    pub max_transport_latency: MaxTransportLatency,
    pub presentation_delay_range: PresentationDelayRange,
}

impl Encodable for AudioStreamEndpoint {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> usize {
        2 + self.additional.char_size()
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(Self::Error::BufferTooSmall);
        }
        buf[0] = self.ase_id.into();
        buf[1] = self.state.into();
        let add_bytes = self.additional.into_char_value();
        buf[2..2 + add_bytes.len()].copy_from_slice(&add_bytes);
        Ok(())
    }
}

decodable_enum! {
    pub enum AseControlPointOpcode<u8, bt_common::packet_encoding::Error, OutOfRange> {
        ConfigCodec = 0x01,
        ConfigQos = 0x02,
        Enable = 0x03,
        ReceiverStartReady = 0x04,
        Disable = 0x05,
        ReceiverStopReady = 0x06,
        UpdateMetadata = 0x07,
        Release = 0x08,
    }
}

impl AseControlPointOpcode {
    pub fn allowed_in_state(&self, state: &AseState) -> bool {
        let allowed_states: &[AseState] = match self {
            Self::ConfigCodec { .. } => {
                &[AseState::Idle, AseState::CodecConfigured, AseState::QosConfigured]
            }
            Self::ConfigQos { .. } => &[AseState::CodecConfigured, AseState::QosConfigured],
            Self::Enable { .. } => &[AseState::QosConfigured],
            Self::ReceiverStartReady { .. } => &[AseState::Enabling],
            Self::ReceiverStopReady { .. } => &[AseState::Disabling],
            Self::Disable { .. } => &[AseState::Streaming, AseState::Enabling],
            Self::UpdateMetadata { .. } => &[AseState::Enabling, AseState::Streaming],
            Self::Release { .. } => &[
                AseState::Enabling,
                AseState::Streaming,
                AseState::CodecConfigured,
                AseState::QosConfigured,
                AseState::Disabling,
            ],
        };
        allowed_states.contains(state)
    }

    /// Returns the list of expected valid states that targeted ASEs should
    /// transition into following a successful invocation of this operation,
    /// depending on the audio stream endpoint direction.
    pub(crate) fn operation_target_states(&self, direction: AudioDirection) -> &[AseState] {
        match self {
            Self::ConfigCodec { .. } => &[AseState::CodecConfigured],
            Self::ConfigQos { .. } => &[AseState::QosConfigured],
            Self::Enable { .. } => &[AseState::Enabling, AseState::Streaming],
            Self::ReceiverStartReady { .. } => &[AseState::Streaming],
            Self::Disable { .. } => match direction {
                AudioDirection::Sink => &[AseState::QosConfigured],
                AudioDirection::Source => &[AseState::Disabling],
            },
            Self::ReceiverStopReady { .. } => &[AseState::QosConfigured],
            Self::UpdateMetadata { .. } => &[AseState::Enabling, AseState::Streaming],
            Self::Release { .. } => {
                &[AseState::Releasing, AseState::Idle, AseState::CodecConfigured]
            }
        }
    }

    /// Returns true if the given `next_state` is a valid transition state
    /// for this operation and audio direction according to the specification.
    pub(crate) fn allows_transition_to(
        &self,
        direction: AudioDirection,
        next_state: &AseState,
    ) -> bool {
        self.operation_target_states(direction).contains(next_state)
    }
}

/// ASE Control Operations.  These can be initiated by a server or client.
/// Defined in Table 4.6 of ASCS v1.0
/// Unrecognized Operations result in Error Operations, which translate directly
/// into an Error response.
/// Some variants already contain responses as decoding errors are detected,
/// i.e. invalid parameters or metadata, which will be delivered after the
/// operation is complete with the results from the rest of the operation.
#[derive(Debug, PartialEq, Clone)]
pub enum AseControlOperation {
    ConfigCodec { codec_configurations: Vec<CodecConfiguration>, responses: Vec<ResponseCode> },
    ConfigQos { qos_configurations: Vec<QosConfiguration>, responses: Vec<ResponseCode> },
    Enable { ases_with_metadata: Vec<AseIdWithMetadata>, responses: Vec<ResponseCode> },
    ReceiverStartReady { ases: Vec<AseId> },
    // The only possible error here is InvalidLength
    Disable { ases: Vec<AseId> },
    // The only possible error here is InvalidLength
    ReceiverStopReady { ases: Vec<AseId> },
    UpdateMetadata { ases_with_metadata: Vec<AseIdWithMetadata>, responses: Vec<ResponseCode> },
    // The only possible error here is InvalidLength
    Release { ases: Vec<AseId> },
    // This is only initiated by the server
    Released { ase_id: AseId },
}

impl AseControlOperation {
    const MIN_BYTE_SIZE: usize = 3;

    fn contains_invalid_length(&self) -> bool {
        match self {
            Self::ConfigCodec { responses, .. }
            | Self::ConfigQos { responses, .. }
            | Self::Enable { responses, .. }
            | Self::UpdateMetadata { responses, .. } => {
                responses.into_iter().find(|x| x.is_invalid_length()).is_some()
            }
            _ => false,
        }
    }
}

impl TryFrom<&AseControlOperation> for u8 {
    type Error = Error;

    fn try_from(value: &AseControlOperation) -> Result<Self, Self::Error> {
        match value {
            AseControlOperation::ConfigCodec { .. } => Ok(0x01),
            AseControlOperation::ConfigQos { .. } => Ok(0x02),
            AseControlOperation::Enable { .. } => Ok(0x03),
            AseControlOperation::ReceiverStartReady { .. } => Ok(0x04),
            AseControlOperation::Disable { .. } => Ok(0x05),
            AseControlOperation::ReceiverStopReady { .. } => Ok(0x06),
            AseControlOperation::UpdateMetadata { .. } => Ok(0x07),
            AseControlOperation::Release { .. } => Ok(0x08),
            AseControlOperation::Released { .. } => {
                Err(Error::Server(ServerError::ServerOnlyOperation))
            }
        }
    }
}

fn partition_results<T, E>(collection: Vec<Result<T, E>>) -> (Vec<T>, Vec<E>) {
    let mut oks = Vec::with_capacity(collection.len());
    let mut errs = Vec::with_capacity(collection.len());
    for item in collection {
        match item {
            Ok(x) => oks.push(x),
            Err(e) => errs.push(e),
        }
    }
    (oks, errs)
}

impl TryFrom<&AseControlOperation> for AseControlPointOpcode {
    type Error = bt_common::packet_encoding::Error;
    fn try_from(value: &AseControlOperation) -> Result<Self, Self::Error> {
        u8::try_from(value).map_err(|_| Self::Error::OutOfRange)?.try_into()
    }
}

impl TryFrom<Vec<u8>> for AseControlOperation {
    type Error = ResponseCode;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        if value.len() < Self::MIN_BYTE_SIZE {
            return Err(ResponseCode::invalid_length());
        }
        let operation: AseControlPointOpcode = match value[0].try_into() {
            Ok(opcode) => opcode,
            Err(_e) => {
                return Err(ResponseCode::unsupported_opcode(value[0]));
            }
        };
        let number_of_ases = value[1] as usize;
        if number_of_ases < 1 {
            return Err(ResponseCode::invalid_length_opcode(value[0]));
        }
        let (op, consumed) = match operation {
            AseControlPointOpcode::ConfigCodec => {
                let (results, consumed) =
                    CodecConfiguration::decode_multiple(&value[2..], Some(number_of_ases));
                let (codec_configurations, responses) = partition_results(results);
                (Self::ConfigCodec { codec_configurations, responses }, consumed)
            }
            AseControlPointOpcode::ConfigQos => {
                let (results, consumed) =
                    QosConfiguration::decode_multiple(&value[2..], Some(number_of_ases));
                let (qos_configurations, responses) = partition_results(results);
                (Self::ConfigQos { qos_configurations, responses }, consumed)
            }
            AseControlPointOpcode::Enable => {
                let (results, consumed) =
                    AseIdWithMetadata::decode_multiple(&value[2..], Some(number_of_ases));
                let (ases_with_metadata, responses) = partition_results(results);
                (Self::Enable { ases_with_metadata, responses }, consumed)
            }
            AseControlPointOpcode::ReceiverStartReady => {
                // Only InvalidLength is possible
                let (results, consumed) = AseId::decode_multiple(&value[2..], Some(number_of_ases));
                let Ok(ases) = results.into_iter().collect::<Result<Vec<_>, ResponseCode>>() else {
                    return Err(ResponseCode::invalid_length_opcode(value[0]));
                };
                (Self::ReceiverStartReady { ases }, consumed)
            }
            AseControlPointOpcode::Disable => {
                // Only InvalidLength is possible
                let (results, consumed) = AseId::decode_multiple(&value[2..], Some(number_of_ases));
                let Ok(ases) = results.into_iter().collect::<Result<Vec<_>, ResponseCode>>() else {
                    return Err(ResponseCode::invalid_length_opcode(value[0]));
                };
                (Self::Disable { ases }, consumed)
            }
            AseControlPointOpcode::ReceiverStopReady => {
                // Only InvalidLength is possible
                let (results, consumed) = AseId::decode_multiple(&value[2..], Some(number_of_ases));
                let Ok(ases) = results.into_iter().collect::<Result<Vec<_>, ResponseCode>>() else {
                    return Err(ResponseCode::invalid_length_opcode(value[0]));
                };
                (Self::ReceiverStopReady { ases }, consumed)
            }
            AseControlPointOpcode::UpdateMetadata => {
                let (results, consumed) =
                    AseIdWithMetadata::decode_multiple(&value[2..], Some(number_of_ases));
                let (ases_with_metadata, responses) = partition_results(results);
                (Self::UpdateMetadata { ases_with_metadata, responses }, consumed)
            }
            AseControlPointOpcode::Release => {
                // Only InvalidLength is possible
                let (results, consumed) = AseId::decode_multiple(&value[2..], Some(number_of_ases));
                let Ok(ases) = results.into_iter().collect::<Result<Vec<_>, ResponseCode>>() else {
                    return Err(ResponseCode::invalid_length_opcode(value[0]));
                };
                (Self::Release { ases }, consumed)
            }
        };
        // A client-initiated ASE Control operation shall also be defined as an invalid
        // length operation if the total length of all parameters written by the
        // client is not equal to the total length of all fixed parameters plus
        // the length of any variable length parameters for that operation as
        // defined in Section 5.1 through Section 5.8.
        if (consumed + 2) != value.len() {
            return Err(ResponseCode::invalid_length_opcode(value[0]));
        }
        if op.contains_invalid_length() {
            return Err(ResponseCode::invalid_length_opcode(value[0]));
        }
        Ok(op)
    }
}

impl Encodable for AseControlOperation {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> usize {
        match self {
            AseControlOperation::ConfigCodec { codec_configurations, .. } => {
                2 + codec_configurations.iter().map(|c| c.encoded_len()).sum::<usize>()
            }
            AseControlOperation::ConfigQos { qos_configurations, .. } => {
                2 + qos_configurations.len() * QosConfiguration::BYTE_SIZE
            }
            AseControlOperation::Enable { ases_with_metadata, .. } => {
                2 + ases_with_metadata.iter().map(|a| a.encoded_len()).sum::<usize>()
            }
            AseControlOperation::ReceiverStartReady { ases } => 2 + ases.len() * AseId::BYTE_SIZE,
            AseControlOperation::Disable { ases } => 2 + ases.len() * AseId::BYTE_SIZE,
            AseControlOperation::ReceiverStopReady { ases } => 2 + ases.len() * AseId::BYTE_SIZE,
            AseControlOperation::UpdateMetadata { ases_with_metadata, .. } => {
                2 + ases_with_metadata.iter().map(|a| a.encoded_len()).sum::<usize>()
            }
            AseControlOperation::Release { ases } => 2 + ases.len() * AseId::BYTE_SIZE,
            AseControlOperation::Released { .. } => 0,
        }
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(Self::Error::BufferTooSmall);
        }
        let opcode: u8 = self.try_into().map_err(|_| Self::Error::OutOfRange)?;
        buf[0] = opcode;
        match self {
            AseControlOperation::ConfigCodec { codec_configurations, .. } => {
                buf[1] = codec_configurations.len() as u8;
                let mut idx = 2;
                for c in codec_configurations {
                    c.encode(&mut buf[idx..])?;
                    idx += c.encoded_len();
                }
            }
            AseControlOperation::ConfigQos { qos_configurations, .. } => {
                buf[1] = qos_configurations.len() as u8;
                let mut idx = 2;
                for q in qos_configurations {
                    q.encode(&mut buf[idx..])?;
                    idx += QosConfiguration::BYTE_SIZE;
                }
            }
            AseControlOperation::Enable { ases_with_metadata, .. } => {
                buf[1] = ases_with_metadata.len() as u8;
                let mut idx = 2;
                for a in ases_with_metadata {
                    a.encode(&mut buf[idx..])?;
                    idx += a.encoded_len();
                }
            }
            AseControlOperation::ReceiverStartReady { ases } => {
                buf[1] = ases.len() as u8;
                let mut idx = 2;
                for a in ases {
                    a.encode(&mut buf[idx..])?;
                    idx += AseId::BYTE_SIZE;
                }
            }
            AseControlOperation::Disable { ases } => {
                buf[1] = ases.len() as u8;
                let mut idx = 2;
                for a in ases {
                    a.encode(&mut buf[idx..])?;
                    idx += AseId::BYTE_SIZE;
                }
            }
            AseControlOperation::ReceiverStopReady { ases } => {
                buf[1] = ases.len() as u8;
                let mut idx = 2;
                for a in ases {
                    a.encode(&mut buf[idx..])?;
                    idx += AseId::BYTE_SIZE;
                }
            }
            AseControlOperation::UpdateMetadata { ases_with_metadata, .. } => {
                buf[1] = ases_with_metadata.len() as u8;
                let mut idx = 2;
                for a in ases_with_metadata {
                    a.encode(&mut buf[idx..])?;
                    idx += a.encoded_len();
                }
            }
            AseControlOperation::Release { ases } => {
                buf[1] = ases.len() as u8;
                let mut idx = 2;
                for a in ases {
                    a.encode(&mut buf[idx..])?;
                    idx += AseId::BYTE_SIZE;
                }
            }
            AseControlOperation::Released { .. } => return Err(Self::Error::OutOfRange),
        }
        Ok(())
    }
}

decodable_enum! {
pub enum TargetLatency<u8, bt_common::packet_encoding::Error, OutOfRange> {
    TargetLowLatency = 0x01,
    TargetBalanced = 0x02,
    TargetHighReliability = 0x03,
}
}

impl TargetLatency {
    const BYTE_SIZE: usize = 1;
}

decodable_enum! {
pub enum TargetPhy<u8, bt_common::packet_encoding::Error, OutOfRange> {
    Le1MPhy = 0x01,
    Le2MPhy = 0x02,
    LeCodedPhy = 0x03,
}
}

impl TargetPhy {
    const BYTE_SIZE: usize = 1;
}

decodable_enum! {
    pub enum Phy<u8, bt_common::packet_encoding::Error, OutOfRange> {
        Le1MPhy = 0b0001,
        Le2MPhy = 0b0010,
        LeCodedPhy = 0b0100,
    }
}

impl From<TargetPhy> for Phy {
    fn from(value: TargetPhy) -> Self {
        match value {
            TargetPhy::Le1MPhy => Self::Le1MPhy,
            TargetPhy::Le2MPhy => Self::Le2MPhy,
            TargetPhy::LeCodedPhy => Self::LeCodedPhy,
        }
    }
}

codable_as_bitmask!(Phy, u8);

impl Phy {
    const BYTE_SIZE: usize = 1;
}

/// Represents Config Codec parameters for a single ASE. See ASCS v1.0.1 Section
/// 5.2.
#[derive(Debug, Clone, PartialEq)]
pub struct CodecConfiguration {
    pub ase_id: AseId,
    pub target_latency: TargetLatency,
    pub target_phy: TargetPhy,
    pub codec_id: CodecId,
    pub codec_specific_configuration: Vec<u8>,
}

impl CodecConfiguration {
    const MIN_BYTE_SIZE: usize =
        AseId::BYTE_SIZE + TargetLatency::BYTE_SIZE + TargetPhy::BYTE_SIZE + CodecId::BYTE_SIZE + 1;
}

impl Decodable for CodecConfiguration {
    type Error = ResponseCode;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::MIN_BYTE_SIZE {
            return (Err(ResponseCode::invalid_length()), buf.len());
        }
        let codec_specific_configuration_len = buf[Self::MIN_BYTE_SIZE - 1] as usize;
        let total_len = codec_specific_configuration_len + Self::MIN_BYTE_SIZE;
        if buf.len() < total_len {
            return (Err(ResponseCode::invalid_length()), buf.len());
        }
        let try_decode_fn = |buf: &[u8]| {
            let ase_id = AseId::try_from(buf[0])?;
            let Ok(target_latency) = TargetLatency::try_from(buf[1]) else {
                // TODO: unclear what to do if the target latency is out of range.
                return Err(ResponseCode::ConfigurationParameterValue {
                    ase_id,
                    issue: ResponseIssue::Invalid,
                    reason: ResponseReason::MaxTransportLatency,
                });
            };
            let Ok(target_phy) = TargetPhy::try_from(buf[2]) else {
                return Err(ResponseCode::ConfigurationParameterValue {
                    ase_id,
                    issue: ResponseIssue::Unsupported,
                    reason: ResponseReason::Phy,
                });
            };
            let Ok(codec_id) = CodecId::decode(&buf[3..]).0 else {
                return Err(ResponseCode::ConfigurationParameterValue {
                    ase_id,
                    issue: ResponseIssue::Invalid,
                    reason: ResponseReason::CodecId,
                });
            };
            let codec_specific_configuration = Vec::from(
                &buf[Self::MIN_BYTE_SIZE..Self::MIN_BYTE_SIZE + codec_specific_configuration_len],
            );
            Ok(Self { ase_id, target_latency, target_phy, codec_id, codec_specific_configuration })
        };
        (try_decode_fn(buf), total_len)
    }
}

impl Encodable for CodecConfiguration {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> usize {
        Self::MIN_BYTE_SIZE + self.codec_specific_configuration.len()
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(Self::Error::BufferTooSmall);
        }
        buf[0] = self.ase_id.into();
        buf[1] = self.target_latency.into();
        buf[2] = self.target_phy.into();
        self.codec_id.encode(&mut buf[3..8])?;
        buf[8] = self.codec_specific_configuration.len() as u8;
        buf[9..9 + self.codec_specific_configuration.len()]
            .copy_from_slice(&self.codec_specific_configuration);
        Ok(())
    }
}

/// Represents Config QoS parameters for a single ASE. See ASCS v1.0.1 Section
/// 5.2.
#[derive(Debug, Clone, PartialEq)]
pub struct QosConfiguration {
    pub ase_id: AseId,
    pub cig_id: CigId,
    pub cis_id: CisId,
    pub sdu_interval: SduInterval,
    pub framing: Framing,
    pub phy: Vec<Phy>,
    pub max_sdu: MaxSdu,
    pub retransmission_number: u8,
    pub max_transport_latency: MaxTransportLatency,
    pub presentation_delay: PresentationDelay,
}

impl QosConfiguration {
    const BYTE_SIZE: usize = AseId::BYTE_SIZE
        + CigId::BYTE_SIZE
        + CisId::BYTE_SIZE
        + SduInterval::BYTE_SIZE
        + Framing::BYTE_SIZE
        + Phy::BYTE_SIZE
        + MaxSdu::BYTE_SIZE
        + 1 // retransmission_number
        + MaxTransportLatency::BYTE_SIZE
        + PresentationDelay::BYTE_SIZE;
}

impl Decodable for QosConfiguration {
    type Error = ResponseCode;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < QosConfiguration::BYTE_SIZE {
            return (Err(ResponseCode::invalid_length()), buf.len());
        }
        let try_decode_fn = |buf: &[u8]| {
            let ase_id = AseId::try_from(buf[0])?;
            let cig_id =
                CigId::try_from(buf[1]).map_err(|_e| ResponseCode::UnspecifiedError { ase_id })?;
            let cis_id =
                CisId::try_from(buf[2]).map_err(|_e| ResponseCode::UnspecifiedError { ase_id })?;
            let sdu_interval;
            match SduInterval::decode(&buf[3..]) {
                (Ok(interval), _) => {
                    sdu_interval = interval;
                }
                (Err(bt_common::packet_encoding::Error::BufferTooSmall), _) => {
                    return Err(ResponseCode::invalid_length());
                }
                (Err(bt_common::packet_encoding::Error::OutOfRange), _) => {
                    return Err(ResponseCode::ConfigurationParameterValue {
                        ase_id,
                        issue: ResponseIssue::Invalid,
                        reason: ResponseReason::SduInterval,
                    });
                }
                _ => unreachable!(),
            };
            let Ok(framing) = Framing::try_from(buf[6]) else {
                return Err(ResponseCode::ConfigurationParameterValue {
                    ase_id,
                    issue: ResponseIssue::Invalid,
                    reason: ResponseReason::Framing,
                });
            };
            let phy = Phy::from_bits(buf[7]).collect();
            let max_sdu = [buf[8], buf[9]].try_into().map_err(|_e| {
                ResponseCode::ConfigurationParameterValue {
                    issue: ResponseIssue::Invalid,
                    reason: ResponseReason::MaximumSduSize,
                    ase_id,
                }
            })?;
            let retransmission_number = buf[10];
            let max_transport_latency =
                MaxTransportLatency::decode(&buf[11..]).0.map_err(|e| match e {
                    bt_common::packet_encoding::Error::BufferTooSmall => {
                        ResponseCode::invalid_length()
                    }
                    bt_common::packet_encoding::Error::OutOfRange => {
                        ResponseCode::ConfigurationParameterValue {
                            issue: ResponseIssue::Invalid,
                            reason: ResponseReason::MaxTransportLatency,
                            ase_id,
                        }
                    }
                    _ => unreachable!(),
                })?;
            let presentation_delay = PresentationDelay::decode(&buf[13..]).0?;
            Ok(Self {
                ase_id,
                cig_id,
                cis_id,
                sdu_interval,
                framing,
                phy,
                max_sdu,
                retransmission_number,
                max_transport_latency,
                presentation_delay,
            })
        };
        (try_decode_fn(buf), QosConfiguration::BYTE_SIZE)
    }
}

impl Encodable for QosConfiguration {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> usize {
        Self::BYTE_SIZE
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.len() < Self::BYTE_SIZE {
            return Err(Self::Error::BufferTooSmall);
        }
        buf[0] = self.ase_id.into();
        self.cig_id.encode(&mut buf[1..2])?;
        self.cis_id.encode(&mut buf[2..3])?;
        self.sdu_interval.encode(&mut buf[3..6])?;
        self.framing.encode(&mut buf[6..7])?;
        buf[7] = Phy::to_bits(self.phy.iter());
        self.max_sdu.encode(&mut buf[8..10])?;
        buf[10] = self.retransmission_number;
        self.max_transport_latency.encode(&mut buf[11..13])?;
        self.presentation_delay.encode(&mut buf[13..16])?;
        Ok(())
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct CigId(u8);

impl CigId {
    const BYTE_SIZE: usize = 1;
}

impl TryFrom<u8> for CigId {
    type Error = bt_common::packet_encoding::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value > 0xEF {
            Err(Self::Error::OutOfRange)
        } else {
            Ok(CigId(value))
        }
    }
}

impl From<CigId> for u8 {
    fn from(value: CigId) -> Self {
        value.0
    }
}

impl Encodable for CigId {
    type Error = bt_common::packet_encoding::Error;
    fn encoded_len(&self) -> core::primitive::usize {
        Self::BYTE_SIZE
    }
    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < Self::BYTE_SIZE {
            return Err(Self::Error::BufferTooSmall);
        }
        buf[0] = self.0;
        Ok(())
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct CisId(u8);

impl CisId {
    const BYTE_SIZE: usize = 1;
}

impl TryFrom<u8> for CisId {
    type Error = bt_common::packet_encoding::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value > 0xEF {
            Err(Self::Error::OutOfRange)
        } else {
            Ok(CisId(value))
        }
    }
}

impl From<CisId> for u8 {
    fn from(value: CisId) -> Self {
        value.0
    }
}

impl Encodable for CisId {
    type Error = bt_common::packet_encoding::Error;
    fn encoded_len(&self) -> core::primitive::usize {
        Self::BYTE_SIZE
    }
    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < Self::BYTE_SIZE {
            return Err(Self::Error::BufferTooSmall);
        }
        buf[0] = self.0;
        Ok(())
    }
}

/// SDU Inteval parameter
/// This value is 24 bits long and little-endian on the wire.
/// It is stored native-endian here.
/// Valid range is [0x0000FF, 0x0FFFFF].
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct SduInterval(u32);

impl SduInterval {
    const BYTE_SIZE: usize = 3;
}

impl TryFrom<std::time::Duration> for SduInterval {
    type Error = bt_common::packet_encoding::Error;
    fn try_from(value: std::time::Duration) -> Result<Self, Self::Error> {
        let Ok(microseconds) = u32::try_from(value.as_micros()) else {
            return Err(Self::Error::OutOfRange);
        };
        if microseconds < 0xFF || microseconds > 0x0FFFFF {
            return Err(Self::Error::OutOfRange);
        }
        Ok(Self(microseconds))
    }
}

impl TryFrom<u32> for SduInterval {
    type Error = bt_common::packet_encoding::Error;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value < 0xFF || value > 0x0FFFFF {
            return Err(Self::Error::OutOfRange);
        }
        Ok(Self(value))
    }
}

impl Decodable for SduInterval {
    type Error = bt_common::packet_encoding::Error;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::BYTE_SIZE {
            return (Err(Self::Error::BufferTooSmall), buf.len());
        }
        let val = u32::from_le_bytes([buf[0], buf[1], buf[2], 0]);
        if (val < 0xFF) || (val > 0x0FFFFF) {
            return (Err(Self::Error::OutOfRange), Self::BYTE_SIZE);
        }
        (Ok(SduInterval(val)), Self::BYTE_SIZE)
    }
}

impl Encodable for SduInterval {
    type Error = bt_common::packet_encoding::Error;
    fn encoded_len(&self) -> core::primitive::usize {
        Self::BYTE_SIZE
    }
    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < Self::BYTE_SIZE {
            return Err(Self::Error::BufferTooSmall);
        }
        [buf[0], buf[1], buf[2], _] = self.0.to_le_bytes();
        Ok(())
    }
}

decodable_enum! {
pub enum Framing<u8, bt_common::packet_encoding::Error, OutOfRange> {
    Unframed = 0x00,
    Framed = 0x01,
}
}

impl Framing {
    const BYTE_SIZE: usize = 1;
}

impl Encodable for Framing {
    type Error = bt_common::packet_encoding::Error;
    fn encoded_len(&self) -> core::primitive::usize {
        Self::BYTE_SIZE
    }
    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < Self::BYTE_SIZE {
            return Err(Self::Error::BufferTooSmall);
        }
        buf[0] = (*self).into();
        Ok(())
    }
}

/// Max SDU parameter value.
/// Valid range is 0x0000-0x0FFF
/// Transmitted in little-endian. Stored here in native-endian.
#[derive(Debug, Clone, PartialEq)]
pub struct MaxSdu(u16);

impl TryFrom<[u8; 2]> for MaxSdu {
    type Error = bt_common::packet_encoding::Error;

    fn try_from(value: [u8; 2]) -> Result<Self, Self::Error> {
        let value = u16::from_le_bytes([value[0], value[1]]);
        if value > 0xFFF {
            return Err(Self::Error::OutOfRange);
        }
        Ok(MaxSdu(value))
    }
}

impl Encodable for MaxSdu {
    type Error = bt_common::packet_encoding::Error;
    fn encoded_len(&self) -> core::primitive::usize {
        Self::BYTE_SIZE
    }
    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < Self::BYTE_SIZE {
            return Err(Self::Error::BufferTooSmall);
        }
        [buf[0], buf[1]] = self.0.to_le_bytes();
        Ok(())
    }
}

impl MaxSdu {
    const BYTE_SIZE: usize = 2;
}

impl TryFrom<u16> for MaxSdu {
    type Error = bt_common::packet_encoding::Error;
    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if value > 0xFFF {
            return Err(Self::Error::OutOfRange);
        }
        Ok(Self(value))
    }
}

/// Max Transport Latency
/// Valid range is [0x0005, 0x0FA0].
/// Transmitted in little-endian, Stored in native-endian.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaxTransportLatency(u16);

impl Decodable for MaxTransportLatency {
    type Error = bt_common::packet_encoding::Error;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::BYTE_SIZE {
            return (Err(Self::Error::BufferTooSmall), buf.len());
        }
        let val = u16::from_le_bytes([buf[0], buf[1]]);
        if val < 0x0005 || val > 0x0FA0 {
            return (Err(Self::Error::OutOfRange), Self::BYTE_SIZE);
        }
        (Ok(MaxTransportLatency(val)), Self::BYTE_SIZE)
    }
}

impl Encodable for MaxTransportLatency {
    type Error = bt_common::packet_encoding::Error;
    fn encoded_len(&self) -> core::primitive::usize {
        Self::BYTE_SIZE
    }
    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < 2 {
            return Err(Self::Error::BufferTooSmall);
        }
        [buf[0], buf[1]] = self.0.to_le_bytes();
        Ok(())
    }
}

impl TryFrom<std::time::Duration> for MaxTransportLatency {
    type Error = bt_common::packet_encoding::Error;

    fn try_from(value: std::time::Duration) -> Result<Self, Self::Error> {
        let Ok(milliseconds) = u16::try_from(value.as_millis()) else {
            return Err(Self::Error::OutOfRange);
        };
        if !(0x0005..=0x0FA0).contains(&milliseconds) {
            return Err(Self::Error::OutOfRange);
        }
        Ok(Self(milliseconds))
    }
}

impl MaxTransportLatency {
    const BYTE_SIZE: usize = 2;
}

/// Presentation delay parameter value being requested by the client for an ASE.
/// This value is 24 bits long (0x00FFFFFF max)
/// Transmitted in little-endian, Stored in native-endian.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PresentationDelay {
    pub microseconds: u32,
}

impl PresentationDelay {
    const BYTE_SIZE: usize = 3;
}

impl Decodable for PresentationDelay {
    type Error = ResponseCode;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::BYTE_SIZE {
            return (Err(ResponseCode::invalid_length()), buf.len());
        }
        let microseconds = u32::from_le_bytes([buf[0], buf[1], buf[2], 0]);
        (Ok(PresentationDelay { microseconds }), Self::BYTE_SIZE)
    }
}

impl Encodable for PresentationDelay {
    type Error = bt_common::packet_encoding::Error;
    fn encoded_len(&self) -> core::primitive::usize {
        Self::BYTE_SIZE
    }
    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < Self::BYTE_SIZE {
            return Err(Self::Error::BufferTooSmall);
        }
        let encoded_le = self.microseconds.to_le_bytes();
        [buf[0], buf[1], buf[2]] = [encoded_le[0], encoded_le[1], encoded_le[2]];
        Ok(())
    }
}

/// Presentation Delay Range.  Used to indicate the supported range and
/// preferred range of the Presentation Delay parameter to be requested by the
/// ASCS Client. Prefered Minimum must be above min, and preferred_max must be
/// below max. Either of these being None indicates no preference.
#[derive(Debug, Clone, PartialEq)]
pub struct PresentationDelayRange {
    min: PresentationDelay,
    max: PresentationDelay,
    preferred_min: Option<PresentationDelay>,
    preferred_max: Option<PresentationDelay>,
}

impl PresentationDelayRange {
    const BYTE_SIZE: usize = PresentationDelay::BYTE_SIZE * 4;

    pub fn min(&self) -> &PresentationDelay {
        &self.min
    }

    pub fn max(&self) -> &PresentationDelay {
        &self.max
    }

    pub fn preferred_min(&self) -> Option<&PresentationDelay> {
        self.preferred_min.as_ref()
    }

    pub fn preferred_max(&self) -> Option<&PresentationDelay> {
        self.preferred_max.as_ref()
    }
    /// Make a new delay range with no preference. Returns
    /// ResponseCode::InvalidLength if min > max or the value is out of the
    /// acceptable range (PresentationDelay is 24 bits)
    pub fn build(min_us: u32, max_us: u32) -> Result<Self, ResponseCode> {
        if min_us > max_us || min_us > 0x00FFFFFF || max_us > 0x00FFFFFF {
            return Err(ResponseCode::invalid_length());
        }
        Ok(Self {
            min: PresentationDelay { microseconds: min_us },
            max: PresentationDelay { microseconds: max_us },
            preferred_min: None,
            preferred_max: None,
        })
    }
    /// Set the preferred range.  If the range does not fit into the supported
    /// range, returns Err(ResponseCode::InvalidLength).
    pub fn with_preferred(
        &mut self,
        min_us: Option<u32>,
        max_us: Option<u32>,
    ) -> Result<&mut Self, ResponseCode> {
        self.preferred_min = match min_us {
            Some(min_us)
                if min_us < self.min.microseconds || max_us.is_some_and(|max| max < min_us) =>
            {
                return Err(ResponseCode::invalid_length());
            }
            Some(min_us) => Some(PresentationDelay { microseconds: min_us }),
            None => None,
        };
        self.preferred_max = match max_us {
            Some(max_us)
                if max_us > self.max.microseconds || min_us.is_some_and(|min| min > max_us) =>
            {
                return Err(ResponseCode::invalid_length());
            }
            Some(max_us) => Some(PresentationDelay { microseconds: max_us }),
            None => None,
        };
        Ok(self)
    }
}

impl Encodable for PresentationDelayRange {
    type Error = bt_common::packet_encoding::Error;
    fn encoded_len(&self) -> core::primitive::usize {
        Self::BYTE_SIZE
    }
    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < Self::BYTE_SIZE {
            return Err(Self::Error::BufferTooSmall);
        }
        buf[0..Self::BYTE_SIZE].fill(0);
        self.min.encode(&mut buf[0..3])?;
        self.max.encode(&mut buf[3..6])?;
        if let Some(pref_min) = &self.preferred_min {
            pref_min.encode(&mut buf[6..9])?;
        }
        if let Some(pref_max) = &self.preferred_max {
            pref_max.encode(&mut buf[9..12])?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AseIdWithMetadata {
    pub ase_id: AseId,
    pub metadata: Vec<Metadata>,
}

impl AseIdWithMetadata {
    const MIN_BYTE_SIZE: usize = 2;
}

impl Decodable for AseIdWithMetadata {
    type Error = ResponseCode;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::MIN_BYTE_SIZE {
            return (Err(ResponseCode::invalid_length()), buf.len());
        }
        let ase_id = match AseId::try_from(buf[0]) {
            Ok(ase_id) => ase_id,
            Err(e) => return (Err(e), buf.len()),
        };
        let metadata_length = buf[1] as usize;
        let total_length = Self::MIN_BYTE_SIZE + metadata_length;
        if buf.len() < total_length {
            return (Err(ResponseCode::invalid_length()), buf.len());
        }
        use bt_common::core::ltv::Error as LtvError;
        use bt_common::core::ltv::LtValue;
        let (metadata_results, consumed) = Metadata::decode_all(&buf[2..2 + metadata_length]);
        if consumed != metadata_length {
            return (Err(ResponseCode::invalid_length()), buf.len());
        }
        let metadata_result: Result<Vec<Metadata>, LtvError<<Metadata as LtValue>::Type>> =
            metadata_results.into_iter().collect();
        let Ok(metadata) = metadata_result else {
            match metadata_result.unwrap_err() {
                LtvError::MissingType => return (Err(ResponseCode::invalid_length()), buf.len()),
                LtvError::MissingData(_) => {
                    return (Err(ResponseCode::invalid_length()), buf.len());
                }
                LtvError::UnrecognizedType(_, type_value) => {
                    return (
                        Err(ResponseCode::Metadata {
                            ase_id,
                            issue: ResponseIssue::Unsupported,
                            type_value,
                        }),
                        total_length,
                    );
                }
                LtvError::LengthOutOfRange(_, t, _) | LtvError::TypeFailedToDecode(t, _) => {
                    return (
                        Err(ResponseCode::Metadata {
                            ase_id,
                            issue: ResponseIssue::Invalid,
                            type_value: t.into(),
                        }),
                        total_length,
                    );
                }
            }
        };
        (Ok(Self { ase_id, metadata }), total_length)
    }
}

impl Encodable for AseIdWithMetadata {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> usize {
        let metadata_len: usize = self.metadata.iter().map(|m| m.encoded_len()).sum();
        2 + metadata_len
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(Self::Error::BufferTooSmall);
        }
        buf[0] = self.ase_id.into();
        let metadata_len: usize = self.metadata.iter().map(|m| m.encoded_len()).sum();
        buf[1] = metadata_len as u8;

        use bt_common::core::ltv::LtValue;
        LtValue::encode_all(self.metadata.clone().into_iter(), &mut buf[2..])?;
        Ok(())
    }
}

impl Decodable for PresentationDelayRange {
    type Error = ResponseCode;

    fn decode(buf: &[u8]) -> (Result<Self, Self::Error>, usize) {
        if buf.len() < Self::BYTE_SIZE {
            return (Err(ResponseCode::invalid_length()), buf.len());
        }
        let min = match PresentationDelay::decode(&buf[0..3]).0 {
            Ok(min) => min,
            Err(e) => return (Err(e), buf.len()),
        };
        let max = match PresentationDelay::decode(&buf[3..6]).0 {
            Ok(max) => max,
            Err(e) => return (Err(e), buf.len()),
        };
        let pref_min_val = u32::from_le_bytes([buf[6], buf[7], buf[8], 0]);
        let preferred_min = if pref_min_val == 0 {
            None
        } else {
            Some(PresentationDelay { microseconds: pref_min_val })
        };
        let pref_max_val = u32::from_le_bytes([buf[9], buf[10], buf[11], 0]);
        let preferred_max = if pref_max_val == 0 {
            None
        } else {
            Some(PresentationDelay { microseconds: pref_max_val })
        };
        (Ok(Self { min, max, preferred_min, preferred_max }), Self::BYTE_SIZE)
    }
}

/// Represents an ASE Control Point notification sent by an ASCS Server to a
/// Client.
///
/// See ASCS v1.0 Section 4.2 for the frame structure and notification
/// properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPointNotification {
    pub opcode: u8,
    pub number_of_ases: u8,
    pub responses: Vec<ResponseCode>,
}

impl ControlPointNotification {
    /// Creates a new, empty `ControlPointNotification` for standard multi-ASE
    /// operations.
    ///
    /// The notification starts with a response count of `0`.
    ///
    /// # Arguments
    /// * `opcode` - The opcode of the operation being responded to.
    pub fn new(opcode: u8) -> Self {
        Self { opcode, number_of_ases: 0, responses: Vec::new() }
    }

    /// Creates a new global error `ControlPointNotification`.
    ///
    /// A response count of `0xFF` is used strictly when the entire control
    /// point write is rejected globally before individual ASEs are
    /// processed (e.g., unrecognized opcode or invalid packet length). The
    /// notification contains a single error response with an
    /// ASE ID of `0x00`. See ASCS v1.0 Table 4.7.
    ///
    /// # Arguments
    /// * `opcode` - The opcode of the operation being responded to.
    /// * `response_code` - Either `0x01` (Unsupported Opcode) or `0x02`
    ///   (Invalid Length).
    pub fn new_error(opcode: u8, response_code: u8) -> Self {
        let code = match response_code {
            0x01 => ResponseCode::UnsupportedOpcode { opcode_byte: opcode },
            0x02 => ResponseCode::InvalidLength { opcode_byte: opcode },
            _ => unreachable!(),
        };
        Self { opcode, number_of_ases: 0xFF, responses: vec![code] }
    }

    /// Adds a single ASE response code to the notification list.
    ///
    /// If the notification is currently in a global error state (response count
    /// `0xFF`), calling this method clears the error and transitions it
    /// back to a standard multi-ASE response list.
    ///
    /// # Arguments
    /// * `response` - The `ResponseCode` variant to append to the notification.
    pub fn add_response(&mut self, response: ResponseCode) {
        if self.number_of_ases == 0xFF {
            self.number_of_ases = 0;
            self.responses.clear();
        }
        self.responses.push(response);
        self.number_of_ases = self.responses.len() as u8;
    }
}

impl Encodable for ControlPointNotification {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> usize {
        2 + self.responses.len() * 3
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(Self::Error::BufferTooSmall);
        }
        buf[0] = self.opcode;
        buf[1] = self.number_of_ases;
        let mut idx = 2;
        for r in &self.responses {
            let val = r.notify_value();
            buf[idx..idx + 3].copy_from_slice(&val);
            idx += 3;
        }
        Ok(())
    }
}

impl Decodable for ControlPointNotification {
    type Error = bt_common::packet_encoding::Error;

    fn decode(buf: &[u8]) -> (Result<Self, Self::Error>, usize) {
        if buf.len() < 2 {
            return (Err(Self::Error::UnexpectedDataLength), buf.len());
        }
        let opcode = buf[0];
        let number_of_ases = buf[1];
        let responses_buf = &buf[2..];
        let mut responses = Vec::new();

        if number_of_ases == 0xFF {
            if responses_buf.len() < 3 {
                return (Err(Self::Error::UnexpectedDataLength), buf.len());
            }
            let r = match ResponseCode::decode_response(&responses_buf[0..3], opcode) {
                Ok(r) => r,
                Err(e) => return (Err(e), buf.len()),
            };
            if r.ase_id_value() != 0 || (r.to_code() != 0x01 && r.to_code() != 0x02) {
                return (Err(Self::Error::UnexpectedDataLength), buf.len());
            }
            responses.push(r);
            (Ok(Self { opcode, number_of_ases, responses }), 5)
        } else {
            let expected_len = number_of_ases as usize * 3;
            if responses_buf.len() < expected_len {
                return (Err(Self::Error::UnexpectedDataLength), buf.len());
            }
            for i in 0..(number_of_ases as usize) {
                let idx = i * 3;
                let r = match ResponseCode::decode_response(&responses_buf[idx..idx + 3], opcode) {
                    Ok(r) => r,
                    Err(e) => return (Err(e), buf.len()),
                };
                responses.push(r);
            }
            (Ok(Self { opcode, number_of_ases, responses }), 2 + expected_len)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bt_common::packet_encoding::Encodable;

    use bt_common::core::ltv::LtValue;
    use bt_common::generic_audio::{codec_configuration, AudioLocation};

    #[test]
    fn opcode_state_rules() {
        // 1. allowed_in_state tests
        assert!(AseControlPointOpcode::ConfigCodec.allowed_in_state(&AseState::Idle));
        assert!(AseControlPointOpcode::ConfigCodec.allowed_in_state(&AseState::CodecConfigured));
        assert!(!AseControlPointOpcode::ConfigCodec.allowed_in_state(&AseState::Streaming));

        assert!(AseControlPointOpcode::Disable.allowed_in_state(&AseState::Streaming));
        assert!(!AseControlPointOpcode::Disable.allowed_in_state(&AseState::Idle));

        // 2. operation_target_states & allows_transition_to tests
        // Sink direction
        assert!(AseControlPointOpcode::Disable
            .allows_transition_to(AudioDirection::Sink, &AseState::QosConfigured));
        assert!(!AseControlPointOpcode::Disable
            .allows_transition_to(AudioDirection::Sink, &AseState::Disabling));

        // Source direction
        assert!(AseControlPointOpcode::Disable
            .allows_transition_to(AudioDirection::Source, &AseState::Disabling));
        assert!(!AseControlPointOpcode::Disable
            .allows_transition_to(AudioDirection::Source, &AseState::QosConfigured));

        // UpdateMetadata (same state allowed)
        assert!(AseControlPointOpcode::UpdateMetadata
            .allows_transition_to(AudioDirection::Source, &AseState::Enabling));
        assert!(AseControlPointOpcode::UpdateMetadata
            .allows_transition_to(AudioDirection::Source, &AseState::Streaming));
        assert!(!AseControlPointOpcode::UpdateMetadata
            .allows_transition_to(AudioDirection::Source, &AseState::Idle));
    }

    #[test]
    fn codec_configuration_roundtrip() {
        let codec_specific_configuration = vec![
            codec_configuration::CodecConfiguration::SamplingFrequency(
                codec_configuration::SamplingFrequency::F48000Hz,
            ),
            codec_configuration::CodecConfiguration::FrameDuration(
                codec_configuration::FrameDuration::TenMs,
            ),
            codec_configuration::CodecConfiguration::AudioChannelAllocation(
                [AudioLocation::FrontLeft].into_iter().collect(),
            ),
            codec_configuration::CodecConfiguration::CodecFramesPerSdu(1),
        ];
        let codec_config_len =
            codec_specific_configuration.iter().fold(0, |a, x| a + x.encoded_len());
        let mut vec = Vec::with_capacity(codec_config_len);
        vec.resize(codec_config_len, 0);
        LtValue::encode_all(codec_specific_configuration.clone().into_iter(), &mut vec[..])
            .unwrap();

        let _codec_config = CodecConfiguration {
            ase_id: AseId(5),
            target_latency: TargetLatency::TargetLowLatency,
            target_phy: TargetPhy::Le1MPhy,
            codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Lc3),
            codec_specific_configuration: vec,
        };
    }

    #[test]
    fn codec_configuration_decode() {
        let encoded = &[
            0x04, 0x02, 0x02, 0x06, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02, 0x01, 0x08, 0x02, 0x02,
            0x01, 0x05, 0x03, 0x01, 0x00, 0x00, 0x00, 0x03, 0x04, 0x78, 0x00,
        ];

        let (codec_config, size) = CodecConfiguration::decode(&encoded[..]);
        let codec_config = codec_config.unwrap();
        assert_eq!(size, encoded.len());
        assert_eq!(codec_config.ase_id, AseId(4));
        assert_eq!(codec_config.target_latency, TargetLatency::TargetBalanced);
        assert_eq!(codec_config.target_phy, TargetPhy::Le2MPhy);
        assert_eq!(codec_config.codec_id, CodecId::Assigned(bt_common::core::CodingFormat::Lc3));
        assert_eq!(codec_config.codec_specific_configuration.len(), 0x10);

        #[rustfmt::skip]
        let encoded_qos = &[
            0x02, 0x01, // Qos OpCode, 1 number_of_ases
            0x03, // ASE_ID,
            0x00, // CIG_ID,
            0x00, // CIS_ID,
            0x10, 0x27, 0x00, // SduInterval
            0x01, 0x02, 0x28, 0x00, 0x02, 0xf4,
            0x01, 0x18, 0x00, 0x80,
        ];
        assert_eq!(QosConfiguration::BYTE_SIZE, encoded_qos.len() - 2);
        let (qos_config, consumed) = QosConfiguration::decode(&encoded_qos[2..]);
        assert!(qos_config.is_ok());
        assert_eq!(consumed, encoded_qos.len() - 2);
        let encoded_qos: Vec<u8> = encoded_qos.into_iter().copied().collect();
        let operation = AseControlOperation::try_from(encoded_qos);
        let Ok(_operation) = operation else {
            panic!("Expected decode to work correctly, for {operation:?}");
        };
    }

    #[test]
    fn ase_control_operation_decode_config_codec() {
        let encoded = &[
            0x01, 0x01, 0x04, 0x02, 0x02, 0x06, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02, 0x01, 0x08,
            0x02, 0x02, 0x01, 0x05, 0x03, 0x01, 0x00, 0x00, 0x00, 0x03, 0x04, 0x78, 0x00,
        ];

        let codec_config = CodecConfiguration::decode(&encoded[2..]).0.unwrap();
        let encoded_vec: Vec<u8> = encoded.into_iter().copied().collect();
        let operation = AseControlOperation::try_from(encoded_vec);

        assert_eq!(
            operation,
            Ok(AseControlOperation::ConfigCodec {
                codec_configurations: vec![codec_config],
                responses: Vec::new()
            })
        );
    }

    #[test]
    fn ase_control_operation_decode_config_codec_some_failures() {
        #[rustfmt::skip]
        let encoded = &[
            0x01, 0x02, // Config Codec, Two ASE_IDs
            // First ASE_ID config
            0x04, 0x02, 0x02, 0x06, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02, 0x01, 0x08,
            0x02, 0x02, 0x01, 0x05, 0x03, 0x01, 0x00, 0x00, 0x00, 0x03, 0x04, 0x78, 0x00,
            // Second ASE_ID config, fails based on invalid parameter value
            0x05, 0x05, 0x02, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let codec_config = CodecConfiguration::decode(&encoded[2..]).0.unwrap();
        let encoded_vec: Vec<u8> = encoded.into_iter().copied().collect();
        let operation = AseControlOperation::try_from(encoded_vec);

        match operation {
            Ok(AseControlOperation::ConfigCodec { codec_configurations, responses }) => {
                assert_eq!(codec_configurations, vec![codec_config]);
                assert_eq!(responses.len(), 1);
                assert!(matches!(responses[0], ResponseCode::ConfigurationParameterValue { .. }));
            }
            x => panic!("Expected ConfigCodec, got {x:?}"),
        };
    }

    #[test]
    fn ase_control_operation_decode_invalid_length() {
        #[rustfmt::skip]
        let encoded = &[
            0x01, 0x02, // Config Codec, Two ASE_IDs
            // First ASE_ID config
            0x04, 0x02, 0x02, 0x06, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02, 0x01, 0x08,
            0x02, 0x02, 0x01, 0x05, 0x03, 0x01, 0x00, 0x00, 0x00, 0x03, 0x04, 0x78, 0x00,
            // Second ASE_ID config, fails based on invalid length
            0x05, 0x05, 0x02, 0x06, 0x00, 0x00, 0x00,
        ];

        let encoded_vec: Vec<u8> = encoded.into_iter().copied().collect();
        let operation = AseControlOperation::try_from(encoded_vec);

        assert_eq!(operation, Err(ResponseCode::invalid_length_opcode(0x01)));
    }

    #[test]
    fn qos_configuration_decode() {
        let encoded = &[
            0x03, // ASE_ID,
            0x00, // CIG_ID,
            0x00, // CIS_ID,
            0x10, 0x27, 0x00, 0x01, 0x02, 0x28, 0x00, 0x02, 0xf4, 0x01, 0x18, 0x00, 0x80,
        ];
        let (qos_config, consumed) = QosConfiguration::decode(&encoded[..]);

        let qos_config = qos_config.unwrap();

        assert_eq!(consumed, encoded.len());
        assert_eq!(qos_config.ase_id, AseId(3));
        assert_eq!(qos_config.cig_id, CigId(0));
        assert_eq!(qos_config.cis_id, CisId(0));
        assert_eq!(qos_config.sdu_interval, SduInterval(10000));
        assert_eq!(qos_config.framing, Framing::Framed);
        assert_eq!(qos_config.phy, vec![Phy::Le2MPhy]);
        assert_eq!(qos_config.max_sdu, MaxSdu(40));
        assert_eq!(qos_config.retransmission_number, 2);
        assert_eq!(qos_config.max_transport_latency, MaxTransportLatency(500));
        assert_eq!(qos_config.presentation_delay, PresentationDelay { microseconds: 8388632 });
    }

    #[test]
    fn aseid_with_metadata_decode() {
        let encoded = &[
            0x03, // ASE_ID
            0x08, // metadata length
            0x04, 0x04, 0x65, 0x6E, 0x67, // Language code
            0x02, 0x03, 0x61, // Program Info
        ];

        let (ase_with_metadata, consumed) = AseIdWithMetadata::decode(&encoded[..]);
        let ase_with_metadata = ase_with_metadata.unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(ase_with_metadata.ase_id, AseId(3));
        assert_eq!(ase_with_metadata.metadata.len(), 2);
    }

    #[test]
    fn ase_control_operation_decode_qos() {
        #[rustfmt::skip]
        let encoded_qos = &[
            0x02, 0x01, // Qos OpCode, 1 number_of_ases
            0x03, // ASE_ID,
            0x00, // CIG_ID,
            0x00, // CIS_ID,
            0x10, 0x27, 0x00, // SduInterval
            0x01, 0x02, 0x28, 0x00, 0x02, 0xf4,
            0x01, 0x18, 0x00, 0x80,
        ];
        assert_eq!(QosConfiguration::BYTE_SIZE, encoded_qos.len() - 2);
        let (qos_config, consumed) = QosConfiguration::decode(&encoded_qos[2..]);
        assert!(qos_config.is_ok());
        assert_eq!(consumed, encoded_qos.len() - 2);
        let encoded_qos: Vec<u8> = encoded_qos.into_iter().copied().collect();
        let operation = AseControlOperation::try_from(encoded_qos);
        assert!(operation.is_ok());

        #[rustfmt::skip]
        let encoded_qos_one_fails = &[
            0x02, 0x03, // Qos OpCode, 3 number_of_ases
            0x03, // ASE_ID,
            0x00, // CIG_ID,
            0x00, // CIS_ID,
            0x10, 0x27, 0x00, // SduInterval
            0x01, 0x02, 0x28, 0x00, 0x02, 0xf4,
            0x01, 0x18, 0x00, 0x80,
            0x00, // ASE_ID, (AseId 0 is not allowed)
            0x00, // CIG_ID,
            0x00, // CIS_ID,
            0x10, 0x27, 0x00, // SduInterval
            0x01, 0x02, 0x28, 0x00, 0x02, 0xf4,
            0x01, 0x18, 0x00, 0x80,
            0x04, // ASE_ID
            0x00, // CIG_ID,
            0x00, // CIS_ID,
            0x10, 0x17, 0x00, // SduInterval
            0x01, 0x02, 0x28, 0x00, 0x02, 0xf4,
            0x01, 0x18, 0x00, 0x80,
        ];

        let encoded_qos_one_fails: Vec<u8> = encoded_qos_one_fails.into_iter().copied().collect();

        match AseControlOperation::try_from(encoded_qos_one_fails) {
            Ok(AseControlOperation::ConfigQos { qos_configurations, responses }) => {
                assert_eq!(qos_configurations.len(), 2);
                assert_eq!(responses.len(), 1);
                assert_eq!(responses[0], ResponseCode::InvalidAseId { value: 0x00 });
            }
            x => panic!("Expected ConfigQos to succeed, got {x:?}"),
        };
    }

    #[test]
    fn presentation_delay_range() {
        // Min must be < than max, and within the right range
        assert!(PresentationDelayRange::build(10, 1).is_err());
        assert!(PresentationDelayRange::build(1000, 0xC0FFEE00).is_err());
        assert!(PresentationDelayRange::build(0xC0FFEE00, 0xC0FFEE01).is_err());

        let mut delay_range = PresentationDelayRange::build(1000, 2000).unwrap();

        assert_eq!(delay_range.min.microseconds, 1000);
        assert_eq!(delay_range.max.microseconds, 2000);

        assert!(delay_range.with_preferred(None, None).is_ok());
        assert_eq!(delay_range.preferred_min, None);
        assert!(delay_range.with_preferred(Some(900), None).is_err());
        assert_eq!(delay_range.preferred_min, None);
        assert!(delay_range.with_preferred(Some(1500), Some(1001)).is_err());
        assert_eq!(delay_range.preferred_min, None);
        assert!(delay_range.with_preferred(Some(1500), None).is_ok());
        assert_eq!(delay_range.preferred_min, Some(PresentationDelay { microseconds: 1500 }));
        assert_eq!(delay_range.preferred_max, None);

        assert!(delay_range.with_preferred(None, Some(2100)).is_err());
        assert!(delay_range.with_preferred(Some(1801), Some(1800)).is_err());
        assert!(delay_range.with_preferred(Some(1500), Some(1800)).is_ok());
        assert_eq!(delay_range.preferred_min, Some(PresentationDelay { microseconds: 1500 }));
        assert_eq!(delay_range.preferred_max, Some(PresentationDelay { microseconds: 1800 }));

        let mut encoded = [0; PresentationDelayRange::BYTE_SIZE];
        assert!(delay_range.encode(&mut encoded).is_ok());

        #[rustfmt::skip]
        let expected: &[u8; 12] = &[
            0xE8, 0x03, 0x00, // 1000
            0xD0, 0x07, 0x00, // 2000
            0xDC, 0x05, 0x00, // 1500
            0x08, 0x07, 0x00, // 1800
        ];

        assert_eq!(&encoded, expected);
    }

    #[test]
    fn test_ase_endpoint_roundtrip() {
        use bt_gatt::types::Handle;
        // Test with AdditionalParameters::None
        let endpoint_none = AudioStreamEndpoint {
            handle: Handle(1),
            direction: AudioDirection::Sink,
            ase_id: AseId(1),
            state: AseState::Idle,
            additional: AseAdditionalParameters::None,
        };
        let mut buf = vec![0; endpoint_none.encoded_len()];
        endpoint_none.encode(&mut buf).unwrap();
        assert_eq!(buf.len(), 2);
        assert_eq!(buf, vec![1, 0]);

        let decoded_endpoint_none =
            AudioStreamEndpoint::from_char_value(Handle(1), AudioDirection::Sink, &buf).unwrap();
        assert_eq!(decoded_endpoint_none.ase_id, endpoint_none.ase_id);
        assert_eq!(decoded_endpoint_none.state, endpoint_none.state);
        assert_eq!(decoded_endpoint_none.additional, endpoint_none.additional);

        // Test with AdditionalParameters::CodecConfigured
        let delay_range = PresentationDelayRange::build(1000, 2000).unwrap();
        let endpoint_codec = AudioStreamEndpoint {
            handle: Handle(2),
            direction: AudioDirection::Source,
            ase_id: AseId(2),
            state: AseState::CodecConfigured,
            additional: AseAdditionalParameters::CodecConfigured {
                framing: Framing::Unframed,
                preferred_phys: vec![Phy::Le1MPhy].into_iter().collect(),
                preferred_retransmission_number: 3,
                max_transport_latency: MaxTransportLatency::try_from(
                    std::time::Duration::from_millis(10),
                )
                .unwrap(),
                presentation_delay_range: delay_range,
                codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Lc3),
                codec_config: vec![1, 2, 3],
            },
        };
        let mut buf2 = vec![0; endpoint_codec.encoded_len()];
        endpoint_codec.encode(&mut buf2).unwrap();

        let decoded_endpoint_codec =
            AudioStreamEndpoint::from_char_value(Handle(2), AudioDirection::Source, &buf2).unwrap();
        assert_eq!(decoded_endpoint_codec.ase_id, endpoint_codec.ase_id);
        assert_eq!(decoded_endpoint_codec.state, endpoint_codec.state);
        assert_eq!(decoded_endpoint_codec.additional, endpoint_codec.additional);
    }

    #[test]
    fn control_point_notification_roundtrip() {
        // 1. Test Standard Multi-ASE notification roundtrip
        let mut notification = ControlPointNotification::new(0x01); // Opcode ConfigCodec
        notification.add_response(ResponseCode::Success { ase_id: AseId(1) });
        notification.add_response(ResponseCode::InvalidAseId { value: 2 });

        let mut buf = vec![0; notification.encoded_len()];
        assert!(notification.encode(&mut buf).is_ok());

        #[rustfmt::skip]
        let expected_bytes = &[
            0x01, // Opcode
            0x02, // Number of ASEs
            0x01, 0x00, 0x00, // Response 1: ASE 1, Success, None
            0x02, 0x03, 0x00, // Response 2: ASE 2, Invalid ASE ID, None
        ];
        assert_eq!(buf, expected_bytes);

        let (decoded, consumed) = ControlPointNotification::decode(&buf);
        assert_eq!(consumed, buf.len());
        assert_eq!(decoded.expect("should succeed"), notification);

        // 2. Test Global Error Notification (0xFF) roundtrip
        let err_notification = ControlPointNotification::new_error(0x01, 0x02); // Invalid Length error
        let mut err_buf = vec![0; err_notification.encoded_len()];
        assert!(err_notification.encode(&mut err_buf).is_ok());

        #[rustfmt::skip]
        let expected_err_bytes = &[
            0x01, // Opcode
            0xFF, // Number of ASEs: Global Error
            0x00, 0x02, 0x00, // ASE 0, Invalid Length, None
        ];
        assert_eq!(err_buf, expected_err_bytes);

        let (decoded_err, consumed_err) = ControlPointNotification::decode(&err_buf);
        assert_eq!(consumed_err, err_buf.len());
        assert_eq!(decoded_err.expect("should succeed"), err_notification);

        // 3. Test Validation constraint fails on malformed 0xFF notification
        // Case A: number_of_ases = 0xFF, but ase_id is not 0x00 (e.g., AseId 1)
        #[rustfmt::skip]
        let malformed_bytes_ase = &[
            0x01, // Opcode
            0xFF, // Global Error
            0x01, 0x02, 0x00, // ASE 1, Invalid Length, None (ase_id must be 0x00)
        ];
        let (res_malformed_ase, _) = ControlPointNotification::decode(malformed_bytes_ase);
        assert_eq!(res_malformed_ase, Err(bt_common::packet_encoding::Error::OutOfRange));

        // Case B: number_of_ases = 0xFF, but response_code is not 0x01 or 0x02 (e.g.
        // Success 0x00)
        #[rustfmt::skip]
        let malformed_bytes_code = &[
            0x01, // Opcode
            0xFF, // Global Error
            0x00, 0x00, 0x00, // ASE 0, Success, None (response code must be 0x01 or 0x02)
        ];
        let (res_malformed_code, _) = ControlPointNotification::decode(malformed_bytes_code);
        assert_eq!(
            res_malformed_code,
            Err(bt_common::packet_encoding::Error::UnexpectedDataLength)
        );
    }
}
