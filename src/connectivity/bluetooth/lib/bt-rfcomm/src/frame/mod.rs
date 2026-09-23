// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use packet_encoding::{Decodable, Encodable, decodable_enum};

/// The command or response classification used when parsing an RFCOMM frame.
mod command_response;
pub use command_response::CommandResponse;
/// Errors associated with parsing an RFCOMM frame.
mod error;
pub use error::FrameParseError;
/// Frame Check Sequence calculations.
mod fcs;
/// Field definitions for an RFCOMM frame.
mod field;
/// Definitions for multiplexer command frames.
pub mod mux_commands;

use self::fcs::{calculate_fcs, verify_fcs};
use self::field::*;
use self::mux_commands::MuxCommand;
use crate::{DLCI, Role};

decodable_enum! {
    /// The type of frame provided in the Control field.
    /// The P/F bit is set to 0 for all frame types.
    /// See table 2, GSM 07.10 Section 5.2.1.3 and RFCOMM 4.2.
    pub enum FrameTypeMarker<u8, FrameParseError, UnsupportedFrameType> {
        SetAsynchronousBalancedMode = 0b00101111,
        UnnumberedAcknowledgement = 0b01100011,
        DisconnectedMode = 0b00001111,
        Disconnect = 0b01000011,
        UnnumberedInfoHeaderCheck = 0b11101111,
    }
}

impl FrameTypeMarker {
    /// Returns true if the frame type is a valid multiplexer start-up frame.
    //
    /// These are the only frames which are allowed to be sent before the multiplexer starts, and
    /// must be sent over the Mux Control Channel.
    fn is_mux_startup(&self, dlci: &DLCI) -> bool {
        dlci.is_mux_control()
            && (*self == FrameTypeMarker::SetAsynchronousBalancedMode
                || *self == FrameTypeMarker::UnnumberedAcknowledgement
                || *self == FrameTypeMarker::DisconnectedMode)
    }

    /// Returns the number of octets needed when calculating the FCS.
    fn fcs_octets(&self) -> usize {
        // For UIH frames, the first 2 bytes of the buffer are used to calculate the FCS.
        // Otherwise, the first 3. Defined in RFCOMM 5.1.1.
        if *self == FrameTypeMarker::UnnumberedInfoHeaderCheck { 2 } else { 3 }
    }

    /// Returns true if the `frame_type` is expected to contain a credit octet.
    ///
    /// `credit_based_flow` indicates whether credit flow control is enabled for the session.
    /// `poll_final` is the P/F bit associated with the Control Field of the frame.
    /// `dlci` is the DLCI associated with the frame.
    ///
    /// RFCOMM 6.5.2 describes the RFCOMM specifics for credit-based flow control. Namely,
    /// "...It does not apply to DLCI 0 or to non-UIH frames."
    fn has_credit_octet(&self, credit_based_flow: bool, poll_final: bool, dlci: DLCI) -> bool {
        *self == FrameTypeMarker::UnnumberedInfoHeaderCheck
            && !dlci.is_mux_control()
            && credit_based_flow
            && poll_final
    }
}

/// A UIH Frame that contains user data.
#[derive(Clone, Debug, PartialEq)]
pub struct UserData {
    pub information: Vec<u8>,
}

impl UserData {
    pub fn is_empty(&self) -> bool {
        self.information.is_empty()
    }

    pub fn empty() -> Self {
        Self { information: vec![] }
    }
}

impl Decodable for UserData {
    type Error = FrameParseError;

    fn decode(buf: &[u8]) -> Result<Self, FrameParseError> {
        Ok(Self { information: buf.to_vec() })
    }
}

impl Encodable for UserData {
    type Error = FrameParseError;

    fn encoded_len(&self) -> usize {
        self.information.len()
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), FrameParseError> {
        if buf.len() < self.encoded_len() {
            return Err(FrameParseError::BufferTooSmall);
        }
        buf.copy_from_slice(&self.information);
        Ok(())
    }
}

/// The data associated with a UIH Frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UIHData {
    /// A UIH Frame with user data.
    User(UserData),
    /// A UIH Frame with a Mux Command.
    Mux(MuxCommand),
}

impl Encodable for UIHData {
    type Error = FrameParseError;

    fn encoded_len(&self) -> usize {
        match self {
            UIHData::User(data) => data.encoded_len(),
            UIHData::Mux(command) => command.encoded_len(),
        }
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), FrameParseError> {
        if buf.len() < self.encoded_len() {
            return Err(FrameParseError::BufferTooSmall);
        }

        match self {
            UIHData::User(data) => data.encode(buf),
            UIHData::Mux(command) => command.encode(buf),
        }
    }
}

/// The types of frames supported in RFCOMM.
/// See RFCOMM 4.2 for the supported frame types.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameData {
    SetAsynchronousBalancedMode,
    UnnumberedAcknowledgement,
    DisconnectedMode,
    Disconnect,
    UnnumberedInfoHeaderCheck(UIHData),
}

impl FrameData {
    pub fn marker(&self) -> FrameTypeMarker {
        match self {
            FrameData::SetAsynchronousBalancedMode => FrameTypeMarker::SetAsynchronousBalancedMode,
            FrameData::UnnumberedAcknowledgement => FrameTypeMarker::UnnumberedAcknowledgement,
            FrameData::DisconnectedMode => FrameTypeMarker::DisconnectedMode,
            FrameData::Disconnect => FrameTypeMarker::Disconnect,
            FrameData::UnnumberedInfoHeaderCheck(_) => FrameTypeMarker::UnnumberedInfoHeaderCheck,
        }
    }

