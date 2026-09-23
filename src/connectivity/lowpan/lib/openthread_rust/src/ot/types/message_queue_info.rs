// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// This structure represents message queue info.
///
/// Functional equivalent of [`otsys::otMessageQueueInfo`](crate::otsys::otMessageQueueInfo).
#[derive(Debug, Default, Copy, Clone)]
#[repr(transparent)]
pub struct MessageQueueInfo(pub otMessageQueueInfo);

impl_ot_castable!(MessageQueueInfo, otMessageQueueInfo);

impl MessageQueueInfo {
    /// Number of messages in the queue.
    pub fn num_messages(&self) -> u16 {
        self.0.mNumMessages
    }

    /// Number of buffers.
    pub fn num_buffers(&self) -> u16 {
        self.0.mNumBuffers
    }

    /// Total bytes.
    pub fn total_bytes(&self) -> u32 {
        self.0.mTotalBytes
    }
}
