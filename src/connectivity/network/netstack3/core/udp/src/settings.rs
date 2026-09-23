// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use netstack3_datagram::DatagramSettings;

/// UDP layer settings.
#[derive(Clone)]
pub struct UdpSettings {
    /// Common datagram socket settings.
    pub datagram: DatagramSettings,
}

impl Default for UdpSettings {
    fn default() -> Self {
        Self { datagram: Default::default() }
    }
}

impl AsRef<DatagramSettings> for UdpSettings {
    fn as_ref(&self) -> &DatagramSettings {
        &self.datagram
    }
}
