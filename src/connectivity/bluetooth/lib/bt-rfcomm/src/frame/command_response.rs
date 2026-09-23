// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::frame::error::FrameParseError;
use crate::frame::FrameTypeMarker;
use crate::Role;

/// The C/R bit in RFCOMM. This is used both at the frame level and the multiplexer
/// channel command level. See RFCOMM 5.1.3 and 5.4.6, respectively.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CommandResponse {
    Command,
    Response,
}

impl CommandResponse {
    /// Classifies a frame type as a Command (C) or Response (R).
    pub(crate) fn classify(
        role: Role,
        frame_type: FrameTypeMarker,
        cr_bit: bool,
    ) -> Result<Self, FrameParseError> {
        use FrameTypeMarker::*;
        // If the multiplexer has started, then the classification is determined by the frame type.
        if role.is_multiplexer_started() {
            let res = match (frame_type, role, cr_bit) {
                // For UIH frames, it is determined by the role.
                // This is defined in GSM 5.4.3.1, which is a subclause of GSM 5.2.1.2.
                (UnnumberedInfoHeaderCheck, Role::Initiator, _) => Ok(CommandResponse::Command),
                (UnnumberedInfoHeaderCheck, Role::Responder, _) => Ok(CommandResponse::Response),
                (UnnumberedInfoHeaderCheck, role, _) => Err(FrameParseError::InvalidRole(role)),
                // For all other frames, the logic is determined by both the role and C/R bit.
                // See GSM 5.2.1.2 Table 1 for the exact mapping.
                (_, Role::Initiator, true) | (_, Role::Responder, false) => {
                    Ok(CommandResponse::Command)
                }
                (_, _, _) => Ok(CommandResponse::Response),
            };
            return res;
        }

        // Otherwise, the classification depends `cr_bit` of the mux startup frame.
        match (frame_type, cr_bit) {
            (SetAsynchronousBalancedMode, true) => Ok(CommandResponse::Command),
            (SetAsynchronousBalancedMode, false) => Ok(CommandResponse::Response),
            (DisconnectedMode | UnnumberedAcknowledgement, true) => Ok(CommandResponse::Response),
            (DisconnectedMode | UnnumberedAcknowledgement, false) => Ok(CommandResponse::Command),
            (frame_type, _) => Err(FrameParseError::InvalidFrameBeforeMuxStartup(frame_type)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;

    #[test]
    fn classify_frame_after_mux_startup() {
        // Multiplexer started because a Role has been assigned.
        let role = Role::Initiator;
        let frame = FrameTypeMarker::SetAsynchronousBalancedMode;
        assert_matches!(
            CommandResponse::classify(role, frame, /* cr_bit= */ true),
            Ok(CommandResponse::Command)
        );

        let role = Role::Initiator;
        let frame = FrameTypeMarker::SetAsynchronousBalancedMode;
        assert_matches!(
            CommandResponse::classify(role, frame, /* cr_bit= */ false),
            Ok(CommandResponse::Response)
        );

        let role = Role::Responder;
        let frame = FrameTypeMarker::Disconnect;
        assert_matches!(
            CommandResponse::classify(role, frame, /* cr_bit= */ true),
            Ok(CommandResponse::Response)
        );

        let role = Role::Responder;
        let frame = FrameTypeMarker::DisconnectedMode;
        assert_matches!(
            CommandResponse::classify(role, frame, /* cr_bit= */ false),
            Ok(CommandResponse::Command)
        );
    }

    #[test]
    fn classify_uih_frame_after_mux_startup() {
        let frame = FrameTypeMarker::UnnumberedInfoHeaderCheck;

        assert_matches!(
            CommandResponse::classify(Role::Initiator, frame, /* cr_bit=*/ true),
            Ok(CommandResponse::Command)
        );

        assert_matches!(
            CommandResponse::classify(Role::Initiator, frame, /* cr_bit=*/ false),
            Ok(CommandResponse::Command)
        );

        assert_matches!(
            CommandResponse::classify(Role::Responder, frame, /* cr_bit=*/ true),
            Ok(CommandResponse::Response)
        );

        assert_matches!(
            CommandResponse::classify(Role::Responder, frame, /* cr_bit=*/ false),
            Ok(CommandResponse::Response)
        );
    }

    /// Tests classifying a SABM command when the multiplexer has not started. The classification
    /// should simply be based on the CR bit.
    #[test]
    fn classify_sabm_before_mux_startup() {
        // Mux not started.
        let role = Role::Unassigned;
        let frame = FrameTypeMarker::SetAsynchronousBalancedMode;
        let cr_bit = true;
        assert_matches!(
            CommandResponse::classify(role, frame, cr_bit),
            Ok(CommandResponse::Command)
        );

        // Mux not started.
        let role = Role::Negotiating;
        let frame = FrameTypeMarker::SetAsynchronousBalancedMode;
        let cr_bit = false;
        assert_matches!(
            CommandResponse::classify(role, frame, cr_bit),
            Ok(CommandResponse::Response)
        );
    }

    /// Tests classifying a DM before multiplexer startup. Should be opposite of C/R bit.
    #[test]
    fn classify_dm_before_mux_startup() {
        let frame = FrameTypeMarker::DisconnectedMode;

        // Mux not started.
        let role = Role::Negotiating;
        let cr_bit = true;
        assert_matches!(
            CommandResponse::classify(role, frame, cr_bit),
            Ok(CommandResponse::Response)
        );

        let role = Role::Unassigned;
        let cr_bit = false;
        assert_matches!(
            CommandResponse::classify(role, frame, cr_bit),
            Ok(CommandResponse::Command)
        );
    }

    #[test]
    fn classify_ua_before_mux_startup() {
        let frame = FrameTypeMarker::UnnumberedAcknowledgement;

        let role = Role::Unassigned;
        let cr_bit = true;
        assert_matches!(
            CommandResponse::classify(role, frame, cr_bit),
            Ok(CommandResponse::Response)
        );

        let role = Role::Negotiating;
        let cr_bit = false;
        assert_matches!(
            CommandResponse::classify(role, frame, cr_bit),
            Ok(CommandResponse::Command)
        );
    }

    #[test]
    fn classify_invalid_frame_before_mux_startup_is_error() {
        // Disconnect can't be sent before startup.
        let role = Role::Unassigned;
        let frame = FrameTypeMarker::Disconnect;
        let cr_bit = true;
        assert_matches!(
            CommandResponse::classify(role, frame, cr_bit),
            Err(FrameParseError::InvalidFrameBeforeMuxStartup(_))
        );

        // UIH can't be sent before startup.
        let role = Role::Unassigned;
        let frame = FrameTypeMarker::UnnumberedInfoHeaderCheck;
        let cr_bit = true;
        assert_matches!(
            CommandResponse::classify(role, frame, cr_bit),
            Err(FrameParseError::InvalidFrameBeforeMuxStartup(_))
        );
    }
}
