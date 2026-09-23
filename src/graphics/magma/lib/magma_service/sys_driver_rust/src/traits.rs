// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::magma_system_connection::MagmaStatus;
use crate::magma_system_context::{MagmaExecCommandBuffer, MagmaExecResource};
use std::sync::Arc;
use zx;

// This struct represents all the information about the driver that the library needs to interact
// with. The implementation of this struct is driver-specific.
pub trait Driver: Send + Sync {
    fn configure(&self, flags: u32);
    fn import_semaphore(
        &self,
        handle: zx::NullableHandle,
        client_id: u64,
        flags: u64,
    ) -> Result<Box<dyn Semaphore>, MagmaStatus>;
}

#[repr(u32)]
pub enum IcdSupportFlags {
    Vulkan = 1,
    Opencl = 2,
    MediaCodecFactory = 4,
}

pub struct MsdIcdInfo {
    pub url: String,
    pub support_flags: IcdSupportFlags,
}

#[derive(Clone, Copy)]
pub enum MagmaClientType {
    Trusted,
    Untrusted,
}

// This represents a single hardware device.
pub trait Device: Send + Sync {
    fn set_memory_pressure_level(&self, level: u32);
    fn query(&self, id: u64) -> Result<(Option<zx::Vmo>, u64), MagmaStatus>;
    fn get_icd_list(&self) -> Result<Vec<MsdIcdInfo>, MagmaStatus>;
    // Sets the power state of the device. The given `callback` will be invoked asynchronously
    // when a power change is completed.
    fn set_power_state(&self, power_state: i64, callback: Box<dyn FnOnce(i32) + Send>);
    fn dump_status(&self, dump_flags: u32);
    fn open(
        &self,
        client_id: u64,
        client_type: MagmaClientType,
        notification_handler: Arc<dyn NotificationHandler>,
    ) -> Option<Box<dyn Connection>>;
}

pub trait NotificationHandler: Send + Sync {
    fn notification_channel_send(&self, data: &[u8]);
    fn context_killed(&self);
}

// This is a single connection from a client.
pub trait Connection {
    fn create_context_2(&self, priority: u64) -> Result<Box<dyn Context>, MagmaStatus>;
    fn map_buffer(
        &self,
        buffer: &dyn Buffer,
        hw_va: u64,
        offset: u64,
        length: u64,
        flags: u64,
    ) -> Result<(), MagmaStatus>;
    fn unmap_buffer(&self, buffer: &dyn Buffer, hw_va: u64) -> Result<(), MagmaStatus>;
    fn release_buffer(&self, buffer: &dyn Buffer, shutting_down: bool);
    fn buffer_range_op(
        &self,
        buffer: &dyn Buffer,
        op: u32,
        start: u64,
        length: u64,
    ) -> Result<(), MagmaStatus>;

    fn enable_performance_counters(&self, counters: Vec<u64>) -> Result<(), MagmaStatus>;
    fn create_performance_counter_buffer_pool(&self, pool_id: u64) -> Result<(), MagmaStatus>;
    fn release_performance_counter_buffer_pool(&self, pool_id: u64) -> Result<(), MagmaStatus>;
    fn add_performance_counter_buffer_offset_to_pool(
        &self,
        pool_id: u64,
        buffer_id: u64,
        offset: u64,
        size: u64,
    ) -> Result<(), MagmaStatus>;
    fn remove_performance_counter_buffer_from_pool(
        &self,
        pool_id: u64,
        buffer_id: u64,
    ) -> Result<(), MagmaStatus>;
    fn dump_performance_counters(&self, pool_id: u64, trigger_id: u32) -> Result<(), MagmaStatus>;
    fn clear_performance_counters(&self, counters: Vec<u64>) -> Result<(), MagmaStatus>;
    fn import_buffer(
        &self,
        vmo: zx::Vmo,
        client_id: u64,
        flags: u64,
    ) -> Result<Box<dyn Buffer>, MagmaStatus>;
}

// This represents a single hardware context that may execute commands.
pub trait Context {
    fn execute_command_buffers(
        &self,
        command_buffers: Vec<MagmaExecCommandBuffer>,
        resources: Vec<MagmaExecResource>,
        buffers: Vec<&dyn Buffer>,
        wait_semaphores: Vec<&dyn Semaphore>,
        signal_semaphores: Vec<&dyn Semaphore>,
    ) -> Result<(), MagmaStatus>;
}

pub trait Buffer: Send + Sync {
    fn as_any(&self) -> &dyn std::any::Any;
}

pub trait Semaphore: Send + Sync {
    fn as_any(&self) -> &dyn std::any::Any;
}

pub trait LogError {
    #[track_caller]
    fn log_err(self, str: impl std::fmt::Display) -> Self;

    #[track_caller]
    fn dlog_err(self, str: impl std::fmt::Display) -> Self
    where
        Self: Sized,
    {
        if cfg!(feature = "debug") { self.log_err(str) } else { self }
    }
}

impl<T, R> LogError for Result<T, R>
where
    R: std::fmt::Display,
{
    #[track_caller]
    fn log_err(self, str: impl std::fmt::Display) -> Self {
        match self {
            Ok(v) => Ok(v),
            Err(e) => {
                log::error!("{}: {}", str, e);
                Err(e)
            }
        }
    }
}
