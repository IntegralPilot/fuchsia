// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::magma_system_buffer::MagmaSystemBuffer;
use crate::magma_system_connection::MagmaStatus;
use crate::magma_system_semaphore::MagmaSystemSemaphore;
use crate::traits;

#[derive(Debug, Copy, Clone)]
pub struct MagmaExecCommandBuffer {
    pub resource_index: u32,
    pub start_offset: u64,
}

#[derive(Debug, Copy, Clone)]
pub struct MagmaExecResource {
    pub buffer_id: u64,
    pub offset: u64,
    pub length: u64,
}

pub struct MagmaSystemContext {
    msd_ctx: Box<dyn traits::Context>,
}

impl MagmaSystemContext {
    pub fn new(msd_ctx: Box<dyn traits::Context>) -> Self {
        MagmaSystemContext { msd_ctx }
    }

    pub fn execute_command_buffers(
        &self,
        command_buffers: Vec<MagmaExecCommandBuffer>,
        resources: Vec<MagmaExecResource>,
        buffers: Vec<&MagmaSystemBuffer>,
        wait_semaphores: Vec<&MagmaSystemSemaphore>,
        signal_semaphores: Vec<&MagmaSystemSemaphore>,
    ) -> Result<(), MagmaStatus> {
        let msd_buffers: Vec<&dyn traits::Buffer> =
            buffers.iter().map(|b| b.msd_buffer()).collect();

        let msd_wait_semaphores: Vec<&dyn traits::Semaphore> =
            wait_semaphores.iter().map(|s| s.msd_semaphore()).collect();

        let msd_signal_semaphores: Vec<&dyn traits::Semaphore> =
            signal_semaphores.iter().map(|s| s.msd_semaphore()).collect();

        self.msd_ctx.execute_command_buffers(
            command_buffers,
            resources,
            msd_buffers,
            msd_wait_semaphores,
            msd_signal_semaphores,
        )
    }
}
