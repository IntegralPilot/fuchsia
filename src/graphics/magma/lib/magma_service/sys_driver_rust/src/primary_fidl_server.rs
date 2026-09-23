// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::magma_common_defs::{MAX_INFLIGHT_BYTES, MAX_INFLIGHT_MESSAGES};
use crate::magma_system_connection::{
    BufferId, ContextId, MagmaObjectType, MagmaStatus, PerformanceId,
};
use crate::magma_system_context::{MagmaExecCommandBuffer, MagmaExecResource};
use anyhow::Context;
use fidl::endpoints::RequestStream;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;

pub struct PrimaryFidlServer {
    pub connection: crate::magma_system_connection::MagmaSystemConnection,
    flow_control_enabled: bool,
    messages_consumed: u64,
    bytes_imported: u64,
}

impl PrimaryFidlServer {
    pub fn new(connection: crate::magma_system_connection::MagmaSystemConnection) -> Self {
        PrimaryFidlServer {
            connection,
            flow_control_enabled: false,
            messages_consumed: 0,
            bytes_imported: 0,
        }
    }

    fn flow_control(
        &mut self,
        size: u64,
        control_handle: &fidl_fuchsia_gpu_magma::PrimaryControlHandle,
    ) {
        if !self.flow_control_enabled {
            return;
        }
        self.messages_consumed += 1;
        self.bytes_imported += size;

        if self.messages_consumed >= MAX_INFLIGHT_MESSAGES / 2 {
            match control_handle.send_on_notify_messages_consumed(self.messages_consumed) {
                Ok(()) => self.messages_consumed = 0,
                Err(e) => log::error!("Failed to send OnNotifyMessagesConsumed: {:?}", e),
            }
        }

        if self.bytes_imported >= MAX_INFLIGHT_BYTES / 2 {
            match control_handle.send_on_notify_memory_imported(self.bytes_imported) {
                Ok(()) => self.bytes_imported = 0,
                Err(e) => log::error!("Failed to send OnNotifyMemoryImported: {:?}", e),
            }
        }
    }

    // Handle messages for the Primary fidl server.
    // At the moment drivers are expected to run this on the synchronized (single threaded) DF dispatcher.
    // Usage with the unsynchronized dispatcher has not been validated.
    pub async fn run(
        &mut self,
        mut stream: fidl_fuchsia_gpu_magma::PrimaryRequestStream,
        mut message_receiver: UnboundedReceiver<crate::magma_system_connection::ConnectionMessage>,
    ) -> anyhow::Result<()> {
        let control_handle = stream.control_handle();

        // Use select so we will exit if the FIDL channel closes.
        loop {
            futures::select! {
                item = stream.next() => {
                    match item {
                        Some(res) => {
                            let request = res.context("Fidl error")?;
                            self.handle_message_request(request, &control_handle).await?;
                        }
                        None => break,
                    }
                },
                    item = message_receiver.next() => {
                    match item {
                        Some(crate::magma_system_connection::ConnectionMessage::ContextKilled) => break,
                        None => break,
                    }
                },
            }
        }

        Ok(())
    }

