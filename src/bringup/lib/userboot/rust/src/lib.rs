// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx::sys::zx_handle_t;
use zx::{Channel, HandleInfo, NullableHandle, Status};

unsafe extern "C" {
    fn TakeBootstrapChannel() -> zx_handle_t;
}

/// This transfers ownership of the channel from the kernel where the message
/// full of system handles can be read.
pub fn take_bootstrap_channel() -> Channel {
    unsafe { NullableHandle::from_raw(TakeBootstrapChannel()) }.into()
}

// Re-export these for general use.
pub use zx_libc::sanitizer::{Log, log};

/// This reads the system capability message from the bootstrap channel.
/// It is mutually exclusive with take_bootstrap_channel().
pub fn take_system_handles() -> Result<Vec<HandleInfo>, Status> {
    let mut handles = Vec::new();
    let channel = take_bootstrap_channel();
    let mut bytes = Vec::new();
    channel.read_etc_split(&mut bytes, &mut handles)?;
    assert!(bytes.is_empty());
    Ok(handles)
}