    fn decode(
        frame_type: &FrameTypeMarker,
        dlci: &DLCI,
        buf: &[u8],
    ) -> Result<Self, FrameParseError> {
        let data = match frame_type {
            FrameTypeMarker::SetAsynchronousBalancedMode => FrameData::SetAsynchronousBalancedMode,
            FrameTypeMarker::UnnumberedAcknowledgement => FrameData::UnnumberedAcknowledgement,
            FrameTypeMarker::DisconnectedMode => FrameData::DisconnectedMode,
            FrameTypeMarker::Disconnect => FrameData::Disconnect,
            FrameTypeMarker::UnnumberedInfoHeaderCheck => {
                let uih_data = if dlci.is_mux_control() {
                    UIHData::Mux(MuxCommand::decode(buf)?)
                } else {
                    UIHData::User(UserData::decode(buf)?)
                };
                FrameData::UnnumberedInfoHeaderCheck(uih_data)
            }
        };
        Ok(data)
    }
}

impl Encodable for FrameData {
    type Error = FrameParseError;

    fn encoded_len(&self) -> usize {
        match self {
            FrameData::SetAsynchronousBalancedMode
            | FrameData::UnnumberedAcknowledgement
            | FrameData::DisconnectedMode
            | FrameData::Disconnect => 0,
            FrameData::UnnumberedInfoHeaderCheck(data) => data.encoded_len(),
        }
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), FrameParseError> {
        if buf.len() < self.encoded_len() {
            return Err(FrameParseError::BufferTooSmall);
        }

        match self {
            FrameData::SetAsynchronousBalancedMode
            | FrameData::UnnumberedAcknowledgement
            | FrameData::DisconnectedMode
            | FrameData::Disconnect => Ok(()),
            FrameData::UnnumberedInfoHeaderCheck(data) => data.encode(buf),
        }
    }
}

/// The minimum frame size (bytes) for an RFCOMM Frame - Address, Control, Length, FCS.
/// See RFCOMM 5.1.
const MIN_FRAME_SIZE: usize = 4;

/// The maximum size (bytes) of an RFCOMM header in a packet.
/// Address (1 byte), Control (1 byte), Length (2 bytes), Credits (1 byte), FCS (1 byte)
/// See RFCOMM 5.1.
pub const MAX_RFCOMM_HEADER_SIZE: usize = 6;

/// The maximum length that can be represented in a single E/A padded octet.
const MAX_SINGLE_OCTET_LENGTH: usize = 127;

/// Maximum length (in bytes) for an RFCOMM frame payload.
/// GSM 07.10 Section 5.2.1.4 limits length indicators to 15 bits.
pub const MAX_INFORMATION_LENGTH: usize = 32767;

/// Returns true if the provided `length` needs to be represented as 2 octets.
fn is_two_octet_length(length: usize) -> bool {
    length > MAX_SINGLE_OCTET_LENGTH
}

/// Returns the C/R bit for a non-UIH frame.
fn cr_bit_for_non_uih_frame(role: Role, command_response: CommandResponse) -> bool {
    // Defined in GSM Section 5.2.1.2 Table 1.
    match (role, command_response) {
        (Role::Initiator, CommandResponse::Command)
        | (Role::Responder, CommandResponse::Response) => true,
        _ => false,
    }
}

/// Returns the C/R bit for a UIH frame.
/// This must only be used on frames sent after multiplexer startup.
fn cr_bit_for_uih_frame(role: Role) -> bool {
    // The C/R bit is based on subclause 5.4.3.1 and matches the role of the device.
    match role {
        Role::Initiator => true,
        _ => false,
    }
}

/// The highest-level unit of data that is passed around in RFCOMM.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The role of the device associated with this frame.
    pub role: Role,
    /// The DLCI associated with this frame.
    pub dlci: DLCI,
    /// The data associated with this frame.
    pub data: FrameData,
    /// The P/F bit for this frame. See RFCOMM 5.2.1 which describes the usages
    /// of the P/F bit in RFCOMM.
    pub poll_final: bool,
    /// Whether this frame is a Command or Response frame.
    pub command_response: CommandResponse,
    /// The credits associated with this frame. Credits are only applicable to UIH frames
    /// when credit-based flow control is enabled. See RFCOMM 6.5.
    pub credits: Option<u8>,
}

