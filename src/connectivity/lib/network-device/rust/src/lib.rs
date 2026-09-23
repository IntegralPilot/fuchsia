// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Client library for [`fidl_fuchsia_hardware_network`]. Contains helpers
//! for sending frames to and receiving frames from L2/L3 network devices.

#![deny(missing_docs)]
pub mod client;
pub mod error;
pub mod port_slab;
pub mod session;

pub use client::{Client, DevicePortEvent, PortStatus};
pub use error::{Error, Result};
pub use port_slab::PortSlab;
pub use session::{
    Buffer, ChecksumRxOffloading, Config, DerivableConfig, DeviceInfo, Port, Rx, RxLease,
    RxMetadata, RxReadyStorage, Session, SinglePartTxBuffer, Task, Tx, TxMetadataMut,
};

/// Alias for receive buffer.
pub type RxBuffer = Buffer<Rx>;
/// Alias for transmit buffer.
pub type TxBuffer = Buffer<Tx>;