    pub async fn handle_message_request(
        &mut self,
        request: fidl_fuchsia_gpu_magma::PrimaryRequest,
        control_handle: &fidl_fuchsia_gpu_magma::PrimaryControlHandle,
    ) -> anyhow::Result<()> {
        match request {
            fidl_fuchsia_gpu_magma::PrimaryRequest::ImportObject { payload, control_handle: _ } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::ImportObject");
                let flags = payload.flags.map(|f| f.bits() as u64).unwrap_or(0);
                let object_type = payload.object_type.context("No magma object_type")?;
                let object_type = MagmaObjectType::try_from(object_type)
                    .map_err(|_| anyhow::anyhow!("Bad MagmaObjectType"))?;
                let object = payload.object.context("No magma object in import")?;

                let mut size = 0;
                if let fidl_fuchsia_gpu_magma::Object::Buffer(ref b) = object {
                    size = b.get_size().unwrap_or(0);
                }
                self.flow_control(size, control_handle);

                let nullable_handle = match object {
                    fidl_fuchsia_gpu_magma::Object::Semaphore(s) => zx::NullableHandle::from(s),
                    fidl_fuchsia_gpu_magma::Object::Buffer(b) => zx::NullableHandle::from(b),
                    fidl_fuchsia_gpu_magma::Object::CounterSemaphore(s) => {
                        zx::NullableHandle::from(s)
                    }
                    _ => return Err(anyhow::anyhow!("Bad object type in import")),
                };
                let object_id = payload.object_id.context("No object id in import")?;

                self.connection
                    .import_object(nullable_handle, flags, object_type, object_id)
                    .context("ImportObject failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::ReleaseObject {
                object_id,
                object_type,
                ..
            } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::ReleaseObject");
                self.flow_control(0, control_handle);
                let object_type = MagmaObjectType::try_from(object_type)
                    .map_err(|_| anyhow::anyhow!("Bad MagmaObjectType"))?;

                self.connection
                    .release_object(object_id, object_type)
                    .context("ReleaseObject failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::CreateContext { context_id, .. } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::CreateContext");
                self.flow_control(0, control_handle);

                self.connection
                    .create_context(ContextId(context_id))
                    .context("CreateContext failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::CreateContext2 {
                context_id, priority, ..
            } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::CreateContext2");
                self.flow_control(0, control_handle);
                let priority = priority.into_primitive() as u64;

                self.connection
                    .create_context_2(ContextId(context_id), priority)
                    .context("CreateContext2 failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::DestroyContext { context_id, .. } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::DestroyContext");
                self.flow_control(0, control_handle);
                self.connection
                    .destroy_context(ContextId(context_id))
                    .close_on_error(control_handle)
                    .context("DestroyContext failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::ExecuteCommand {
                context_id,
                resources,
                command_buffers,
                wait_semaphores,
                signal_semaphores,
                flags,
                ..
            } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::ExecuteCommand");
                self.flow_control(0, &control_handle);
                let command_buffers_mapped: Vec<MagmaExecCommandBuffer> = command_buffers
                    .into_iter()
                    .map(|cb| MagmaExecCommandBuffer {
                        resource_index: cb.resource_index,
                        start_offset: cb.start_offset,
                    })
                    .collect();

                let resources_mapped: Vec<MagmaExecResource> = resources
                    .into_iter()
                    .map(|res| MagmaExecResource {
                        buffer_id: res.buffer_id,
                        offset: res.offset,
                        length: res.size,
                    })
                    .collect();

                self.connection
                    .execute_command_buffers(
                        ContextId(context_id),
                        command_buffers_mapped,
                        resources_mapped,
                        wait_semaphores,
                        signal_semaphores,
                        flags.bits() as u64,
                    )
                    .close_on_error(control_handle)
                    .context("ExecuteCommandBuffers failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::ExecuteInlineCommands { .. } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::ExecuteInlineCommand");
                self.flow_control(0, &control_handle);
                let res: Result<(), zx::Status> = MagmaStatus::InvalidArgs.into();
                control_handle.shutdown_with_epitaph(res);
                return Err(anyhow::anyhow!("ExecuteInlineCommands unimplmented"));
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::Flush { responder } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::Flush");
                responder.send().context("Failed to flush")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::MapBuffer { payload, control_handle: _ } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::MapBuffer");
                self.flow_control(0, &control_handle);
                let flags = payload.flags.map(|f| f.bits() as u64).unwrap_or(0);
                let range = payload.range.unwrap();

                self.connection
                    .map_buffer(
                        BufferId(range.buffer_id),
                        payload.hw_va.unwrap(),
                        range.offset,
                        range.size,
                        flags,
                    )
                    .close_on_error(control_handle)
                    .context("MapBuffer failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::UnmapBuffer { payload, control_handle: _ } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::UnmapBuffer");
                self.flow_control(0, &control_handle);
                let result = self
                    .connection
                    .unmap_buffer(BufferId(payload.buffer_id.unwrap()), payload.hw_va.unwrap());
                if let Err(e) = result {
                    return Err(anyhow::anyhow!("UnmapBuffer failed: {}", e));
                }
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::BufferRangeOp2 { op, range, .. } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::BufferRangeOp2");
                self.flow_control(0, &control_handle);
                let op = match op {
                    fidl_fuchsia_gpu_magma::BufferOp::PopulateTables => 1,
                    fidl_fuchsia_gpu_magma::BufferOp::DepopulateTables => 2,
                    _ => return Err(anyhow::anyhow!("Failed to translate buffer op")),
                };

                self.connection
                    .buffer_range_op(BufferId(range.buffer_id), op, range.offset, range.size)
                    .context("BufferRangeOp2 failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::EnablePerformanceCounterAccess {
                access_token,
                ..
            } => {
                fuchsia_trace::duration!(
                    "magma",
                    "PrimaryFidlServer::EnablePerformanceCounterAccess"
                );
                self.flow_control(0, &control_handle);

                self.connection
                    .enable_performance_counter_access(access_token.into())
                    .close_on_error(control_handle)
                    .context("EnablePerformanceCounterAccess failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::CreatePerformanceCounterBufferPool {
                pool_id,
                event_channel,
                control_handle: _,
            } => {
                fuchsia_trace::duration!(
                    "magma",
                    "PrimaryFidlServer::CreatePerformanceCounterBufferPool"
                );
                self.flow_control(0, &control_handle);
                self.connection
                    .create_performance_counter_buffer_pool(PerformanceId(pool_id), event_channel)
                    .context("CreatePerformanceCounterBufferPool failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::ReleasePerformanceCounterBufferPool {
                pool_id,
                control_handle: _,
            } => {
                fuchsia_trace::duration!(
                    "magma",
                    "PrimaryFidlServer::ReleasePerformanceCounterBufferPool"
                );
                self.flow_control(0, &control_handle);
                self.connection
                    .release_performance_counter_buffer_pool(PerformanceId(pool_id))
                    .context("ReleasePerformanceCounterBufferPool failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::AddPerformanceCounterBufferOffsetsToPool {
                pool_id: _pool_id,
                offsets: _offsets,
                control_handle: _,
            } => {
                self.flow_control(0, &control_handle);
                return Err(anyhow::anyhow!("Not implemented"));
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::RemovePerformanceCounterBufferFromPool {
                pool_id,
                buffer_id,
                control_handle: _,
            } => {
                fuchsia_trace::duration!(
                    "magma",
                    "PrimaryFidlServer::RemovePerformanceCounterBufferFromPool"
                );
                self.flow_control(0, &control_handle);
                self.connection
                    .remove_performance_counter_buffer_from_pool(
                        PerformanceId(pool_id),
                        BufferId(buffer_id),
                    )
                    .context("RemovePerformanceCounterBufferFromPool failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::DumpPerformanceCounters {
                pool_id,
                trigger_id,
                control_handle: _,
            } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::DumpPerformanceCounters");
                self.flow_control(0, &control_handle);
                self.connection
                    .dump_performance_counters(PerformanceId(pool_id), trigger_id.into())
                    .context("DumpPerformanceCounters failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::ClearPerformanceCounters {
                counters,
                control_handle: _,
            } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::ClearPerformanceCounters");
                self.flow_control(0, &control_handle);
                self.connection
                    .clear_performance_counters(counters)
                    .context("ClearPerformanceCounters failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::IsPerformanceCounterAccessAllowed {
                responder,
            } => {
                fuchsia_trace::duration!(
                    "magma",
                    "PrimaryFidlServer::IsPerformanceCounterAccessAllowed"
                );
                let allowed = self.connection.is_performance_counter_access_allowed();
                responder
                    .send(allowed)
                    .context("Failed to send PerformanceCounterAccessAllowed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::EnablePerformanceCounters {
                counters, ..
            } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::EnablePerformanceCounters");
                self.flow_control(0, &control_handle);
                self.connection
                    .enable_performance_counters(counters)
                    .close_on_error(control_handle)
                    .context("EnablePerformanceCounters failed")?;
            }
            fidl_fuchsia_gpu_magma::PrimaryRequest::EnableFlowControl { control_handle: _ } => {
                fuchsia_trace::duration!("magma", "PrimaryFidlServer::EnableFlowControl");
                self.flow_control_enabled = true;
            }
        }
        Ok(())
    }
}

trait CloseOnError {
    fn close_on_error(self, control_handle: &fidl_fuchsia_gpu_magma::PrimaryControlHandle) -> Self;
}

impl CloseOnError for Result<(), MagmaStatus> {
    fn close_on_error(self, control_handle: &fidl_fuchsia_gpu_magma::PrimaryControlHandle) -> Self {
        match &self {
            Ok(_) => (),
            Err(e) => {
                let res: Result<(), zx::Status> = (*e).into();
                control_handle.shutdown_with_epitaph(res);
            }
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::magma_system_connection::{MagmaNotificationHandler, SemaphoreId};
    use crate::traits::NotificationHandler;
    use std::sync::Arc;

    fn invalid_notification_channel()
    -> fidl::endpoints::ServerEnd<fidl_fuchsia_gpu_magma::NotificationMarker> {
        fidl::endpoints::ServerEnd::new(zx::Channel::invalid())
    }

    #[fuchsia::test]
    async fn import_buffer() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        let vmo = zx::Vmo::create(4096).unwrap();
        let id = 1;

        let payload = fidl_fuchsia_gpu_magma::PrimaryImportObjectRequest {
            object: Some(fidl_fuchsia_gpu_magma::Object::Buffer(vmo)),
            object_id: Some(id),
            object_type: Some(fidl_fuchsia_gpu_magma::ObjectType::Buffer),

            ..fidl_fuchsia_gpu_magma::PrimaryImportObjectRequest::default()
        };

        assert!(proxy.import_object(payload).is_ok());

        assert!(proxy.flush().await.is_ok());

        assert!(server.lock().await.connection.lookup_buffer(BufferId(id)).is_some());
    }

    #[fuchsia::test]
    async fn release_buffer() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        let vmo = zx::Vmo::create(4096).unwrap();
        let id = 1;

        let payload = fidl_fuchsia_gpu_magma::PrimaryImportObjectRequest {
            object: Some(fidl_fuchsia_gpu_magma::Object::Buffer(vmo)),
            object_id: Some(id),
            object_type: Some(fidl_fuchsia_gpu_magma::ObjectType::Buffer),
            ..fidl_fuchsia_gpu_magma::PrimaryImportObjectRequest::default()
        };

        assert!(proxy.import_object(payload).is_ok());
        assert!(proxy.flush().await.is_ok());
        assert!(server.lock().await.connection.lookup_buffer(BufferId(id)).is_some());

        assert!(proxy.release_object(id, fidl_fuchsia_gpu_magma::ObjectType::Buffer).is_ok());
        assert!(proxy.flush().await.is_ok());
        assert!(server.lock().await.connection.lookup_buffer(BufferId(id)).is_none());
    }

    #[fuchsia::test]
    async fn import_semaphore() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        let semaphore = zx::Event::create();
        let id = 1;

        let payload = fidl_fuchsia_gpu_magma::PrimaryImportObjectRequest {
            object: Some(fidl_fuchsia_gpu_magma::Object::Semaphore(semaphore)),
            object_id: Some(id),
            object_type: Some(fidl_fuchsia_gpu_magma::ObjectType::Semaphore),
            ..fidl_fuchsia_gpu_magma::PrimaryImportObjectRequest::default()
        };

        assert!(proxy.import_object(payload).is_ok());
        assert!(proxy.flush().await.is_ok());
        assert!(server.lock().await.connection.lookup_semaphore(SemaphoreId(id)).is_some());
    }

    #[fuchsia::test]
    async fn release_semaphore() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        let semaphore = zx::Event::create();
        let id = 1;

        let payload = fidl_fuchsia_gpu_magma::PrimaryImportObjectRequest {
            object: Some(fidl_fuchsia_gpu_magma::Object::Semaphore(semaphore)),
            object_id: Some(id),
            object_type: Some(fidl_fuchsia_gpu_magma::ObjectType::Semaphore),
            ..fidl_fuchsia_gpu_magma::PrimaryImportObjectRequest::default()
        };

        assert!(proxy.import_object(payload).is_ok());
        assert!(proxy.flush().await.is_ok());
        assert!(server.lock().await.connection.lookup_semaphore(SemaphoreId(id)).is_some());

        assert!(proxy.release_object(id, fidl_fuchsia_gpu_magma::ObjectType::Semaphore).is_ok());
        assert!(proxy.flush().await.is_ok());
        assert!(server.lock().await.connection.lookup_semaphore(SemaphoreId(id)).is_none());
    }

    #[fuchsia::test]
    async fn create_context() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        let context_id = 1;
        assert!(proxy.create_context(context_id).is_ok());
        assert!(proxy.flush().await.is_ok());
    }

    #[fuchsia::test]
    async fn destroy_context() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        let context_id = 1;
        assert!(proxy.create_context(context_id).is_ok());
        assert!(proxy.flush().await.is_ok());

        assert!(proxy.destroy_context(context_id).is_ok());
        assert!(proxy.flush().await.is_ok());
    }

    #[fuchsia::test]
    async fn create_context_2() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        let context_id = 1;
        assert!(
            proxy.create_context2(context_id, fidl_fuchsia_gpu_magma::Priority::Medium).is_ok()
        );
        assert!(proxy.flush().await.is_ok());
    }

