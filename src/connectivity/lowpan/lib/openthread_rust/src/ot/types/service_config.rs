// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Data type representing a service configuration.
/// Functional equivalent of [`otsys::otServiceConfig`](crate::otsys::otServiceConfig).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct ServiceConfig(pub otServiceConfig);

impl_ot_castable!(ServiceConfig, otServiceConfig);

impl ServiceConfig {
    /// IANA Enterprise Number.
    pub fn enterprise_number(&self) -> u32 {
        self.0.mEnterpriseNumber
    }

    /// The Server configuration.
    pub fn server_config(&self) -> &ServerConfig {
        (&self.0.mServerConfig).into()
    }

    /// Service data bytes.
    pub fn service_data(&self) -> [u8; 252usize] {
        self.0.mServiceData
    }

    /// Length of service data.
    pub fn service_data_len(&self) -> u8 {
        self.0.mServiceDataLength
    }

    /// Service ID (when iterating over the Network Data).
    pub fn service_id(&self) -> u8 {
        self.0.mServiceId
    }
}
