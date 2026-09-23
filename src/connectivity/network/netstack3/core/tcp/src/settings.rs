// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::num::NonZeroUsize;

use netstack3_base::BufferSizeSettings;

/// TCP layer stack settings.
#[derive(Clone)]
pub struct TcpSettings {
    /// Receive buffer settings.
    pub receive_buffer: BufferSizeSettings<NonZeroUsize>,
    /// Send buffer settings.
    pub send_buffer: BufferSizeSettings<NonZeroUsize>,
}

#[cfg(any(test, feature = "testutils"))]
impl Default for TcpSettings {
    fn default() -> Self {
        // Arbitrarily chosen to satisfy tests so we have some semblance of
        // clamping capacity in tests.

        let min = NonZeroUsize::new(16).unwrap();
        let max = NonZeroUsize::new(16 << 20).unwrap();
        let default = NonZeroUsize::new(netstack3_base::WindowSize::DEFAULT.into()).unwrap();
        let sizes = BufferSizeSettings::new(min, default, max).unwrap();
        Self { receive_buffer: sizes, send_buffer: sizes }
    }
}