    #[fuchsia::test]
    async fn map_unmap_buffer() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        let vmo = zx::Vmo::create(4096 * 3).unwrap();
        let id = 1;

        let payload = fidl_fuchsia_gpu_magma::PrimaryImportObjectRequest {
            object: Some(fidl_fuchsia_gpu_magma::Object::Buffer(vmo)),
            object_id: Some(id),
            object_type: Some(fidl_fuchsia_gpu_magma::ObjectType::Buffer),
            ..fidl_fuchsia_gpu_magma::PrimaryImportObjectRequest::default()
        };

        assert!(proxy.import_object(payload).is_ok());
        assert!(proxy.flush().await.is_ok());

        let map_payload = fidl_fuchsia_gpu_magma::PrimaryMapBufferRequest {
            hw_va: Some(4096 * 1000),
            range: Some(fidl_fuchsia_gpu_magma::BufferRange {
                buffer_id: id,
                offset: 4096,
                size: 4096 * 2,
            }),
            flags: Some(fidl_fuchsia_gpu_magma::MapFlags::from_bits_truncate(1)),
            ..fidl_fuchsia_gpu_magma::PrimaryMapBufferRequest::default()
        };
        assert!(proxy.map_buffer(&map_payload).is_ok());
        assert!(proxy.flush().await.is_ok());

