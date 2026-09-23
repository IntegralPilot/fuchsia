// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use netstack3_base::{BufferSizeSettings, PositiveIsize};

/// Settings common to datagram sockets.
#[derive(Debug, Copy, Clone)]
pub struct DatagramSettings {
    /// Send buffer size configuration.
    ///
    /// Note that the datagram crate does *NOT* keep the receive buffers for the
    /// socket, that is delegated to bindings.
    pub send_buffer: BufferSizeSettings<PositiveIsize>,
}

/// Maximum send buffer size. Value taken from Linux defaults.
const MAX_SEND_BUFFER_SIZE: PositiveIsize = PositiveIsize::new(4 * 1024 * 1024).unwrap();
/// Default send buffer size. Value taken from Linux defaults.
const DEFAULT_SEND_BUFFER_SIZE: PositiveIsize = PositiveIsize::new(208 * 1024).unwrap();
/// Minimum send buffer size. Value taken from Linux defaults.
const MIN_SEND_BUFFER_SIZE: PositiveIsize = PositiveIsize::new(4 * 1024).unwrap();

impl Default for DatagramSettings {
    fn default() -> Self {
        Self {
            send_buffer: BufferSizeSettings::new(
                MIN_SEND_BUFFER_SIZE,
                DEFAULT_SEND_BUFFER_SIZE,
                MAX_SEND_BUFFER_SIZE,
            )
            .unwrap(),
        }
    }
}

#[cfg(any(test, feature = "testutils"))]
impl AsRef<DatagramSettings> for DatagramSettings {
    fn as_ref(&self) -> &DatagramSettings {
        self
    }
}
