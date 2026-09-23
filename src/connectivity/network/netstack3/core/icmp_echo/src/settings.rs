// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use netstack3_datagram::DatagramSettings;

/// ICMP echo socket settings.
#[derive(Clone)]
pub struct IcmpEchoSettings {
    /// Common datagram socket settings.
    pub datagram: DatagramSettings,
}

impl Default for IcmpEchoSettings {
    fn default() -> Self {
        Self { datagram: Default::default() }
    }
}

impl AsRef<DatagramSettings> for IcmpEchoSettings {
    fn as_ref(&self) -> &DatagramSettings {
        &self.datagram
    }
}
