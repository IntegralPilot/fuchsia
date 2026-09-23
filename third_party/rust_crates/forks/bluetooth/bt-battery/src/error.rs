// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::packet_encoding::Error as PacketError;
use bt_gatt::types::Handle;
use bt_gatt::types::{Error as GattLibraryError, GattError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("LE Scan for battery services has already started.")]
    ScanAlreadyStarted,

    #[error("GATT operation error: {0:?}")]
    Gatt(#[from] GattError),

    #[error("GATT library error: {0:?}")]
    GattLibrary(#[from] GattLibraryError),

    #[error("No compatible battery service found")]
    ServiceNotFound,

    #[error("Packet serialization/deserialization error: {0}")]
    Packet(#[from] PacketError),

    #[error("Malformed service on peer: {0}")]
    Service(#[from] ServiceError),

    #[error("Generic failure: {0}")]
    Generic(String),
}

/// Errors found when interacting with the remote peer's battery service.
#[derive(Debug, Error, PartialEq)]
pub enum ServiceError {
    #[error("Missing a required service characteristic")]
    MissingCharacteristic,

    #[error("Notification streams unexpectedly terminated")]
    NotificationStreamClosed,

    #[error("Notification received for an unsupported handle: {0:?}")]
    UnsupportedHandle(Handle),
}
