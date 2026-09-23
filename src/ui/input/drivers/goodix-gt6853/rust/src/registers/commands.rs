// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Host command codes, device status responses, and structured command packets for Goodix GT6853.

use crate::data_types::traits::{AddressableRegister, ReadableRegister, WritableRegister};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Real-time command and device status register.
///
/// Written by the host to send command codes, and read by the host to inspect
/// controller response and readiness states.
//
// @cite(gt6853-hardware-description): sec=3.2 title="Real-Time Command Registers"
// @cite(gt6853-programming-guide): sec=3.2 title="Real-Time Command Registers"
// @cite(gt6853-programming-guide): sec=4.4 title="Send Configuration"
// @cite(gt6853-programming-guide): sec=4.5 title="Read Configuration"
// @alias(gt6853-hardware-description): theirs="Command"
// @alias(gt6853-programming-guide): theirs="Command"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct Command(pub u8);

impl AddressableRegister for Command {
    const ADDRESS: u16 = 0x60CC;
}

impl ReadableRegister for Command {}
impl WritableRegister for Command {}

impl Command {
    // --- Host Command Codes (written by host) ---

    /// Notifies the controller to prepare for configuration writing.
    pub const WRITE_CONFIG_START: Self = Self(0x80);

    /// Notifies the controller that configuration writing is complete.
    pub const WRITE_CONFIG_END: Self = Self(0x83);

    /// Requests the controller to prepare configuration data for reading.
    pub const READ_CONFIG_START: Self = Self(0x86);

    /// Notifies the controller that configuration reading is complete.
    pub const READ_CONFIG_END: Self = Self(0xFF);

    // --- Controller Status Codes (read by host) ---

    /// Indicates the controller is ready to receive configuration data.
    pub const READY_FOR_CONFIG_WRITE: Self = Self(0x82);

    /// Indicates the controller has prepared configuration data and is ready for reading.
    pub const READY_FOR_CONFIG_READ: Self = Self(0x85);

    /// Indicates the controller is idle and not executing any command.
    pub const IDLE: Self = Self(0xFF);

    /// Returns `true` if this code matches a known host command definition.
    pub const fn is_known_host_command(&self) -> bool {
        matches!(
            *self,
            Self::WRITE_CONFIG_START
                | Self::WRITE_CONFIG_END
                | Self::READ_CONFIG_START
                | Self::READ_CONFIG_END
        )
    }

    /// Returns `true` if this code matches a known controller status definition.
    pub const fn is_known_controller_status(&self) -> bool {
        matches!(*self, Self::READY_FOR_CONFIG_WRITE | Self::READY_FOR_CONFIG_READ | Self::IDLE)
    }

    /// Returns `true` if this code matches any known host command or controller status definition.
    pub const fn is_known(&self) -> bool {
        self.is_known_host_command() || self.is_known_controller_status()
    }
}

/// Structured command packet sent to the controller.
///
/// Ensures the representation invariant that the packet checksum is always valid.
//
// @cite(gt6853-hardware-description): sec=3.2 title="Real-Time Command Registers"
// @cite(gt6853-programming-guide): sec=3.2 title="Real-Time Command Registers"
// @cite(gt6853-programming-guide): sec=4.4 title="Send Configuration"
// @cite(gt6853-programming-guide): sec=4.5 title="Read Configuration"
// @alias(gt6853-hardware-description): theirs="Command"
// @alias(gt6853-programming-guide): theirs="Command"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct CommandPacket {
    command: Command,
    data: u8,
    checksum: u8,
}

impl AddressableRegister for CommandPacket {
    const ADDRESS: u16 = 0x60CC;
}

impl WritableRegister for CommandPacket {}

impl CommandPacket {
    /// Constructs a new [`CommandPacket`] with an automatically computed hardware checksum.
    ///
    /// Set `data` to 0 when the command does not require an additional parameter byte.
    /// The checksum is computed as `0 - command - data` (modulo 256).
    pub const fn new(command: Command, data: u8) -> Self {
        // Hardware checksum algorithm: checksum = (0 - command - data) & 0xFF.
        let checksum = 0u8.wrapping_sub(command.0).wrapping_sub(data);
        Self { command, data, checksum }
    }

    pub const fn command(&self) -> Command {
        self.command
    }

    pub const fn data(&self) -> u8 {
        self.data
    }

    pub const fn checksum(&self) -> u8 {
        self.checksum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_packet_construction_with_zero_data() {
        let packet = CommandPacket::new(Command::WRITE_CONFIG_START, 0);
        assert_eq!(packet.command(), Command::WRITE_CONFIG_START);
        assert_eq!(packet.data(), 0);
        // (0 - 0x80 - 0) = 0x80
        assert_eq!(packet.checksum(), 0x80);
        assert_eq!(packet.as_bytes(), &[0x80, 0x00, 0x80]);

        let end_packet = CommandPacket::new(Command::WRITE_CONFIG_END, 0);
        // (0 - 0x83 - 0) = 0x7D
        assert_eq!(end_packet.checksum(), 0x7D);
        assert_eq!(end_packet.as_bytes(), &[0x83, 0x00, 0x7D]);
    }

    #[test]
    fn test_command_packet_construction_with_non_zero_data() {
        let packet = CommandPacket::new(Command::WRITE_CONFIG_START, 0x05);
        assert_eq!(packet.command(), Command::WRITE_CONFIG_START);
        assert_eq!(packet.data(), 0x05);
        // (0 - 0x80 - 0x05) = 0x7B
        assert_eq!(packet.checksum(), 0x7B);
        assert_eq!(packet.as_bytes(), &[0x80, 0x05, 0x7B]);
    }

    #[test]
    fn test_command_is_known() {
        // Host commands
        assert!(Command::WRITE_CONFIG_START.is_known_host_command());
        assert!(Command::WRITE_CONFIG_END.is_known_host_command());
        assert!(Command::READ_CONFIG_START.is_known_host_command());
        assert!(Command::READ_CONFIG_END.is_known_host_command());
        assert!(!Command::READY_FOR_CONFIG_WRITE.is_known_host_command());
        assert!(!Command(0x12).is_known_host_command());

        // Controller status codes
        assert!(Command::READY_FOR_CONFIG_WRITE.is_known_controller_status());
        assert!(Command::READY_FOR_CONFIG_READ.is_known_controller_status());
        assert!(Command::IDLE.is_known_controller_status());
        assert!(!Command::WRITE_CONFIG_START.is_known_controller_status());
        assert!(!Command(0x12).is_known_controller_status());

        // Generic is_known
        assert!(Command::WRITE_CONFIG_START.is_known());
        assert!(Command::WRITE_CONFIG_END.is_known());
        assert!(Command::READ_CONFIG_START.is_known());
        assert!(Command::READ_CONFIG_END.is_known());
        assert!(Command::READY_FOR_CONFIG_WRITE.is_known());
        assert!(Command::READY_FOR_CONFIG_READ.is_known());
        assert!(Command::IDLE.is_known());
        assert!(!Command(0x12).is_known());
    }
}
