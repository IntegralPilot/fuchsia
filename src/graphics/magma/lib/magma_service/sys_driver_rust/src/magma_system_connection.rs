// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::magma_system_buffer::MagmaSystemBuffer;
use crate::magma_system_context::{MagmaExecCommandBuffer, MagmaExecResource, MagmaSystemContext};
use crate::magma_system_semaphore::MagmaSystemSemaphore;
use crate::traits;
use crate::traits::LogError;
use fidl::endpoints::ServerEnd;
use std::sync::Arc;

use std::collections::HashMap;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MagmaObjectType {
    Buffer,
    Semaphore,
}

impl TryFrom<fidl_fuchsia_gpu_magma::ObjectType> for MagmaObjectType {
    type Error = fidl_fuchsia_gpu_magma::ObjectType;

    fn try_from(value: fidl_fuchsia_gpu_magma::ObjectType) -> Result<Self, Self::Error> {
        match value {
            fidl_fuchsia_gpu_magma::ObjectType::Buffer => Ok(MagmaObjectType::Buffer),
            fidl_fuchsia_gpu_magma::ObjectType::Semaphore => Ok(MagmaObjectType::Semaphore),
            _ => Err(value),
        }
    }
}

pub enum ConnectionMessage {
    ContextKilled,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContextId(pub u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BufferId(pub u64);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SemaphoreId(pub u64);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PerformanceId(pub u64);

#[repr(i32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, strum_macros::FromRepr)]
pub enum MagmaStatus {
    Ok = crate::magma_common_defs::MAGMA_STATUS_OK,
    InternalError = crate::magma_common_defs::MAGMA_STATUS_INTERNAL_ERROR,
    InvalidArgs = crate::magma_common_defs::MAGMA_STATUS_INVALID_ARGS,
    AccessDenied = crate::magma_common_defs::MAGMA_STATUS_ACCESS_DENIED,
    MemoryError = crate::magma_common_defs::MAGMA_STATUS_MEMORY_ERROR,
    Unimplemented = crate::magma_common_defs::MAGMA_STATUS_UNIMPLEMENTED,
    BadState = crate::magma_common_defs::MAGMA_STATUS_BAD_STATE,
}

impl std::fmt::Display for MagmaStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for MagmaStatus {}

impl TryFrom<i32> for MagmaStatus {
    type Error = i32;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Self::from_repr(value).ok_or(value)
    }
}

impl From<MagmaStatus> for Result<(), zx::Status> {
    fn from(status: MagmaStatus) -> Result<(), zx::Status> {
        match status {
            MagmaStatus::Ok => Ok(()),
            MagmaStatus::InternalError => Err(zx::Status::INTERNAL),
            MagmaStatus::InvalidArgs => Err(zx::Status::INVALID_ARGS),
            MagmaStatus::AccessDenied => Err(zx::Status::ACCESS_DENIED),
            MagmaStatus::MemoryError => Err(zx::Status::NO_MEMORY),
            MagmaStatus::Unimplemented => Err(zx::Status::NOT_SUPPORTED),
            MagmaStatus::BadState => Err(zx::Status::BAD_STATE),
        }
    }
}

pub trait Owner {
    fn driver(&self) -> &dyn traits::Driver;
    fn perf_count_access_token_id(&self) -> u64;
}

pub struct MagmaNotificationHandler {
    pub notification_channel: ServerEnd<fidl_fuchsia_gpu_magma::NotificationMarker>,
    pub message_sender: futures::channel::mpsc::UnboundedSender<ConnectionMessage>,
}

impl traits::NotificationHandler for MagmaNotificationHandler {
    fn notification_channel_send(&self, data: &[u8]) {
        if let Err(e) = self.notification_channel.channel().write(data, &mut []) {
            log::error!("Failed to write to notification channel: {:?}", e);
        }
    }

