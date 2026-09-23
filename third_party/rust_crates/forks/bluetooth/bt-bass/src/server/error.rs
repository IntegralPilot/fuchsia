// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

use crate::types::SourceId;
use bt_common::packet_encoding::Error as PacketError;
use bt_gatt::types::Error as BTGattError;

/// Errors encountered when operating a BASS GATT server.
#[derive(Debug, Error)]
pub enum Error {
    #[error("Service is already published")]
    AlreadyPublished,

    #[error("Service should support at least one Broadcast Receive State characteristic")]
    MissingReceiveState,

    #[error("Exceeded maximum number of Broadcast Receive State characteristics (255)")]
    ExceedsMaxReceiveStates,

    #[error("An unsupported opcode ({0:#x}) used in Control Point operation")]
    OpCodeNotSupported(u8),

    #[error("Invalid source id ({0}) used in Control Point operation")]
    InvalidSourceId(SourceId),

    #[error("Packet encoding/decoding error: {0}")]
    Packet(#[from] PacketError),

    #[error("GATT operation error: {0}")]
    Gatt(#[from] BTGattError),
}
