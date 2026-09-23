// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Data type representing a server configuration.
/// Functional equivalent of [`otsys::otServerConfig`](crate::otsys::otServerConfig).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct ServerConfig(pub otServerConfig);

impl_ot_castable!(ServerConfig, otServerConfig);

impl ServerConfig {
    /// Length of server data.
    pub fn server_data_len(&self) -> u8 {
        self.0.mServerDataLength
    }

    /// Server data bytes.
    pub fn server_data(&self) -> [u8; 248usize] {
        self.0.mServerData
    }

    /// The Server RLOC16.
    pub fn rloc16(&self) -> u16 {
        self.0.mRloc16
    }

    /// Whether this config is considered Stable Network Data.
    pub fn is_stable(&self) -> bool {
        self.0.mStable()
    }
}