    fn context_killed(&self) {
        let _ = self.message_sender.unbounded_send(ConnectionMessage::ContextKilled);
    }
}

pub struct MagmaSystemConnection {
    // This is a reference to MagmaSystemDevice.
    owner: Arc<dyn Owner>,
    msd_connection: Box<dyn traits::Connection>,
    context_map: HashMap<ContextId, MagmaSystemContext>,
    buffer_map: HashMap<BufferId, MagmaSystemBuffer>,
    semaphore_map: HashMap<SemaphoreId, MagmaSystemSemaphore>,
    can_access_performance_counters: bool,
    pool_map: HashMap<
        PerformanceId,
        fidl::endpoints::ServerEnd<fidl_fuchsia_gpu_magma::PerformanceCounterEventsMarker>,
    >,
    pub notification_handler: Arc<MagmaNotificationHandler>,
}

impl MagmaSystemConnection {
    pub fn new(
        owner: Arc<dyn Owner>,
        msd_connection: Box<dyn traits::Connection>,
        notification_handler: Arc<MagmaNotificationHandler>,
    ) -> Self {
        MagmaSystemConnection {
            owner,
            msd_connection,
            context_map: HashMap::new(),
            buffer_map: HashMap::new(),
            semaphore_map: HashMap::new(),
            can_access_performance_counters: false,
            pool_map: HashMap::new(),
            notification_handler: notification_handler,
        }
    }

    pub fn lookup_context(&self, id: ContextId) -> Option<&MagmaSystemContext> {
        self.context_map.get(&id)
    }

    pub fn lookup_buffer(&self, id: BufferId) -> Option<&MagmaSystemBuffer> {
        self.buffer_map.get(&id)
    }

    pub fn lookup_semaphore(&self, id: SemaphoreId) -> Option<&MagmaSystemSemaphore> {
        self.semaphore_map.get(&id)
    }

    pub fn import_object(
        &mut self,
        handle: zx::NullableHandle,
        flags: u64,
        object_type: MagmaObjectType,
        client_id: u64,
    ) -> Result<(), MagmaStatus> {
        if client_id == 0 {
            return Err(MagmaStatus::InvalidArgs).dlog_err("Imported zero client_id");
        }

        match object_type {
            MagmaObjectType::Buffer => {
                let vmo: zx::Vmo = handle.into();

                // Verify it's a VMO.
                vmo.get_size().map_err(|_| MagmaStatus::InvalidArgs)?;

                // Duplicate handle for import.
                let duplicate_vmo = vmo
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .map_err(|_| MagmaStatus::InternalError)?;
                let msd_buffer =
                    self.msd_connection.import_buffer(duplicate_vmo, client_id, flags)?;

                let buffer = MagmaSystemBuffer::new(vmo, msd_buffer);

                if self.buffer_map.contains_key(&BufferId(client_id)) {
                    return Err(MagmaStatus::InvalidArgs);
                }
                self.buffer_map.insert(BufferId(client_id), buffer);
                Ok(())
            }
            MagmaObjectType::Semaphore => {
                let global_id = handle
                    .koid()
                    .map(|koid| koid.raw_koid())
                    .map_err(|_| MagmaStatus::InternalError)?;
                let msd_semaphore =
                    self.owner.driver().import_semaphore(handle, client_id, flags)?;
                let semaphore = MagmaSystemSemaphore::new(global_id, msd_semaphore);

                if self.semaphore_map.contains_key(&SemaphoreId(client_id)) {
                    return Err(MagmaStatus::InvalidArgs);
                }
                self.semaphore_map.insert(SemaphoreId(client_id), semaphore);
                Ok(())
            }
        }
    }

    pub fn release_object(
        &mut self,
        object_id: u64,
        object_type: MagmaObjectType,
    ) -> Result<(), MagmaStatus> {
        match object_type {
            MagmaObjectType::Buffer => {
                let buffer =
                    self.buffer_map.remove(&BufferId(object_id)).ok_or(MagmaStatus::InvalidArgs)?;
                self.msd_connection.release_buffer(buffer.msd_buffer(), false);
                Ok(())
            }
            MagmaObjectType::Semaphore => {
                self.semaphore_map
                    .remove(&SemaphoreId(object_id))
                    .ok_or(MagmaStatus::InvalidArgs)?;
                Ok(())
            }
        }
    }

    pub fn create_context(&mut self, context_id: ContextId) -> Result<(), MagmaStatus> {
        self.create_context_2(context_id, fidl_fuchsia_gpu_magma::Priority::Medium.into_primitive())
    }

    pub fn create_context_2(
        &mut self,
        context_id: ContextId,
        priority: u64,
    ) -> Result<(), MagmaStatus> {
        if self.context_map.contains_key(&context_id) {
            return Err(MagmaStatus::InvalidArgs);
        }

        let msd_ctx = self.msd_connection.create_context_2(priority)?;

        let ctx = MagmaSystemContext::new(msd_ctx);

        self.context_map.insert(context_id, ctx);
        Ok(())
    }

