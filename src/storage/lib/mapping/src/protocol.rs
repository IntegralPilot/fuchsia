// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module defines the communication protocol over the VMOs shared between the filesystem
//! (Fxfs), the block driver (`block_server`), and the client/verifier (e.g., `pkg-cache`). The
//! two VMOs (mapping queue and delivery queue) are provided to
//! `fuchsia.storage.block/Mapper.OpenSession` to initialize a session.
//!
//! Two main channels of communication exist for paging:
//!
//! 1. **Mapping Queue (`RawMappingCommand`)**:
//!    Direction: Filesystem (Fxfs) -> Driver (Established via the client during initialization)
//!    Purpose: The filesystem informs the driver of where a file resides on the storage device
//!    (its extents). When the driver receives a page fault from the kernel for a specific `key`,
//!    it consults this mapping to know which blocks to read from disk.
//!
//! 2. **Delivery Queue (`RawDeliveryCommand`)**:
//!    Direction: Driver -> Client/Verifier (e.g., `pkg-cache`, which also acts as the Pager)
//!    Purpose: After the driver reads (and decompresses) the requested blocks, it writes the
//!    data into the delivery queue VMO and sends a delivery command. The verifier receives this
//!    command, cryptographically verifies the payload against the Merkle tree, and then supplies
//!    it to the kernel pager (`zx_pager_supply_pages`). The blob's Merkle leaf data is also
//!    transported via the delivery queue when the driver fetches and caches the metadata.

use anyhow::{Error, anyhow};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub const MAPPINGS_COMMAND: u32 = 1;
pub const CLOSE_BLOB_COMMAND: u32 = 2;

/// Flag indicating that the mapping payload contains an encryption key.
pub const MAPPINGS_FLAG_ENCRYPTED: u32 = 1 << 16;

/// Size of the encryption key in bytes (256 bits).
pub const ENCRYPTION_KEY_SIZE: usize = 32;

// The `vmo-fifo` divides the VMO into two regions: a fixed-size command slots region, and a
// dynamically allocated payload region where the actual extents are written.
//
// The following is the layout for a 512KB VMO with 256 capacity:
// [ Headers (64B) | Command Slots: 256 * 40B = 10,240B | .. Padding to 16KB .. | Payload (496KB) ]
// Note: Each command slot takes 40 bytes for `RawMappingCommand`.
//
// 496KB / 8-bytes per extent = 63,488 maximum extents bounded by the payload block.
pub const MAPPING_VMO_SIZE: u64 = 512 * 1024;

// With a maximum capacity of 256 pending mapping commands, this allows for an average of ~248
// extents per blob. In the worst case of maximum fragmentation (every 4KB block maps to one
// extent), 63,488 extents can map up to ~248MB of blob data (or ~496MB if block size is 8KB).
pub const PENDING_COMMANDS_CAPACITY: u32 = 256;

/// A command packet used to communicate extent mappings.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct RawMappingCommand {
    pub opcode: u32,
    pub offset: u32,
    pub key: u64,
    pub stored_size: u64,
    pub device_offset: u64,
    pub metadata_count: u32,
    pub extent_count: u32,
}

impl RawMappingCommand {
    /// Returns the command opcode without flags.
    pub fn opcode(&self) -> u32 {
        self.opcode & 0xffff
    }

