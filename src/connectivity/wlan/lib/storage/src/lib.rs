// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! High level node abstraction on top of Fuchsia's Stash service.
//! Allows to insert structured, hierarchical data into a Stash backed
//! store.

pub mod policy;
mod stash_store;
mod storage_store;

#[cfg(test)]
mod tests {
    use rand::distr::{Alphanumeric, SampleString as _};
    use rand::rng;
    use wlan_storage_constants::{NetworkIdentifier, SecurityType, StashedSsid};

    pub fn rand_string() -> String {
        Alphanumeric.sample_string(&mut rng(), 20)
    }

    pub fn network_id(
        ssid: impl Into<StashedSsid>,
        security_type: SecurityType,
    ) -> NetworkIdentifier {
        NetworkIdentifier { ssid: ssid.into(), security_type }
    }
}