    pub fn destroy_context(&mut self, context_id: ContextId) -> Result<(), MagmaStatus> {
        self.context_map.remove(&context_id).ok_or(MagmaStatus::InvalidArgs)?;
        Ok(())
    }

    pub fn execute_command_buffers(
        &mut self,
        context_id: ContextId,
        command_buffers: Vec<MagmaExecCommandBuffer>,
        resources: Vec<MagmaExecResource>,
        wait_semaphores: Vec<u64>,
        signal_semaphores: Vec<u64>,
        _flags: u64,
    ) -> Result<(), MagmaStatus> {
        let context = self.lookup_context(context_id).ok_or(MagmaStatus::InvalidArgs)?;

        let mut resolved_buffers = Vec::with_capacity(resources.len());
        for resource in &resources {
            let id = resource.buffer_id;
            let buffer = self.buffer_map.get(&BufferId(id)).ok_or(MagmaStatus::InvalidArgs)?;

            // Validate the resource.
            let buffer_size = buffer.size()?;
            let Some(end) = resource.offset.checked_add(resource.length) else {
                log::warn!("Resource offset + length overflowed");
                return Err(MagmaStatus::InvalidArgs);
            };
            if end > buffer_size {
                log::warn!(
                    "Resource range [0x{:x}, 0x{:x}) out of bounds (size 0x{:x})",
                    resource.offset,
                    end,
                    buffer_size
                );
                return Err(MagmaStatus::InvalidArgs);
            }

            resolved_buffers.push(buffer);
        }

        // Validate command buffer resources.
        for command_buffer in &command_buffers {
            if command_buffer.resource_index >= resources.len() as u32 {
                log::warn!("Resource OOB: {} {}", command_buffer.resource_index, resources.len());
                return Err(MagmaStatus::InvalidArgs);
            }
            let buffer_size = resolved_buffers[command_buffer.resource_index as usize].size()?;
            if command_buffer.start_offset >= buffer_size {
                log::warn!("Buffer OOB: {} {}", command_buffer.start_offset, buffer_size);
                return Err(MagmaStatus::InvalidArgs);
            }
        }

        let mut resolved_wait_semaphores = Vec::with_capacity(wait_semaphores.len());
        for id in &wait_semaphores {
            let semaphore =
                self.semaphore_map.get(&SemaphoreId(*id)).ok_or(MagmaStatus::InvalidArgs)?;
            resolved_wait_semaphores.push(semaphore);
        }

        let mut resolved_signal_semaphores = Vec::with_capacity(signal_semaphores.len());
        for id in &signal_semaphores {
            let semaphore =
                self.semaphore_map.get(&SemaphoreId(*id)).ok_or(MagmaStatus::InvalidArgs)?;
            resolved_signal_semaphores.push(semaphore);
        }

        context.execute_command_buffers(
            command_buffers,
            resources,
            resolved_buffers,
            resolved_wait_semaphores,
            resolved_signal_semaphores,
        )
    }

    pub fn map_buffer(
        &mut self,
        buffer_id: BufferId,
        gpu_va: u64,
        offset: u64,
        length: u64,
        flags: u64,
    ) -> Result<(), MagmaStatus> {
        let buffer = self.buffer_map.get(&buffer_id).ok_or(MagmaStatus::InvalidArgs)?;

        let buffer_size = buffer.size()?;
        if offset.checked_add(length).is_none() {
            return Err(MagmaStatus::InvalidArgs);
        }
        if length + offset > buffer_size {
            return Err(MagmaStatus::InvalidArgs);
        }
        if flags == 0 {
            return Err(MagmaStatus::InvalidArgs);
        }

        self.msd_connection.map_buffer(buffer.msd_buffer(), gpu_va, offset, length, flags)
    }

    pub fn unmap_buffer(&mut self, buffer_id: BufferId, gpu_va: u64) -> Result<(), MagmaStatus> {
        let buffer = self.buffer_map.get(&buffer_id).ok_or(MagmaStatus::InvalidArgs)?;

        self.msd_connection.unmap_buffer(buffer.msd_buffer(), gpu_va)
    }

    pub fn buffer_range_op(
        &mut self,
        buffer_id: BufferId,
        op: u32,
        start: u64,
        length: u64,
    ) -> Result<(), MagmaStatus> {
        let buffer = self.buffer_map.get(&buffer_id).ok_or(MagmaStatus::InvalidArgs)?;

        let buffer_size = buffer.size()?;
        if start.checked_add(length).is_none() {
            return Err(MagmaStatus::InvalidArgs);
        }
        if start + length > buffer_size {
            return Err(MagmaStatus::InvalidArgs);
        }

        self.msd_connection.buffer_range_op(buffer.msd_buffer(), op, start, length)
    }

