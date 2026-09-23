// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// The direct link connection identifier (DLCI) definitions.
mod dlci;
pub use dlci::{DLCI, ServerChannel};

/// The error type used throughout this library.
mod error;
pub use error::Error as RfcommError;

/// The definitions for RFCOMM frames - the basic unit of data in RFCOMM.
pub mod frame;

/// Convenience helpers for the `bredr.Profile` API RFCOMM operations.
pub mod profile;

/// The Role assigned to a device in an RFCOMM Session.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Role {
    /// RFCOMM Session has not started up the start-up procedure.
    Unassigned,
    /// The start-up procedure is in progress, and so the role is being negotiated.
    Negotiating,
    /// The device that starts up the multiplexer control channel is considered
    /// the initiator.
    Initiator,
    /// The device that responds to the start-up procedure.
    Responder,
}

impl Role {
    /// Returns the Role opposite to the current Role.
    pub fn opposite_role(&self) -> Self {
        match self {
            Role::Initiator => Role::Responder,
            Role::Responder => Role::Initiator,
            role => *role,
        }
    }

    /// Returns true if the multiplexer has started - namely, a role has been assigned.
    pub fn is_multiplexer_started(&self) -> bool {
        *self == Role::Initiator || *self == Role::Responder
    }
}

/// Returns the maximum RFCOMM packet size that can be used for the provided L2CAP `mtu`.
/// It is assumed that `mtu` is a valid L2CAP MTU.
pub fn max_packet_size_from_l2cap_mtu(mtu: u16) -> u16 {
    let max_from_mtu = mtu.saturating_sub(crate::frame::MAX_RFCOMM_HEADER_SIZE as u16);
    std::cmp::min(max_from_mtu, crate::frame::MAX_INFORMATION_LENGTH as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_to_opposite_role() {
        let role = Role::Initiator;
        assert_eq!(role.opposite_role(), Role::Responder);

        let role = Role::Responder;
        assert_eq!(role.opposite_role(), Role::Initiator);

        let role = Role::Unassigned;
        assert_eq!(role.opposite_role(), Role::Unassigned);

        let role = Role::Negotiating;
        assert_eq!(role.opposite_role(), Role::Negotiating);
    }

    #[test]
    fn max_packet_size_capped_for_large_l2cap_mtu() {
        let l2cap_mtu = 65535;
        assert_eq!(max_packet_size_from_l2cap_mtu(l2cap_mtu), 32767);
    }
}