    /// Returns whether the command has the encrypted flag set.
    pub fn is_encrypted(&self) -> bool {
        (self.opcode & MAPPINGS_FLAG_ENCRYPTED) != 0
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MappingCommand {
    /// Informs the driver of the extent mappings for a file.
    /// The VMO payload contains `extent_count` data extent mappings followed by `metadata_count`
    /// Merkle extent mappings, and optionally a 32-byte encryption key if `encrypted` is true.
    Mappings {
        /// Session-unique identifier for the file.
        key: u64,
        /// Byte offset within the shared VMO where the extent descriptors begin.
        offset: u32,
        /// Total stored size of the file's data (compressed size if compressed, or byte size).
        stored_size: u64,
        /// Base physical device byte offset on the underlying storage device.
        device_offset: u64,
        /// Number of Merkle tree metadata extent mappings.
        metadata_count: u32,
        /// Number of file data extent mappings.
        extent_count: u32,
        /// Whether the file is encrypted. If true, the payload contains a 32-byte key following
        /// the extent descriptors.
        encrypted: bool,
    },
    /// Informs the driver that the file session is closed and mappings can be discarded.
    CloseBlob {
        /// Session-unique identifier for the file.
        key: u64,
    },
}

impl From<MappingCommand> for RawMappingCommand {
    fn from(cmd: MappingCommand) -> Self {
        match cmd {
            MappingCommand::Mappings {
                key,
                offset,
                stored_size,
                device_offset,
                metadata_count,
                extent_count,
                encrypted,
            } => RawMappingCommand {
                opcode: MAPPINGS_COMMAND | if encrypted { MAPPINGS_FLAG_ENCRYPTED } else { 0 },
                offset,
                key,
                stored_size,
                device_offset,
                metadata_count,
                extent_count,
            },
            MappingCommand::CloseBlob { key } => RawMappingCommand {
                opcode: CLOSE_BLOB_COMMAND,
                offset: 0,
                key,
                stored_size: 0,
                device_offset: 0,
                metadata_count: 0,
                extent_count: 0,
            },
        }
    }
}

impl TryFrom<RawMappingCommand> for MappingCommand {
    type Error = Error;

    fn try_from(cmd: RawMappingCommand) -> Result<Self, Self::Error> {
        let opcode = cmd.opcode & 0xffff;
        let encrypted = (cmd.opcode & MAPPINGS_FLAG_ENCRYPTED) != 0;
        let unknown_flags = cmd.opcode & !(0xffff | MAPPINGS_FLAG_ENCRYPTED);
        if unknown_flags != 0 {
            return Err(anyhow!("Unknown flags in opcode: {:#x}", cmd.opcode));
        }
        match opcode {
            MAPPINGS_COMMAND => Ok(MappingCommand::Mappings {
                key: cmd.key,
                offset: cmd.offset,
                stored_size: cmd.stored_size,
                device_offset: cmd.device_offset,
                metadata_count: cmd.metadata_count,
                extent_count: cmd.extent_count,
                encrypted,
            }),
            CLOSE_BLOB_COMMAND => {
                if encrypted {
                    return Err(anyhow!("Encrypted flag not allowed for CloseBlob"));
                }
                Ok(MappingCommand::CloseBlob { key: cmd.key })
            }
            _ => Err(anyhow!("Unknown opcode: {}", cmd.opcode)),
        }
    }
}

pub const DELIVERY_DATA_COMMAND: u32 = 1;
pub const DELIVERY_REGISTER_BLOB_COMMAND: u32 = 2;

// The Delivery Queue ring buffer holds `RawDeliveryCommand` structures, which are 32 bytes each.
// If we establish an 8MB (8,388,608 bytes) VMO with a capacity of 256 pending delivery commands,
// the struct sizes and capacities stack dynamically bounding to the exact VMO wall, aligning
// the payload naturally to a 4KB hardware page boundary for zero-copy kernel transfers:
//
// Offsets: 0        64            8,256                   12,288                          8,388,608
// Layout:  | Header |  Cmd Slots  |  Padding to 4KB page  | Payload (Data & Merkle leaves)|
// Sizes:   |  64 B  |  8,192 B    |       4,032 B         |         8,376,320 B           |
//
// At an 8MB VMO size, the payload space allows for an average of ~32KB per chunk/leaf block
// in-flight for 256 outstanding commands.
pub const DELIVERY_VMO_SIZE: u64 = 8 * 1024 * 1024;
pub const PENDING_DELIVERY_COMMANDS_CAPACITY: u32 = 256;

/// The chunk size requirement for delivering payloads from the block driver across the delivery
/// queue. The driver must supply [`DeliveryCommand::Data`] chunks where the `target_offset` and
/// `length` align to `DELIVERY_DATA_SIZE` boundaries (unless representing the final chunk of the
/// blob).
///
/// This chunk size is set as 128 KiB size, matching Fxfs's target read-ahead size. This is used to
/// set the read size of `fuchsia_merkle::ReadSizedMerkleVerifier`, which optimizes memory usage
/// when verifying reads. See `fuchsia_merkle::ReadSizedMerkleVerifier` for more information.
pub const DELIVERY_DATA_SIZE: usize = 128 * 1024;

/// A command packet used by the driver to deliver merkle leaves and data chunks for verification.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct RawDeliveryCommand {
    pub opcode: u32,
    pub _padding: u32,
    pub key: u64,
    pub target_offset: u64,
    pub length: u32,
    pub offset: u32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum DeliveryCommand {
    /// Informs the verifier that data has been read and decompressed into the delivery queue VMO,
    /// ready for verification.
    Data {
        /// Identifies the blob.
        key: u64,
        /// The logical byte offset of this data chunk in the target VMO.
        target_offset: u64,
        /// The length of the data chunk.
        length: u32,
        /// The offset in this delivery queue where this data chunk resides.
        offset: u32,
    },
    /// Informs the verifier that the blob's Merkle tree metadata has been transferred to the queue.
    RegisterBlob {
        /// Identifies the blob.
        key: u64,
        /// The offset in this delivery queue where the Merkle leaves reside.
        offset: u32,
        /// The length of the Merkle leaf data in bytes.
        length: u32,
    },
}

impl From<DeliveryCommand> for RawDeliveryCommand {
    fn from(cmd: DeliveryCommand) -> Self {
        match cmd {
            DeliveryCommand::Data { key, target_offset, length, offset } => RawDeliveryCommand {
                opcode: DELIVERY_DATA_COMMAND,
                _padding: 0,
                key,
                target_offset,
                length,
                offset,
            },
            DeliveryCommand::RegisterBlob { key, offset, length } => RawDeliveryCommand {
                opcode: DELIVERY_REGISTER_BLOB_COMMAND,
                _padding: 0,
                key,
                target_offset: 0,
                length,
                offset,
            },
        }
    }
}

impl TryFrom<RawDeliveryCommand> for DeliveryCommand {
    type Error = Error;

    fn try_from(cmd: RawDeliveryCommand) -> Result<Self, Self::Error> {
        match cmd.opcode {
            DELIVERY_DATA_COMMAND => Ok(DeliveryCommand::Data {
                key: cmd.key,
                target_offset: cmd.target_offset,
                length: cmd.length,
                offset: cmd.offset,
            }),
            DELIVERY_REGISTER_BLOB_COMMAND => Ok(DeliveryCommand::RegisterBlob {
                key: cmd.key,
                offset: cmd.offset,
                length: cmd.length,
            }),
            _ => Err(anyhow!("Unknown opcode: {}", cmd.opcode)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mappings_command_round_trip() {
        let cmd = MappingCommand::Mappings {
            key: 123,
            offset: 456,
            stored_size: 789,
            device_offset: 1011,
            metadata_count: 2,
            extent_count: 3,
            encrypted: false,
        };
        let raw = RawMappingCommand::from(cmd);
        assert_eq!(raw.opcode(), MAPPINGS_COMMAND);
        assert!(!raw.is_encrypted());
        assert_eq!(MappingCommand::try_from(raw).unwrap(), cmd);
    }

    #[test]
    fn test_mappings_command_encrypted_round_trip() {
        let cmd = MappingCommand::Mappings {
            key: 123,
            offset: 456,
            stored_size: 789,
            device_offset: 1011,
            metadata_count: 2,
            extent_count: 3,
            encrypted: true,
        };
        let raw = RawMappingCommand::from(cmd);
        assert_eq!(raw.opcode(), MAPPINGS_COMMAND);
        assert!(raw.is_encrypted());
        assert_eq!(MappingCommand::try_from(raw).unwrap(), cmd);
    }

    #[test]
    fn test_close_blob_command_round_trip() {
        let cmd = MappingCommand::CloseBlob { key: 42 };
        let raw = RawMappingCommand::from(cmd);
        assert_eq!(raw.opcode(), CLOSE_BLOB_COMMAND);
        assert!(!raw.is_encrypted());
        assert_eq!(MappingCommand::try_from(raw).unwrap(), cmd);
    }

    #[test]
    fn test_close_blob_encrypted_flag_rejected() {
        let raw = RawMappingCommand {
            opcode: CLOSE_BLOB_COMMAND | MAPPINGS_FLAG_ENCRYPTED,
            offset: 0,
            key: 42,
            stored_size: 0,
            device_offset: 0,
            metadata_count: 0,
            extent_count: 0,
        };
        assert!(MappingCommand::try_from(raw).is_err());
    }

    #[test]
    fn test_unknown_opcode_rejected() {
        let raw = RawMappingCommand {
            opcode: 99,
            offset: 0,
            key: 42,
            stored_size: 0,
            device_offset: 0,
            metadata_count: 0,
            extent_count: 0,
        };
        assert!(MappingCommand::try_from(raw).is_err());
    }

    #[test]
    fn test_unknown_flags_rejected() {
        let raw = RawMappingCommand {
            opcode: MAPPINGS_COMMAND | (1 << 31),
            offset: 0,
            key: 42,
            stored_size: 0,
            device_offset: 0,
            metadata_count: 0,
            extent_count: 0,
        };
        assert!(MappingCommand::try_from(raw).is_err());
    }
}
