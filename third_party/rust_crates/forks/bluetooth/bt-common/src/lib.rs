// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

extern crate core as rust_core;

/// Peers are identified by ids, which should be treated as opaque by service
/// libraries. Stack implementations should ensure that each PeerId identifies a
/// single peer over a single instance of the stack - a
/// [`bt_gatt::Central::connect`] should always attempt to connect to the
/// same peer as long as the PeerId was retrieved after the `Central` was
/// instantiated. PeerIds can be valid longer than that (often if the peer is
/// bonded)
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PeerId(pub u64);

impl rust_core::fmt::Display for PeerId {
    fn fmt(
        &self,
        f: &mut rust_core::fmt::Formatter<'_>,
    ) -> std::result::Result<(), std::fmt::Error> {
        write!(f, "{:016x}", self.0)
    }
}

impl std::fmt::Debug for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PeerId").field(&format_args!("0x{}", self)).finish()
    }
}

pub mod core;

pub mod company_id;
pub use company_id::CompanyId;

pub mod generic_audio;

pub mod packet_encoding;

pub mod uuids;
pub use crate::uuids::Uuid;

pub mod debug_command;