        let unmap_payload = fidl_fuchsia_gpu_magma::PrimaryUnmapBufferRequest {
            buffer_id: Some(id),
            hw_va: Some(4096 * 1000),
            ..fidl_fuchsia_gpu_magma::PrimaryUnmapBufferRequest::default()
        };
        assert!(proxy.unmap_buffer(&unmap_payload).is_ok());
        assert!(proxy.flush().await.is_ok());

        let range = fidl_fuchsia_gpu_magma::BufferRange { buffer_id: id, offset: 1000, size: 2000 };
        assert!(
            proxy
                .buffer_range_op2(fidl_fuchsia_gpu_magma::BufferOp::PopulateTables, &range)
                .is_ok()
        );
        assert!(proxy.flush().await.is_ok());
    }

    #[fuchsia::test]
    async fn flow_control_events() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        assert!(proxy.enable_flow_control().is_ok());

        for i in 0..500 {
            assert!(proxy.create_context(i).is_ok());
        }

        let mut event_stream = proxy.take_event_stream();
        let event = event_stream.next().await.unwrap().unwrap();

        match event {
            fidl_fuchsia_gpu_magma::PrimaryEvent::OnNotifyMessagesConsumed { count } => {
                assert_eq!(count, 500);
            }
            _ => panic!("Unexpected event"),
        }
    }

    #[fuchsia::test]
    async fn test_flush() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        for i in 0..10 {
            assert!(proxy.create_context(i).is_ok());
        }

        assert!(proxy.flush().await.is_ok());
    }

    #[fuchsia::test]
    async fn notification_channel() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let (client_channel, server_channel) =
            fidl::endpoints::create_endpoints::<fidl_fuchsia_gpu_magma::NotificationMarker>();

        let (tx, _) = futures::channel::mpsc::unbounded();
        let connection = crate::magma_system_connection::MagmaSystemConnection::new(
            Arc::new(device.clone()),
            Box::new(crate::mock::MockConnection),
            Arc::new(MagmaNotificationHandler {
                notification_channel: server_channel,
                message_sender: tx,
            }),
        );

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));

        let data = 0x12345678u32;
        server
            .lock()
            .await
            .connection
            .notification_handler
            .notification_channel_send(&data.to_ne_bytes());

        let mut message_buf = zx::MessageBuf::new();
        let client_channel = client_channel.into_channel();
        client_channel.read(&mut message_buf).unwrap();
        let bytes = message_buf.bytes();

        assert_eq!(bytes.len(), 4);
        assert_eq!(u32::from_ne_bytes(bytes.try_into().unwrap()), data);
    }

    #[fuchsia::test]
    async fn multiple_flush() {
        let driver = crate::mock::MockDriver;
        let msd_dev = crate::mock::MockDevice;
        let device = Arc::new(crate::magma_system_device::MagmaSystemDevice::new(
            Box::new(driver),
            Box::new(msd_dev),
            0,
        ));

        let connection = create_test_connection(Arc::new(device.clone()));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        let mut futures = vec![];
        for _ in 0..100 {
            futures.push(proxy.flush());
        }
        let results = futures::future::join_all(futures).await;
        for r in results {
            assert!(r.is_ok());
        }
    }

    struct MockConnectionOwner {
        driver: crate::mock::MockDriver,
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

    fn create_test_connection(
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
    async fn enable_performance_counters() {
        let event = zx::Event::create();
        let koid = event.koid().unwrap().raw_koid();

        let owner = MockConnectionOwner { driver: crate::mock::MockDriver, token_id: koid };

        let connection = create_test_connection(Arc::new(owner));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        assert!(proxy.enable_performance_counter_access(event).is_ok());
        assert!(proxy.flush().await.is_ok());

        assert!(server.lock().await.connection.is_performance_counter_access_allowed());
    }

    #[fuchsia::test]
    async fn test_performance_counters() {
        let event = zx::Event::create();
        let koid = event.koid().unwrap().raw_koid();

        let owner = MockConnectionOwner { driver: crate::mock::MockDriver, token_id: koid };

        let connection = create_test_connection(Arc::new(owner));

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        fuchsia_async::Task::local(async move {
            let mut stream = stream;
            let control_handle = stream.control_handle();
            while let Some(res) = stream.next().await {
                let request = res.unwrap();
                let mut server_locked = server_clone.lock().await;
                server_locked.handle_message_request(request, &control_handle).await.unwrap();
            }
        })
        .detach();

        assert!(proxy.enable_performance_counter_access(event).is_ok());
        assert!(proxy.flush().await.is_ok());

        let counters = vec![2];
        assert!(proxy.enable_performance_counters(&counters).is_ok());
        assert!(proxy.flush().await.is_ok());

        let pool_id = 1;
        let pool_endpoints = fidl::endpoints::create_endpoints::<
            fidl_fuchsia_gpu_magma::PerformanceCounterEventsMarker,
        >();
        assert!(proxy.create_performance_counter_buffer_pool(pool_id, pool_endpoints.1).is_ok());
        assert!(proxy.flush().await.is_ok());

        assert!(proxy.dump_performance_counters(pool_id, 2).is_ok());
        assert!(proxy.flush().await.is_ok());

        assert!(proxy.release_performance_counter_buffer_pool(pool_id).is_ok());
        assert!(proxy.flush().await.is_ok());
    }

    #[fuchsia::test]
    async fn context_killed_closes_connection() {
        let owner = MockConnectionOwner { driver: crate::mock::MockDriver, token_id: 0 };

        let (tx, rx) = futures::channel::mpsc::unbounded();
        let connection = crate::magma_system_connection::MagmaSystemConnection::new(
            Arc::new(owner),
            Box::new(crate::mock::MockConnection),
            Arc::new(MagmaNotificationHandler {
                notification_channel: invalid_notification_channel(),
                message_sender: tx,
            }),
        );

        let (_proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        let (result_tx, result_rx) = futures::channel::oneshot::channel();
        fuchsia_async::Task::local(async move {
            let mut server_locked = server_clone.lock().await;
            let result = server_locked.run(stream, rx).await;
            let _ = result_tx.send(result);
        })
        .detach();

        // Trigger context killed!
        server.lock().await.connection.notification_handler.context_killed();

        // Wait for result!
        let result = result_rx.await.unwrap();
        assert!(result.is_ok());
    }

    #[fuchsia::test]
    async fn fidl_channel_close_closes_connection() {
        let owner = MockConnectionOwner { driver: crate::mock::MockDriver, token_id: 0 };

        let (tx, rx) = futures::channel::mpsc::unbounded();
        let connection = crate::magma_system_connection::MagmaSystemConnection::new(
            Arc::new(owner),
            Box::new(crate::mock::MockConnection),
            Arc::new(MagmaNotificationHandler {
                notification_channel: invalid_notification_channel(),
                message_sender: tx,
            }),
        );

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_gpu_magma::PrimaryMarker>();

        let server =
            std::sync::Arc::new(futures::lock::Mutex::new(PrimaryFidlServer::new(connection)));
        let server_clone = server.clone();
        let (result_tx, result_rx) = futures::channel::oneshot::channel();
        fuchsia_async::Task::local(async move {
            let mut server_locked = server_clone.lock().await;
            let result = server_locked.run(stream, rx).await;
            let _ = result_tx.send(result);
        })
        .detach();

        // Drop our FIDL channel and wait for it to close.
        std::mem::drop(proxy);
        let result = result_rx.await.unwrap();
        assert!(result.is_ok());
    }
}