impl Frame {
    /// Attempts to parse the provided `buf` into a Frame.
    ///
    /// `role` is the current Role of the RFCOMM Session.
    /// `credit_based_flow` indicates whether credit-based flow control is turned on for this
    /// Session.
    pub fn parse(role: Role, credit_based_flow: bool, buf: &[u8]) -> Result<Self, FrameParseError> {
        if buf.len() < MIN_FRAME_SIZE {
            return Err(FrameParseError::BufferTooSmall);
        }

        // Parse the Address Field of the frame.
        let address_field = AddressField(buf[FRAME_ADDRESS_IDX]);
        let dlci: DLCI = address_field.dlci()?;
        let cr_bit: bool = address_field.cr_bit();

        // Parse the Control Field of the frame.
        let control_field = ControlField(buf[FRAME_CONTROL_IDX]);
        let frame_type: FrameTypeMarker = control_field.frame_type()?;
        let poll_final = control_field.poll_final();

        // If the Session multiplexer hasn't started, then the `frame_type` must be a
        // multiplexer startup frame.
        if !role.is_multiplexer_started() && !frame_type.is_mux_startup(&dlci) {
            return Err(FrameParseError::InvalidFrame);
        }

        // Classify the frame as either a Command or Response depending on the role, type of frame,
        // and the C/R bit of the Address Field.
        let command_response = CommandResponse::classify(role, frame_type, cr_bit)?;

        // Parse the Information field of the Frame. If the EA bit is 0, then we need to construct
        // the InformationLength using two bytes.
        let information_field = InformationField(buf[FRAME_INFORMATION_IDX]);
        let is_two_octet_length = !information_field.ea_bit();
        let mut length = information_field.length() as u16;
        if is_two_octet_length {
            length |= (buf[FRAME_INFORMATION_IDX + 1] as u16) << INFORMATION_SECOND_OCTET_SHIFT;
        }

        // The header size depends on the Information Length size and the (optional) credits octet.
        // Address (1) + Control (1) + Length (1 or 2)
        let mut header_size = 2 + if is_two_octet_length { 2 } else { 1 };
        let mut credits = None;
        if frame_type.has_credit_octet(credit_based_flow, poll_final, dlci) {
            if buf.len() < header_size {
                return Err(FrameParseError::BufferTooSmall);
            }
            credits = Some(buf[header_size]);
            header_size += 1;
        }

        // Check the FCS before parsing the body of the packet.
        let fcs_index = header_size + usize::from(length);
        if buf.len() <= fcs_index {
            return Err(FrameParseError::BufferTooSmall);
        }
        let fcs = buf[fcs_index];
        if !verify_fcs(fcs, &buf[..frame_type.fcs_octets()]) {
            return Err(FrameParseError::FCSCheckFailed);
        }

        let data = &buf[header_size..fcs_index];
        let data = FrameData::decode(&frame_type, &dlci, data)?;

        Ok(Self { role, dlci, data, poll_final, command_response, credits })
    }

    pub fn make_sabm_command(role: Role, dlci: DLCI) -> Self {
        Self {
            role,
            dlci,
            data: FrameData::SetAsynchronousBalancedMode,
            poll_final: true, // Always set for SABM.
            command_response: CommandResponse::Command,
            credits: None,
        }
    }

    pub fn make_dm_response(role: Role, dlci: DLCI) -> Self {
        Self {
            role,
            dlci,
            data: FrameData::DisconnectedMode,
            poll_final: true, // Always set for DM response.
            command_response: CommandResponse::Response,
            credits: None,
        }
    }

    pub fn make_ua_response(role: Role, dlci: DLCI) -> Self {
        Self {
            role,
            dlci,
            data: FrameData::UnnumberedAcknowledgement,
            poll_final: true, // Always set for UA response.
            command_response: CommandResponse::Response,
            credits: None,
        }
    }

    pub fn make_mux_command(role: Role, data: MuxCommand) -> Self {
        let command_response = data.command_response;
        Self {
            role,
            dlci: DLCI::MUX_CONTROL_DLCI,
            data: FrameData::UnnumberedInfoHeaderCheck(UIHData::Mux(data)),
            poll_final: false, // Always unset for UIH frames, GSM 5.4.3.1.
            command_response,
            credits: None,
        }
    }

    pub fn make_user_data_frame(
        role: Role,
        dlci: DLCI,
        user_data: UserData,
        credits: Option<u8>,
    ) -> Self {
        // When credit based flow control is supported, the `poll_final` bit is redefined
        // for UIH frames. See RFCOMM 6.5.2. If credits are provided, then the `poll_final` bit
        // should be set.
        Self {
            role,
            dlci,
            data: FrameData::UnnumberedInfoHeaderCheck(UIHData::User(user_data)),
            poll_final: credits.is_some(),
            command_response: CommandResponse::Command,
            credits,
        }
    }

    pub fn make_disc_command(role: Role, dlci: DLCI) -> Self {
        Self {
            role,
            dlci,
            data: FrameData::Disconnect,
            poll_final: true, // Always set for Disconnect.
            command_response: CommandResponse::Command,
            credits: None,
        }
    }
}

impl Encodable for Frame {
    type Error = FrameParseError;

