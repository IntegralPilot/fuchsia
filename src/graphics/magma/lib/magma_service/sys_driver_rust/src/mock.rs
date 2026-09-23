// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::magma_system_connection::MagmaStatus;
use crate::traits;
use crate::traits::NotificationHandler;
use std::sync::Arc;

pub struct MockDriver;
impl traits::Driver for MockDriver {
    fn configure(&self, _flags: u32) {}

    fn import_semaphore(
        &self,
        _handle: zx::NullableHandle,
        _client_id: u64,
        _flags: u64,
    ) -> Result<Box<dyn traits::Semaphore>, MagmaStatus> {
        Ok(Box::new(MockSemaphore))
    }
}

pub struct MockDevice;
impl traits::Device for MockDevice {
    fn set_memory_pressure_level(&self, _level: u32) {}
    fn query(&self, _id: u64) -> Result<(Option<zx::Vmo>, u64), MagmaStatus> {
        Err(MagmaStatus::InternalError)
    }
    fn get_icd_list(&self) -> Result<Vec<crate::traits::MsdIcdInfo>, MagmaStatus> {
        Ok(vec![])
    }
    fn set_power_state(&self, _power_state: i64, _callback: Box<dyn FnOnce(i32) + Send>) {}
    fn dump_status(&self, _dump_flags: u32) {}
    fn open(
        &self,
        _client_id: u64,
        _client_type: traits::MagmaClientType,
        _notification_handler: Arc<dyn NotificationHandler>,
    ) -> Option<Box<dyn traits::Connection>> {
        Some(Box::new(MockConnection))
    }
}

pub struct MockConnection;
impl traits::Connection for MockConnection {
    fn create_context_2(&self, _priority: u64) -> Result<Box<dyn traits::Context>, MagmaStatus> {
        Ok(Box::new(MockContext))
    }
    fn map_buffer(
        &self,
        _buffer: &dyn traits::Buffer,
        _hw_va: u64,
        _offset: u64,
        _length: u64,
        _flags: u64,
    ) -> Result<(), MagmaStatus> {
        Ok(())
    }
    fn unmap_buffer(&self, _buffer: &dyn traits::Buffer, _hw_va: u64) -> Result<(), MagmaStatus> {
        Ok(())
    }
    fn buffer_range_op(
        &self,
        _buffer: &dyn traits::Buffer,
        _op: u32,
        _start: u64,
        _length: u64,
    ) -> Result<(), MagmaStatus> {
        Ok(())
    }
    fn enable_performance_counters(&self, _counters: Vec<u64>) -> Result<(), MagmaStatus> {
        Ok(())
    }
    fn create_performance_counter_buffer_pool(&self, _pool_id: u64) -> Result<(), MagmaStatus> {
        Ok(())
    }
    fn release_performance_counter_buffer_pool(&self, _pool_id: u64) -> Result<(), MagmaStatus> {
        Ok(())
    }
    fn add_performance_counter_buffer_offset_to_pool(
        &self,
        _pool_id: u64,
        _buffer_id: u64,
        _offset: u64,
        _size: u64,
    ) -> Result<(), MagmaStatus> {
        Ok(())
    }
    fn remove_performance_counter_buffer_from_pool(
        &self,
        _pool_id: u64,
        _buffer_id: u64,
    ) -> Result<(), MagmaStatus> {
        Ok(())
    }
    fn dump_performance_counters(
        &self,
        _pool_id: u64,
        _trigger_id: u32,
    ) -> Result<(), MagmaStatus> {
        Ok(())
    }
    fn clear_performance_counters(&self, _counters: Vec<u64>) -> Result<(), MagmaStatus> {
        Ok(())
    }
    fn release_buffer(&self, _buffer: &dyn traits::Buffer, _shutting_down: bool) {}
    fn import_buffer(
        &self,
        vmo: zx::Vmo,
        _client_id: u64,
        _flags: u64,
    ) -> Result<Box<dyn traits::Buffer>, MagmaStatus> {
        Ok(Box::new(MockBuffer { vmo }))
    }
}

pub struct MockContext;
impl traits::Context for MockContext {
    fn execute_command_buffers(
        &self,
        _command_buffers: Vec<crate::magma_system_context::MagmaExecCommandBuffer>,
        _resources: Vec<crate::magma_system_context::MagmaExecResource>,
        _buffers: Vec<&dyn traits::Buffer>,
        _wait_semaphores: Vec<&dyn traits::Semaphore>,
        _signal_semaphores: Vec<&dyn traits::Semaphore>,
    ) -> Result<(), MagmaStatus> {
        Ok(())
    }
}

pub struct MockBuffer {
    pub vmo: zx::Vmo,
}
impl traits::Buffer for MockBuffer {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct MockSemaphore;
impl traits::Semaphore for MockSemaphore {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