    pub fn enable_performance_counter_access(
        &mut self,
        access_token: zx::NullableHandle,
    ) -> Result<(), MagmaStatus> {
        let perf_count_access_token_id = self.owner.perf_count_access_token_id();
        if perf_count_access_token_id == 0 {
            return Err(MagmaStatus::InvalidArgs);
        }
        if access_token.is_invalid() {
            return Err(MagmaStatus::InvalidArgs);
        }

        let koid =
            access_token.koid().map(|k| k.raw_koid()).map_err(|_| MagmaStatus::InternalError)?;

        if koid != perf_count_access_token_id {
            // This is not counted as an error, since it can happen if the client uses the event from the
            // wrong driver.

            return Ok(());
        }

        self.can_access_performance_counters = true;
        Ok(())
    }

    pub fn is_performance_counter_access_allowed(&self) -> bool {
        self.can_access_performance_counters
    }

    pub fn enable_performance_counters(&mut self, counters: Vec<u64>) -> Result<(), MagmaStatus> {
        if !self.is_performance_counter_access_allowed() {
            return Err(MagmaStatus::AccessDenied);
        }

        self.msd_connection.enable_performance_counters(counters)
    }

    pub fn create_performance_counter_buffer_pool(
        &mut self,
        pool_id: PerformanceId,
        server_end: ServerEnd<fidl_fuchsia_gpu_magma::PerformanceCounterEventsMarker>,
    ) -> Result<(), MagmaStatus> {
        if !self.is_performance_counter_access_allowed() {
            return Err(MagmaStatus::AccessDenied);
        }

        if self.pool_map.contains_key(&pool_id) {
            return Err(MagmaStatus::InvalidArgs);
        }
        self.pool_map.insert(pool_id, server_end);

        self.msd_connection.create_performance_counter_buffer_pool(pool_id.0)
    }

    pub fn release_performance_counter_buffer_pool(
        &mut self,
        pool_id: PerformanceId,
    ) -> Result<(), MagmaStatus> {
        if !self.is_performance_counter_access_allowed() {
            return Err(MagmaStatus::AccessDenied);
        }

        self.pool_map.remove(&pool_id).ok_or(MagmaStatus::InvalidArgs)?;

        self.msd_connection.release_performance_counter_buffer_pool(pool_id.0)
    }

    pub fn add_performance_counter_buffer_offset_to_pool(
        &mut self,
        pool_id: PerformanceId,
        buffer_id: BufferId,
        offset: u64,
        size: u64,
    ) -> Result<(), MagmaStatus> {
        if !self.is_performance_counter_access_allowed() {
            return Err(MagmaStatus::AccessDenied);
        }

        if !self.pool_map.contains_key(&pool_id) {
            return Err(MagmaStatus::InvalidArgs);
        }

        self.msd_connection.add_performance_counter_buffer_offset_to_pool(
            pool_id.0,
            buffer_id.0,
            offset,
            size,
        )
    }

    pub fn remove_performance_counter_buffer_from_pool(
        &mut self,
        pool_id: PerformanceId,
        buffer_id: BufferId,
    ) -> Result<(), MagmaStatus> {
        if !self.is_performance_counter_access_allowed() {
            return Err(MagmaStatus::AccessDenied);
        }

        if !self.pool_map.contains_key(&pool_id) {
            return Err(MagmaStatus::InvalidArgs);
        }

        self.msd_connection.remove_performance_counter_buffer_from_pool(pool_id.0, buffer_id.0)
    }

    pub fn dump_performance_counters(
        &mut self,
        pool_id: PerformanceId,
        trigger_id: u64,
    ) -> Result<(), MagmaStatus> {
        if !self.is_performance_counter_access_allowed() {
            return Err(MagmaStatus::AccessDenied);
        }

        if !self.pool_map.contains_key(&pool_id) {
            return Err(MagmaStatus::InvalidArgs);
        }

        self.msd_connection.dump_performance_counters(pool_id.0, trigger_id as u32)
    }

