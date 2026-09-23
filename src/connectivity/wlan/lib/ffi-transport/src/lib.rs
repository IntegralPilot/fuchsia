// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod completers;
mod transport;

pub use transport::{
    EthernetRx, EthernetTx, EthernetTxEvent, EthernetTxEventSender, FfiEthernetRx, FfiEthernetTx,
    FfiWlanRx, FfiWlanTx, WlanRx, WlanRxEvent, WlanRxEventSender, WlanTx,
};
