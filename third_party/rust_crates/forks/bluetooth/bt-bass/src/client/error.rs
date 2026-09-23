// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

use bt_bap::types::BroadcastId;
use bt_common::packet_encoding::Error as PacketError;
use bt_gatt::types::{Error as BTGattError, Handle};

#[derive(Debug, Error)]
pub enum Error {
    #[error(
        "An unsupported opcode ({0:#x}) used in a Broadcast Audio Scan Control Point operation"
    )]
    OpCodeNotSupported(u8),

    #[error("Invalid source id ({0:#x}) used in a Broadcast Audio Scan Control Point operation")]
    InvalidSourceId(u8),

    #[error("Packet serialization/deserialization error: {0}")]
    Packet(PacketError),

    #[error("Malformed service on peer: {0}")]
    Service(ServiceError),

    #[error("GATT operation error: {0}")]
    Gatt(BTGattError),

    #[error("Error while polling BASS client event stream: {0}")]
    EventStream(Box<Error>),

    #[error("Broadcast source with id ({0:?}) is unknown")]
    UnknownBroadcastSource(BroadcastId),

    #[error("Failure occurred: {0}")]
    Generic(String),
}

/// This error represents an error we found at the remote BASS service or
/// we encountered during the interaction with the remote service which
/// prevents us from proceeding further with the client operation.
#[derive(Debug, Error, PartialEq)]
pub enum ServiceError {
    #[error("Missing a required service characteristic")]
    MissingCharacteristic,

    #[error("More than one Broadcast Audio Scan Control Point characteristics")]
    ExtraScanControlPointCharacteristic,

    #[error(
        "Failed to configure notification for Broadcast Recieve State characteristic (handle={0:?})"
    )]
    NotificationConfig(Handle),

    #[error("Broadcast Receive State notification channels closed unexpectedly: {0}")]
    NotificationChannelClosed(String),
}