    pub fn clear_performance_counters(&mut self, counters: Vec<u64>) -> Result<(), MagmaStatus> {
        if !self.is_performance_counter_access_allowed() {
            return Err(MagmaStatus::AccessDenied);
        }

        self.msd_connection.clear_performance_counters(counters)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockDevice, MockDriver};

    use std::sync::{Arc, Mutex};

    fn invalid_notification_channel()
    -> fidl::endpoints::ServerEnd<fidl_fuchsia_gpu_magma::NotificationMarker> {
        fidl::endpoints::ServerEnd::new(zx::Channel::invalid())
    }

    fn create_test_connection(
        device: std::sync::Arc<crate::magma_system_device::MagmaSystemDevice>,
    ) -> crate::magma_system_connection::MagmaSystemConnection {
        let (tx, _) = futures::channel::mpsc::unbounded();
        crate::magma_system_connection::MagmaSystemConnection::new(
            Arc::new(device.clone()),
            Box::new(crate::mock::MockConnection),
            Arc::new(MagmaNotificationHandler {
                notification_channel: invalid_notification_channel(),
                message_sender: tx,
            }),
        )
    }

    fn create_test_connection_with_owner(
        owner: Arc<dyn crate::magma_system_connection::Owner>,
    ) -> crate::magma_system_connection::MagmaSystemConnection {
        let (tx, _) = futures::channel::mpsc::unbounded();
        crate::magma_system_connection::MagmaSystemConnection::new(
            owner,
            Box::new(crate::mock::MockConnection),
            Arc::new(MagmaNotificationHandler {
                notification_channel: invalid_notification_channel(),
                message_sender: tx,
            }),
        )
    }

    #[fuchsia::test]
    fn execute_command_buffers_normal() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        assert!(connection.create_context(ContextId(1)).is_ok());

        let vmo = zx::Vmo::create(256).unwrap();
        let id = 1;

        assert!(
            connection
                .import_object(zx::NullableHandle::from(vmo), 0, MagmaObjectType::Buffer, id)
                .is_ok()
        );

        let command_buffers = vec![MagmaExecCommandBuffer { resource_index: 0, start_offset: 0 }];
        let resources = vec![MagmaExecResource { buffer_id: 1, offset: 0, length: 256 }];

        let result = connection.execute_command_buffers(
            ContextId(1),
            command_buffers,
            resources,
            vec![],
            vec![],
            0,
        );
        assert!(result.is_ok());
    }

    #[fuchsia::test]
    fn execute_command_buffers_invalid_resource_index() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        assert!(connection.create_context(ContextId(1)).is_ok());

        let vmo = zx::Vmo::create(256).unwrap();
        let id = 1;

        assert!(
            connection
                .import_object(zx::NullableHandle::from(vmo), 0, MagmaObjectType::Buffer, id)
                .is_ok()
        );

        let command_buffers = vec![MagmaExecCommandBuffer {
            resource_index: 1, // Invalid! Only 1 resource at index 0.
            start_offset: 0,
        }];
        let resources = vec![MagmaExecResource { buffer_id: 1, offset: 0, length: 256 }];