    fn encoded_len(&self) -> usize {
        // Address + Control + FCS + (optional) Credits + 1 or 2 octets for Length + Frame data.
        3 + self.credits.map_or(0, |_| 1)
            + if is_two_octet_length(self.data.encoded_len()) { 2 } else { 1 }
            + self.data.encoded_len()
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), FrameParseError> {
        if buf.len() != self.encoded_len() {
            return Err(FrameParseError::BufferTooSmall);
        }

        let assumed_role = if !self.role.is_multiplexer_started() {
            if !self.data.marker().is_mux_startup(&self.dlci) {
                return Err(FrameParseError::InvalidFrame);
            }
            // The role is only determined after the multiplexer starts. Per GSM 5.2.1.2, the
            // initiating side always sends the first SABM.
            if self.data.marker() == FrameTypeMarker::SetAsynchronousBalancedMode {
                Role::Initiator
            } else {
                Role::Responder
            }
        } else {
            self.role
        };
        // The C/R bit of the Address Field depends on the frame type:
        //   - For UIH frames, the C/R bit is based on GSM Section 5.4.3.1.
        //   - For other frames, the C/R bit is determined by Table 1 in GSM Section 5.2.1.2.
        let cr_bit = if self.data.marker() == FrameTypeMarker::UnnumberedInfoHeaderCheck {
            cr_bit_for_uih_frame(assumed_role)
        } else {
            cr_bit_for_non_uih_frame(assumed_role, self.command_response)
        };

        // Set the Address Field, E/A = 1 since there is only one octet.
        let mut address_field = AddressField(0);
        address_field.set_ea_bit(true);
        address_field.set_cr_bit(cr_bit);
        address_field.set_dlci(u8::from(self.dlci));
        buf[FRAME_ADDRESS_IDX] = address_field.0;

        // Control Field.
        let mut control_field = ControlField(0);
        control_field.set_frame_type(u8::from(&self.data.marker()));
        control_field.set_poll_final(self.poll_final);
        buf[FRAME_CONTROL_IDX] = control_field.0;

        // Information Field.
        let data_length = self.data.encoded_len();
        if data_length > MAX_INFORMATION_LENGTH {
            return Err(FrameParseError::FrameTooLarge(data_length));
        }
        let is_two_octet_length = is_two_octet_length(data_length);
        let mut first_octet_length = InformationField(0);
        first_octet_length.set_length(data_length as u8);
        first_octet_length.set_ea_bit(!is_two_octet_length);
        buf[FRAME_INFORMATION_IDX] = first_octet_length.0;
        // If the length is two octets, get the upper 8 bits and set the second octet.
        if is_two_octet_length {
            let second_octet_length = (data_length >> INFORMATION_SECOND_OCTET_SHIFT) as u8;
            buf[FRAME_INFORMATION_IDX + 1] = second_octet_length;
        }

        // Address + Control + Information.
        let mut header_size = 2 + if is_two_octet_length { 2 } else { 1 };

        // Encode the credits for this frame, if applicable.
        let credit_based_flow = self.credits.is_some();
        if self.data.marker().has_credit_octet(credit_based_flow, self.poll_final, self.dlci) {
            buf[header_size] = self.credits.unwrap();
            header_size += 1;
        }

        let fcs_idx = header_size + data_length as usize;

        // Frame data.
        self.data.encode(&mut buf[header_size..fcs_idx])?;

        // FCS that is computed based on `frame_type`.
        buf[fcs_idx] = calculate_fcs(&buf[..self.data.marker().fcs_octets()]);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::frame::mux_commands::ModemStatusParams;

    use super::*;

    use assert_matches::assert_matches;
    use mux_commands::{MuxCommandParams, RemotePortNegotiationParams};

    #[test]
    fn test_is_mux_startup_frame() {
        let control_dlci = DLCI::try_from(0).unwrap();
        let user_dlci = DLCI::try_from(5).unwrap();

        let frame_type = FrameTypeMarker::SetAsynchronousBalancedMode;
        assert!(frame_type.is_mux_startup(&control_dlci));
        assert!(!frame_type.is_mux_startup(&user_dlci));

        let frame_type = FrameTypeMarker::UnnumberedAcknowledgement;
        assert!(frame_type.is_mux_startup(&control_dlci));
        assert!(!frame_type.is_mux_startup(&user_dlci));

        let frame_type = FrameTypeMarker::DisconnectedMode;
        assert!(frame_type.is_mux_startup(&control_dlci));
        assert!(!frame_type.is_mux_startup(&user_dlci));

        let frame_type = FrameTypeMarker::Disconnect;
        assert!(!frame_type.is_mux_startup(&control_dlci));
        assert!(!frame_type.is_mux_startup(&user_dlci));
    }

    #[test]
    fn test_has_credit_octet() {
        let frame_type = FrameTypeMarker::UnnumberedInfoHeaderCheck;
        let pf = true;
        let credit_based_flow = true;
        let dlci = DLCI::try_from(3).unwrap();
        assert!(frame_type.has_credit_octet(credit_based_flow, pf, dlci));

        let pf = false;
        let credit_based_flow = true;
        assert!(!frame_type.has_credit_octet(credit_based_flow, pf, dlci));

        let pf = true;
        let credit_based_flow = false;
        assert!(!frame_type.has_credit_octet(credit_based_flow, pf, dlci));

        let pf = true;
        let credit_based_flow = true;
        let dlci = DLCI::try_from(0).unwrap(); // Mux DLCI.
        assert!(!frame_type.has_credit_octet(credit_based_flow, pf, dlci));

        let pf = false;
        let credit_based_flow = false;
        assert!(!frame_type.has_credit_octet(credit_based_flow, pf, dlci));

        let frame_type = FrameTypeMarker::SetAsynchronousBalancedMode;
        let pf = true;
        let credit_based_flow = true;
        let dlci = DLCI::try_from(5).unwrap();
        assert!(!frame_type.has_credit_octet(credit_based_flow, pf, dlci));
    }

    #[test]
    fn test_parse_too_small_frame() {
        let role = Role::Unassigned;
        let buf: &[u8] = &[0x00];
        assert_matches!(Frame::parse(role, false, buf), Err(FrameParseError::BufferTooSmall));
    }

    #[test]
    fn test_parse_invalid_dlci() {
        let role = Role::Unassigned;
        let buf: &[u8] = &[
            0b00000101, // Address Field - EA = 1, C/R = 0, DLCI = 1.
            0b00101111, // Control Field - SABM command with P/F = 0.
            0b00000001, // Length Field - Bit0 = 1: Indicates one octet length.
            0x00,       // Random FCS.
        ];
        assert_matches!(Frame::parse(role, false, buf), Err(FrameParseError::InvalidDLCI(1)));
    }

    /// It's possible that a remote device sends a packet with an invalid frame.
    /// In this case, we should error gracefully.
    #[test]
    fn test_parse_invalid_frame_type() {
        let role = Role::Unassigned;
        let buf: &[u8] = &[
            0b00000001, // Address Field - EA = 1, C/R = 0, DLCI = 0.
            0b10101010, // Control Field - Invalid command with P/F = 0.
            0b00000001, // Length Field - Bit1 = 0 indicates 1 octet length.
            0x00,       // Random FCS.
        ];
        assert_matches!(Frame::parse(role, false, buf), Err(FrameParseError::UnsupportedFrameType));
    }

    /// It's possible that the remote peer sends a packet for a valid frame, but the session
    /// multiplexer has not started. In this case, we should error gracefully.
    #[test]
    fn test_parse_invalid_frame_type_sent_before_mux_startup() {
        let role = Role::Unassigned;
        let buf: &[u8] = &[
            0b00000001, // Address Field - EA = 1, C/R = 0, DLCI = 0.
            0b11101111, // Control Field - UnnumberedInfoHeaderCheck with P/F = 0.
            0b00000001, // Length Field - Bit1 = 0 indicates 1 octet length.
            0x00,       // Random FCS.
        ];
        assert_matches!(Frame::parse(role, false, buf), Err(FrameParseError::InvalidFrame));
    }

    #[test]
    fn test_parse_invalid_frame_missing_fcs() {
        let role = Role::Unassigned;
        let buf: &[u8] = &[
            0b00000011, // Address Field - EA = 1, C/R = 1, DLCI = 0.
            0b00101111, // Control Field - SABM command with P/F = 0.
            0b00000000, // Length Field - Bit1 = 0 Indicates two octet length.
            0b00000001, // Second octet of length.
                        // Missing FCS.
        ];
        assert_matches!(Frame::parse(role, false, buf), Err(FrameParseError::BufferTooSmall));
    }

    #[test]
    fn test_parse_valid_frame_over_mux_dlci() {
        let role = Role::Unassigned;
        let frame_type = FrameTypeMarker::SetAsynchronousBalancedMode;
        let mut buf = vec![
            0b00000011, // Address Field - EA = 1, C/R = 1, DLCI = 0.
            0b00101111, // Control Field - SABM command with P/F = 0.
            0b00000001, // Length Field - Bit1 = 1 Indicates one octet length - no info.
        ];
        // Calculate the FCS and tack it on to the end.
        let fcs = calculate_fcs(&buf[..frame_type.fcs_octets()]);
        buf.push(fcs);

        let res = Frame::parse(role, false, &buf[..]).unwrap();
        let expected_frame = Frame {
            role,
            dlci: DLCI::try_from(0).unwrap(),
            data: FrameData::SetAsynchronousBalancedMode,
            poll_final: false,
            command_response: CommandResponse::Command,
            credits: None,
        };
        assert_eq!(res, expected_frame);
    }

    #[test]
    fn test_parse_valid_frame_over_user_dlci() {
        let role = Role::Responder;
        let frame_type = FrameTypeMarker::SetAsynchronousBalancedMode;
        let mut buf = vec![
            0b00001111, // Address Field - EA = 1, C/R = 1, User DLCI = 3.
            0b00101111, // Control Field - SABM command with P/F = 0.
            0b00000001, // Length Field - Bit1 = 1 Indicates one octet length - no info.
        ];
        // Calculate the FCS for the first three bytes, since non-UIH frame.
        let fcs = calculate_fcs(&buf[..frame_type.fcs_octets()]);
        buf.push(fcs);

        let res = Frame::parse(role, false, &buf[..]).unwrap();
        let expected_frame = Frame {
            role,
            dlci: DLCI::try_from(3).unwrap(),
            data: FrameData::SetAsynchronousBalancedMode,
            poll_final: false,
            command_response: CommandResponse::Response,
            credits: None,
        };
        assert_eq!(res, expected_frame);
    }

    #[test]
    fn test_parse_frame_with_information_length_invalid_buf_size() {
        let role = Role::Responder;
        let frame_type = FrameTypeMarker::UnnumberedInfoHeaderCheck;
        let mut buf = vec![
            0b00001111, // Address Field - EA = 1, C/R = 1, User DLCI = 3.
            0b11101111, // Control Field - UIH command with P/F = 0.
            0b00000111, // Length Field - Bit1 = 1 Indicates one octet length = 3.
            0b00000000, // Data octet #1 - missing octets 2,3.
        ];
        // Calculate the FCS for the first two bytes, since UIH frame.
        let fcs = calculate_fcs(&buf[..frame_type.fcs_octets()]);
        buf.push(fcs);

        assert_matches!(Frame::parse(role, false, &buf[..]), Err(FrameParseError::BufferTooSmall));
    }

    #[test]
    fn test_parse_valid_frame_with_information_length() {
        let role = Role::Responder;
        let frame_type = FrameTypeMarker::UnnumberedInfoHeaderCheck;
        let mut buf = vec![
            0b00001101, // Address Field - EA = 1, C/R = 0, User DLCI = 3.
            0b11101111, // Control Field - UIH command with P/F = 0.
            0b00000101, // Length Field - Bit1 = 1 Indicates one octet length = 2.
            0b00000000, // Data octet #1,
            0b00000000, // Data octet #2,
        ];
        // Calculate the FCS for the first two bytes, since UIH frame.
        let fcs = calculate_fcs(&buf[..frame_type.fcs_octets()]);
        buf.push(fcs);

        let res = Frame::parse(role, false, &buf[..]).unwrap();
        let expected_frame = Frame {
            role,
            dlci: DLCI::try_from(3).unwrap(),
            data: FrameData::UnnumberedInfoHeaderCheck(UIHData::User(UserData {
                information: vec![
                    0b00000000, // Data octet #1.
                    0b00000000, // Data octet #2.
                ],
            })),
            poll_final: false,
            command_response: CommandResponse::Response,
            credits: None,
        };
        assert_eq!(res, expected_frame);
    }

    #[test]
    fn test_parse_valid_frame_with_two_octet_information_length() {
        let role = Role::Responder;
        let frame_type = FrameTypeMarker::UnnumberedInfoHeaderCheck;
        let length = 129;
        let length_data = vec![0; length];

        // Concatenate the header, `length_data` payload, and FCS.
        let buf = vec![
            0b00001101, // Address Field - EA = 1, C/R = 0, User DLCI = 3.
            0b11101111, // Control Field - UIH command with P/F = 0.
            0b00000010, // Length Field0 - E/A = 0. Length = 1.
            0b00000001, // Length Field1 - No E/A. Length = 128.
        ];
        // Calculate the FCS for the first two bytes, since UIH frame.
        let fcs = calculate_fcs(&buf[..frame_type.fcs_octets()]);
        let buf = [buf, length_data.clone(), vec![fcs]].concat();

        let res = Frame::parse(role, false, &buf[..]).unwrap();
        let expected_frame = Frame {
            role,
            dlci: DLCI::try_from(3).unwrap(),
            data: FrameData::UnnumberedInfoHeaderCheck(UIHData::User(UserData {
                information: length_data,
            })),
            poll_final: false,
            command_response: CommandResponse::Response,
            credits: None,
        };
        assert_eq!(res, expected_frame);
    }

    #[test]
    fn test_parse_uih_frame_with_mux_command() {
        let role = Role::Responder;
        let frame_type = FrameTypeMarker::UnnumberedInfoHeaderCheck;
        let mut buf = vec![
            0b00000001, // Address Field - EA = 1, C/R = 0, Mux DLCI = 0.
            0b11111111, // Control Field - UIH command with P/F = 1.
            0b00000111, // Length Field - Bit1 = 1 Indicates one octet length = 3.
            0b10010001, // Data octet #1 - RPN command.
            0b00000011, // Data octet #2 - RPN Command length = 1.
            0b00011111, // Data octet #3 - RPN Data, DLCI = 7.
        ];
        // Calculate the FCS for the first two bytes, since UIH frame.
        let fcs = calculate_fcs(&buf[..frame_type.fcs_octets()]);
        buf.push(fcs);

        let res = Frame::parse(role, false, &buf[..]).unwrap();
        let expected_mux_command = MuxCommand {
            params: MuxCommandParams::RemotePortNegotiation(RemotePortNegotiationParams {
                dlci: DLCI::try_from(7).unwrap(),
                port_values: None,
            }),
            command_response: CommandResponse::Response,
        };
        let expected_frame = Frame {
            role,
            dlci: DLCI::try_from(0).unwrap(),
            data: FrameData::UnnumberedInfoHeaderCheck(UIHData::Mux(expected_mux_command)),
            poll_final: true,
            command_response: CommandResponse::Response,
            credits: None,
        };
        assert_eq!(res, expected_frame);
    }

    #[test]
    fn test_parse_uih_frame_with_credits() {
        let role = Role::Initiator;
        let frame_type = FrameTypeMarker::UnnumberedInfoHeaderCheck;
        let credit_based_flow = true;
        let mut buf = vec![
            0b00011111, // Address Field - EA = 1, C/R = 1, User DLCI = 7.
            0b11111111, // Control Field - UIH command with P/F = 1.
            0b00000111, // Length Field - Bit1 = 1 Indicates one octet length = 3.
            0b00000101, // Credits Field = 5.
            0b00000000, // UserData octet #1.
            0b00000001, // UserData octet #2.
            0b00000010, // UserData octet #3.
        ];
        // Calculate the FCS for the first two bytes, since UIH frame.
        let fcs = calculate_fcs(&buf[..frame_type.fcs_octets()]);
        buf.push(fcs);

        let res = Frame::parse(role, credit_based_flow, &buf[..]).unwrap();
        let expected_user_data = UserData { information: vec![0x00, 0x01, 0x02] };
        let expected_frame = Frame {
            role,
            dlci: DLCI::try_from(7).unwrap(),
            data: FrameData::UnnumberedInfoHeaderCheck(UIHData::User(expected_user_data)),
            poll_final: true,
            command_response: CommandResponse::Command,
            credits: Some(5),
        };
        assert_eq!(res, expected_frame);
    }

    #[test]
    fn test_encode_frame_invalid_buf() {
        let frame = Frame {
            role: Role::Unassigned,
            dlci: DLCI::try_from(0).unwrap(),
            data: FrameData::SetAsynchronousBalancedMode,
            poll_final: false,
            command_response: CommandResponse::Command,
            credits: None,
        };
        let mut buf = [];
        assert_matches!(frame.encode(&mut buf[..]), Err(FrameParseError::BufferTooSmall));
    }

    /// Tests that attempting to encode a Mux Startup frame over a user DLCI is rejected.
    #[test]
    fn test_encode_mux_startup_frame_over_user_dlci_fails() {
        let frame = Frame {
            role: Role::Unassigned,
            dlci: DLCI::try_from(3).unwrap(),
            data: FrameData::SetAsynchronousBalancedMode,
            poll_final: false,
            command_response: CommandResponse::Command,
            credits: None,
        };
        let mut buf = vec![0; frame.encoded_len()];
        assert_matches!(frame.encode(&mut buf[..]), Err(FrameParseError::InvalidFrame));
    }

    #[test]
    fn encode_mux_startup_command_succeeds() {
        let frame = Frame {
            role: Role::Unassigned,
            dlci: DLCI::try_from(0).unwrap(),
            data: FrameData::SetAsynchronousBalancedMode,
            poll_final: true,
            command_response: CommandResponse::Command,
            credits: None,
        };
        let mut buf = vec![0; frame.encoded_len()];
        assert!(frame.encode(&mut buf[..]).is_ok());
        let expected = vec![
            0b00000011, // Address Field: DLCI = 0, C/R = 1, E/A = 1.
            0b00111111, // Control Field: SABM, P/F = 1.
            0b00000001, // Length Field: Length = 0, E/A = 1.
            0b00011100, // FCS - precomputed.
        ];
        assert_eq!(buf, expected);
    }

    #[test]
    fn encode_mux_startup_response_succeeds() {
        let frame = Frame::make_ua_response(Role::Unassigned, DLCI::try_from(0).unwrap());
        let mut buf = vec![0; frame.encoded_len()];
        assert!(frame.encode(&mut buf[..]).is_ok());
        let expected = vec![
            0b00000011, // Address Field: DLCI = 0, C/R = 1, E/A = 1.
            0b01110011, // Control Field: UA, P/F = 1.
            0b00000001, // Length Field: Length = 0, E/A = 1.
            0b11010111, // FCS - precomputed.
        ];
        assert_eq!(buf, expected);
    }

    #[test]
    fn encode_user_data_as_initiator_succeeds() {
        let frame = Frame::make_user_data_frame(
            Role::Initiator,
            DLCI::try_from(3).unwrap(),
            UserData {
                information: vec![
                    0b00000001, // Data octet #1.
                    0b00000010, // Data octet #2.
                ],
            },
            Some(8),
        );
        let mut buf = vec![0; frame.encoded_len()];
        assert!(frame.encode(&mut buf[..]).is_ok());
        let expected = vec![
            0b00001111, // Address Field: DLCI = 3, C/R = 1, E/A = 1.
            0b11111111, // Control Field - UIH command with P/F = 1.
            0b00000101, // Length Field - Bit1 = 1 Indicates one octet, length = 2.
            0b00001000, // Credit Field - Credits = 8.
            0b00000001, // Data octet #1.
            0b00000010, // Data octet #2.
            0b11110011, // FCS - precomputed.
        ];
        assert_eq!(buf, expected);
    }

    #[test]
    fn test_encode_user_data_as_responder_succeeds() {
        let frame = Frame::make_user_data_frame(
            Role::Responder,
            DLCI::try_from(9).unwrap(),
            UserData {
                information: vec![
                    0b00000001, // Data octet #1.
                ],
            },
            Some(10),
        );
        let mut buf = vec![0; frame.encoded_len()];
        assert!(frame.encode(&mut buf[..]).is_ok());
        let expected = vec![
            0b00100101, // Address Field: DLCI = 3, C/R = 0, E/A = 1.
            0b11111111, // Control Field - UIH command with P/F = 1.
            0b00000011, // Length Field - Bit1 = 1 Indicates one octet, length = 1.
            0b00001010, // Credit Field - Credits = 10.
            0b00000001, // Data octet #1.
            0b11101001, // FCS - precomputed.
        ];
        assert_eq!(buf, expected);
    }

    #[test]
    fn encode_mux_command_as_initiator() {
        let mux_command = MuxCommand {
            params: MuxCommandParams::ModemStatus(ModemStatusParams::default(
                DLCI::try_from(5).unwrap(),
            )),
            command_response: CommandResponse::Command,
        };
        let frame = Frame::make_mux_command(Role::Initiator, mux_command);

        let mut buf = vec![0; frame.encoded_len()];
        assert!(frame.encode(&mut buf[..]).is_ok());
        let expected = vec![
            0b00000011, // Address Field: DLCI = 0, C/R = 1, E/A = 1.
            0b11101111, // Control Field - UIH command with P/F = 1.
            0b00001001, // Length Field - Bit1 = 1 Indicates one octet, length = 4.
            0b11100011, // Data octet #1 - MSC response, C/R = 1, E/A = 1.
            0b00000101, // Data octet #2 - Length = 2, E/A = 1.
            0b00010111, // Data octet #3 DLCI = 5, E/A = 1, Bit2 = 1 always.
            0b10001101, // Data octet #4 Signals = default, E/A = 1.
            0b01110000, // FCS - precomputed.
        ];
        assert_eq!(buf, expected);
    }

    #[test]
    fn encode_mux_command_as_responder() {
        let mux_command = MuxCommand {
            params: MuxCommandParams::RemotePortNegotiation(RemotePortNegotiationParams {
                dlci: DLCI::try_from(7).unwrap(),
                port_values: None,
            }),
            command_response: CommandResponse::Command,
        };
        let frame = Frame::make_mux_command(Role::Responder, mux_command);

        let mut buf = vec![0; frame.encoded_len()];
        assert!(frame.encode(&mut buf[..]).is_ok());
        let expected = vec![
            0b00000001, // Address Field: DLCI = 0, C/R = 0, E/A = 1.
            0b11101111, // Control Field - UIH command with P/F = 1.
            0b00000111, // Length Field - Bit1 = 1 Indicates one octet, length = 3.
            0b10010011, // Data octet #1 - RPN command, C/R = 1, E/A = 1.
            0b00000011, // Data octet #2 - RPN Command length = 1.
            0b00011111, // Data octet #3 - RPN Data, DLCI = 7.
            0b10101010, // FCS - precomputed.
        ];
        assert_eq!(buf, expected);
    }

    #[test]
    fn encode_mux_response_as_initiator() {
        let mux_command = MuxCommand {
            params: MuxCommandParams::RemotePortNegotiation(RemotePortNegotiationParams {
                dlci: DLCI::try_from(13).unwrap(),
                port_values: None,
            }),
            command_response: CommandResponse::Response,
        };
        let frame = Frame::make_mux_command(Role::Initiator, mux_command);

        let mut buf = vec![0; frame.encoded_len()];
        assert!(frame.encode(&mut buf[..]).is_ok());
        let expected = vec![
            0b00000011, // Address Field: DLCI = 0, C/R = 1, E/A = 1.
            0b11101111, // Control Field - UIH command with P/F = 1.
            0b00000111, // Length Field - Bit1 = 1 Indicates one octet, length = 3.
            0b10010001, // Data octet #1 - RPN command, C/R = 0, E/A = 1.
            0b00000011, // Data octet #2 - RPN Command length = 1.
            0b00110111, // Data octet #3 - RPN Data, DLCI = 7.
            0b01110000, // FCS - precomputed.
        ];
        assert_eq!(buf, expected);
    }

    #[test]
    fn encode_mux_response_as_responder() {
        let mux_command = MuxCommand {
            params: MuxCommandParams::ModemStatus(ModemStatusParams::default(
                DLCI::try_from(11).unwrap(),
            )),
            command_response: CommandResponse::Response,
        };
        let frame = Frame::make_mux_command(Role::Responder, mux_command);

        let mut buf = vec![0; frame.encoded_len()];
        assert!(frame.encode(&mut buf[..]).is_ok());
        let expected = vec![
            0b00000001, // Address Field: DLCI = 0, C/R = 0, E/A = 1.
            0b11101111, // Control Field - UIH command with P/F = 1.
            0b00001001, // Length Field - Bit1 = 1 Indicates one octet, length = 4.
            0b11100001, // Data octet #1 - MSC response, C/R = 0, E/A = 1.
            0b00000101, // Data octet #2 - Length = 2, E/A = 1.
            0b00101111, // Data octet #3 DLCI = 11, E/A = 1, Bit2 = 1 always.
            0b10001101, // Data octet #4 Signals = default, E/A = 1.
            0b10101010, // FCS - precomputed.
        ];
        assert_eq!(buf, expected);
    }

    #[test]
    fn test_encode_user_data_with_two_octet_length_succeeds() {
        let length = 130;
        let mut information = vec![0; length];
        let frame = Frame {
            role: Role::Initiator,
            dlci: DLCI::try_from(5).unwrap(),
            data: FrameData::UnnumberedInfoHeaderCheck(UIHData::User(UserData {
                information: information.clone(),
            })),
            poll_final: true,
            command_response: CommandResponse::Command,
            credits: Some(8),
        };
        let mut buf = vec![0; frame.encoded_len()];
        assert!(frame.encode(&mut buf[..]).is_ok());
        let mut expected = vec![
            0b00010111, // Address Field: DLCI = 5, C/R = 1, E/A = 1.
            0b11111111, // Control Field - UIH command with P/F = 1.
            0b00000100, // Length Field - E/A = 0. Length = 2.
            0b00000001, // Length Field2 - 128.
            0b00001000, // Credit Field - Credits = 8.
        ];
        // Add the information.
        expected.append(&mut information);
        // Add the precomputed FCS.
        expected.push(0b0000_1100);
        assert_eq!(buf, expected);
    }

    #[test]
    fn test_parse_frame_with_invalid_fcs() {
        let role = Role::Responder;
        let frame_type = FrameTypeMarker::SetAsynchronousBalancedMode;
        let mut buf = vec![
            0b00001111, // Address Field - EA = 1, C/R = 1, User DLCI = 3.
            0b00101111, // Control Field - SABM command with P/F = 0.
            0b00000001, // Length Field - Bit1 = 1 Indicates one octet length - no info.
        ];
        // Calculate the correct FCS.
        let correct_fcs = calculate_fcs(&buf[..frame_type.fcs_octets()]);
        // Mutate the FCS to make it invalid.
        let invalid_fcs = correct_fcs ^ 0xFF;
        buf.push(invalid_fcs);

        assert_matches!(Frame::parse(role, false, &buf[..]), Err(FrameParseError::FCSCheckFailed));
    }

    #[test]
    fn encode_oversized_user_data_fails() {
        let length = 32_768;
        let information = vec![0; length];
        let frame = Frame {
            role: Role::Initiator,
            dlci: DLCI::try_from(5).unwrap(),
            data: FrameData::UnnumberedInfoHeaderCheck(UIHData::User(UserData { information })),
            poll_final: true,
            command_response: CommandResponse::Command,
            credits: None,
        };
        let mut buf = vec![0; frame.encoded_len()];
        assert_matches!(frame.encode(&mut buf[..]), Err(FrameParseError::FrameTooLarge(32768)));
    }
}