        let result = connection.execute_command_buffers(
            ContextId(1),
            command_buffers,
            resources,
            vec![],
            vec![],
            0,
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), MagmaStatus::InvalidArgs);
    }

    #[fuchsia::test]
    fn execute_command_buffers_invalid_batch_start_offset() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        assert!(connection.create_context(ContextId(1)).is_ok());

        let vmo = zx::Vmo::create(256).unwrap();
        let id = 1;

        assert!(
            connection
                .import_object(zx::NullableHandle::from(vmo), 0, MagmaObjectType::Buffer, id)
                .is_ok()
        );

        let command_buffers = vec![MagmaExecCommandBuffer {
            resource_index: 0,
            start_offset: 4096, // Invalid! Size is 4096 (due to page alignment), so offset 4096 is OOB!
        }];
        let resources = vec![MagmaExecResource { buffer_id: 1, offset: 0, length: 256 }];

        let result = connection.execute_command_buffers(
            ContextId(1),
            command_buffers,
            resources,
            vec![],
            vec![],
            0,
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), MagmaStatus::InvalidArgs);
    }

    #[fuchsia::test]
    fn execute_command_buffers_invalid_exec_resource_handle() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        assert!(connection.create_context(ContextId(1)).is_ok());

        let command_buffers = vec![MagmaExecCommandBuffer { resource_index: 0, start_offset: 0 }];
        let resources = vec![MagmaExecResource {
            buffer_id: 1, // Will fail lookup!
            offset: 0,
            length: 256,
        }];

        let result = connection.execute_command_buffers(
            ContextId(1),
            command_buffers,
            resources,
            vec![],
            vec![],
            0,
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), MagmaStatus::InvalidArgs);
    }

    #[fuchsia::test]
    fn execute_command_buffers_invalid_params() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        assert!(connection.create_context(ContextId(1)).is_ok());

        let vmo = zx::Vmo::create(256).unwrap();
        let id = 1;

        assert!(
            connection
                .import_object(zx::NullableHandle::from(vmo), 0, MagmaObjectType::Buffer, id)
                .is_ok()
        );

        // Overflow the offset and length.
        let result = connection.execute_command_buffers(
            ContextId(1),
            vec![MagmaExecCommandBuffer { resource_index: 0, start_offset: 0 }],
            vec![MagmaExecResource { buffer_id: 1, offset: 1, length: u64::MAX }],
            vec![],
            vec![],
            0,
        );
        assert_eq!(result.unwrap_err(), MagmaStatus::InvalidArgs);

        // Have a length that is out of bounds.
        let result = connection.execute_command_buffers(
            ContextId(1),
            vec![MagmaExecCommandBuffer { resource_index: 0, start_offset: 0 }],
            vec![MagmaExecResource { buffer_id: 1, offset: 0, length: 4097 }],
            vec![],
            vec![],
            0,
        );
        assert_eq!(result.unwrap_err(), MagmaStatus::InvalidArgs);

        // Have an offset that is out of bounds.
        let result = connection.execute_command_buffers(
            ContextId(1),
            vec![MagmaExecCommandBuffer { resource_index: 0, start_offset: 0 }],
            vec![MagmaExecResource { buffer_id: 1, offset: 4097, length: 256 }],
            vec![],
            vec![],
            0,
        );
        assert_eq!(result.unwrap_err(), MagmaStatus::InvalidArgs);
    }

    #[fuchsia::test]
    fn execute_command_buffers_duplicate_exec_resource_handle() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        assert!(connection.create_context(ContextId(1)).is_ok());

        let vmo = zx::Vmo::create(256).unwrap();
        let id = 1;

        assert!(
            connection
                .import_object(zx::NullableHandle::from(vmo), 0, MagmaObjectType::Buffer, id)
                .is_ok()
        );

        let command_buffers = vec![MagmaExecCommandBuffer { resource_index: 0, start_offset: 0 }];
        let resources = vec![
            MagmaExecResource { buffer_id: 1, offset: 0, length: 256 },
            MagmaExecResource {
                buffer_id: 1, // Duplicate!
                offset: 0,
                length: 256,
            },
        ];

        let result = connection.execute_command_buffers(
            ContextId(1),
            command_buffers,
            resources,
            vec![],
            vec![],
            0,
        );
        assert!(result.is_ok()); // Duplicate resource is allowed!
    }

    #[fuchsia::test]
    fn execute_command_buffers_invalid_wait_semaphore() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        assert!(connection.create_context(ContextId(1)).is_ok());

        let result =
            connection.execute_command_buffers(ContextId(1), vec![], vec![], vec![1], vec![], 0); // Fail lookup of 1!
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), MagmaStatus::InvalidArgs);
    }

    #[fuchsia::test]
    fn execute_command_buffers_invalid_signal_semaphore() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        assert!(connection.create_context(ContextId(1)).is_ok());

        let result =
            connection.execute_command_buffers(ContextId(1), vec![], vec![], vec![], vec![1], 0); // Fail lookup of 1!
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), MagmaStatus::InvalidArgs);
    }

    #[fuchsia::test]
    fn context_management() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        assert!(connection.create_context(ContextId(0)).is_ok());
        assert!(connection.create_context(ContextId(1)).is_ok());

        assert!(connection.destroy_context(ContextId(0)).is_ok());
        assert!(connection.destroy_context(ContextId(0)).is_err()); // Double destroy fails!

        assert!(connection.destroy_context(ContextId(1)).is_ok());
        assert!(connection.destroy_context(ContextId(1)).is_err());
    }

    #[fuchsia::test]
    fn buffer_management() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        let vmo = zx::Vmo::create(4096).unwrap();
        let id = 1;

        assert!(
            connection
                .import_object(zx::NullableHandle::from(vmo), 0, MagmaObjectType::Buffer, id)
                .is_ok()
        );

        assert!(connection.lookup_buffer(BufferId(id)).is_some());

        // Double import fails!
        let vmo2 = zx::Vmo::create(4096).unwrap();
        assert!(
            connection
                .import_object(zx::NullableHandle::from(vmo2), 0, MagmaObjectType::Buffer, id)
                .is_err()
        );

        assert!(connection.release_object(id, MagmaObjectType::Buffer).is_ok());

        assert!(connection.lookup_buffer(BufferId(id)).is_none());

        // Double release fails!
        assert!(connection.release_object(id, MagmaObjectType::Buffer).is_err());
    }

    #[fuchsia::test]
    fn semaphores() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        let semaphore = zx::Event::create();
        let id = 1;

        assert!(
            connection
                .import_object(
                    zx::NullableHandle::from(semaphore),
                    0,
                    MagmaObjectType::Semaphore,
                    id
                )
                .is_ok()
        );

        assert!(connection.lookup_semaphore(SemaphoreId(id)).is_some());

        // Double import fails!
        let semaphore2 = zx::Event::create();
        assert!(
            connection
                .import_object(
                    zx::NullableHandle::from(semaphore2),
                    0,
                    MagmaObjectType::Semaphore,
                    id
                )
                .is_err()
        );

        assert!(connection.release_object(id, MagmaObjectType::Semaphore).is_ok());

        assert!(connection.lookup_semaphore(SemaphoreId(id)).is_none());

        // Double release fails!
        assert!(connection.release_object(id, MagmaObjectType::Semaphore).is_err());
    }

    #[fuchsia::test]
    fn bad_semaphore_import() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        let bogus_handle: zx::NullableHandle = unsafe { std::mem::transmute(0u32) };
        assert!(connection.import_object(bogus_handle, 0, MagmaObjectType::Semaphore, 1).is_err());
    }

    #[fuchsia::test]
    fn shutdown() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let dropped = Arc::new(Mutex::new(false));

        struct TestConnection {
            dropped: Arc<Mutex<bool>>,
        }
        impl traits::Connection for TestConnection {
            fn create_context_2(
                &self,
                _priority: u64,
            ) -> Result<Box<dyn traits::Context>, MagmaStatus> {
                Err(MagmaStatus::InternalError)
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
            fn unmap_buffer(
                &self,
                _buffer: &dyn traits::Buffer,
                _hw_va: u64,
            ) -> Result<(), MagmaStatus> {
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
            fn create_performance_counter_buffer_pool(
                &self,
                _pool_id: u64,
            ) -> Result<(), MagmaStatus> {
                Ok(())
            }
            fn release_performance_counter_buffer_pool(
                &self,
                _pool_id: u64,
            ) -> Result<(), MagmaStatus> {
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
                _vmo: zx::Vmo,
                _client_id: u64,
                _flags: u64,
            ) -> Result<Box<dyn traits::Buffer>, MagmaStatus> {
                Err(MagmaStatus::InternalError)
            }
        }

        impl Drop for TestConnection {
            fn drop(&mut self) {
                let mut dropped = self.dropped.lock().unwrap();
                *dropped = true;
            }
        }

        let (tx, _) = futures::channel::mpsc::unbounded();
        let connection = MagmaSystemConnection::new(
            Arc::new(device.clone()),
            Box::new(TestConnection { dropped: dropped.clone() }),
            Arc::new(MagmaNotificationHandler {
                notification_channel: invalid_notification_channel(),
                message_sender: tx,
            }),
        );

        assert!(!*dropped.lock().unwrap());
        drop(connection);
        assert!(*dropped.lock().unwrap());
    }

    #[fuchsia::test]
    fn buffer_sharing() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection_0 = create_test_connection(device.clone());
        let mut connection_1 = create_test_connection(device.clone());

        let vmo = zx::Vmo::create(4096).unwrap();
        let id_0 = 1;
        let id_1 = 2;

        let duplicate_vmo = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();

        assert!(
            connection_0
                .import_object(zx::NullableHandle::from(vmo), 0, MagmaObjectType::Buffer, id_0)
                .is_ok()
        );
        assert!(
            connection_1
                .import_object(
                    zx::NullableHandle::from(duplicate_vmo),
                    0,
                    MagmaObjectType::Buffer,
                    id_1
                )
                .is_ok()
        );

        let buf_0 = connection_0.lookup_buffer(BufferId(id_0)).unwrap();
        let buf_1 = connection_1.lookup_buffer(BufferId(id_1)).unwrap();

        assert_eq!(buf_0.global_id().unwrap(), buf_1.global_id().unwrap());
    }

    #[fuchsia::test]
    fn bad_buffer_import() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        let id = 1;

        // Bogus handle (invalid).
        let bogus_vmo: zx::Vmo = unsafe { std::mem::transmute(0u32) };
        assert!(
            connection
                .import_object(zx::NullableHandle::from(bogus_vmo), 0, MagmaObjectType::Buffer, id)
                .is_err()
        );

        // Import semaphore as buffer!
        let semaphore = zx::Event::create();
        assert!(
            connection
                .import_object(zx::NullableHandle::from(semaphore), 0, MagmaObjectType::Buffer, id)
                .is_err()
        );
    }

    #[fuchsia::test]
    fn map_buffer_gpu() {
        let driver = MockDriver;
        let msd_dev = MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let mut connection = create_test_connection(device.clone());

        let vmo = zx::Vmo::create(4096 * 10).unwrap();
        let id = 1;

        // Bad ID (not imported)
        assert!(connection.map_buffer(BufferId(2), 0, 0, 4096 * 10, 1).is_err());

        let vmo_clone = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        assert!(
            connection
                .import_object(zx::NullableHandle::from(vmo_clone), 0, MagmaObjectType::Buffer, id)
                .is_ok()
        );

        // Bad page offset
        assert!(connection.map_buffer(BufferId(id), 0, 4096 * 10, 4096, 1).is_err());

        // Bad page count
        assert!(connection.map_buffer(BufferId(id), 0, 0, 4096 * 11, 1).is_err());

        // Valid map
        assert!(connection.map_buffer(BufferId(id), 0, 0, 4096 * 10, 1).is_ok());
    }

    struct MockConnectionOwner {
        driver: MockDriver,
        token_id: u64,
    }
    impl crate::magma_system_connection::Owner for MockConnectionOwner {
        fn driver(&self) -> &dyn crate::traits::Driver {
            &self.driver
        }
        fn perf_count_access_token_id(&self) -> u64 {
            self.token_id
        }
    }

    #[fuchsia::test]
    fn performance_counters() {
        let event = zx::Event::create();
        let koid = event.koid().unwrap().raw_koid();
        let owner = MockConnectionOwner { driver: MockDriver, token_id: koid };

        let mut connection = create_test_connection_with_owner(Arc::new(owner));

        // Access denied by default!
        assert_eq!(
            connection.create_performance_counter_buffer_pool(
                PerformanceId(1),
                fidl::endpoints::ServerEnd::new(fidl::Channel::from(zx::NullableHandle::from(
                    zx::Counter::invalid()
                )))
            ),
            Err(MagmaStatus::AccessDenied)
        );

        // Enable access!
        assert!(
            connection.enable_performance_counter_access(zx::NullableHandle::from(event)).is_ok()
        );

        let valid_pool_id = 1;

        assert!(
            connection
                .create_performance_counter_buffer_pool(
                    PerformanceId(valid_pool_id),
                    fidl::endpoints::ServerEnd::new(fidl::Channel::from(zx::NullableHandle::from(
                        zx::Counter::invalid()
                    )))
                )
                .is_ok()
        );

        // Double create fails!
        assert_eq!(
            connection.create_performance_counter_buffer_pool(
                PerformanceId(valid_pool_id),
                fidl::endpoints::ServerEnd::new(fidl::Channel::from(zx::NullableHandle::from(
                    zx::Counter::invalid()
                )))
            ),
            Err(MagmaStatus::InvalidArgs)
        );

        assert!(connection.dump_performance_counters(PerformanceId(valid_pool_id), 1).is_ok());

        let vmo = zx::Vmo::create(4096).unwrap();
        let id = 1;
        assert!(
            connection
                .import_object(zx::NullableHandle::from(vmo), 0, MagmaObjectType::Buffer, id)
                .is_ok()
        );

        assert!(
            connection
                .add_performance_counter_buffer_offset_to_pool(
                    PerformanceId(valid_pool_id),
                    BufferId(id),
                    0,
                    4096
                )
                .is_ok()
        );

        assert!(
            connection
                .remove_performance_counter_buffer_from_pool(
                    PerformanceId(valid_pool_id),
                    BufferId(id)
                )
                .is_ok()
        );

        assert!(
            connection
                .release_performance_counter_buffer_pool(PerformanceId(valid_pool_id))
                .is_ok()
        );
    }
}
