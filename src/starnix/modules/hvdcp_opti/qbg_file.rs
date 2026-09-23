// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::utils::connect_to_device_channel;
use fidl_fuchsia_hardware_qcom_hvdcpopti as fhvdcpopti;
use fidl_fuchsia_power_system as fpower;
use futures::channel::oneshot;
use futures_util::{FutureExt, select};
use starnix_core::mm::MemoryAccessorExt;
use starnix_core::power::{ContainerWakingStream, create_proxy_for_wake_events_counter};
use starnix_core::task::dynamic_thread_spawner::SpawnRequestBuilder;
use starnix_core::task::{CurrentTask, EventHandler, WaitCanceler, WaitQueue, Waiter};
use starnix_core::vfs::{
    FileObject, FileObjectState, FileOps, InputBuffer, NamespaceNode, OutputBuffer, VecInputBuffer,
    fileops_impl_nonseekable, fileops_impl_noop_sync,
};
use starnix_logging::{log_error, log_warn, track_stub};
use starnix_sync::{LockDepMutex, QbgReadQueueLock, QbgShutdownLock};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{errno, error};
use std::collections::VecDeque;
use std::sync::Arc;

pub const QBGIOCXCFG: u32 = 0x80684201;
pub const QBGIOCXEPR: u32 = 0x80304202;
pub const QBGIOCXEPW: u32 = 0xC0304203;
pub const QBGIOCXSTEPCHGCFG: u32 = 0xC0F74204;

pub fn create_qbg_device(
    _current_task: &CurrentTask,
    _id: DeviceId,
    _node: &NamespaceNode,
    _flags: OpenFlags,
) -> Result<Box<dyn FileOps>, Errno> {
    Ok(Box::new(QbgDeviceFile::new()))
}

struct QbgDeviceState {
    waiters: WaitQueue,
    read_queue:
        LockDepMutex<VecDeque<(VecInputBuffer, Option<fpower::LeaseToken>)>, QbgReadQueueLock>,
    shutdown: LockDepMutex<Option<oneshot::Sender<()>>, QbgShutdownLock>,
}

impl QbgDeviceState {
    fn new() -> Self {
        Self {
            waiters: WaitQueue::default(),
            read_queue: Default::default(),
            shutdown: Default::default(),
        }
    }

    fn handle_event(&self, event: fhvdcpopti::DataProviderEvent) {
        let fhvdcpopti::DataProviderEvent::OnFifoData { data, wake_lease } = event;
        self.read_queue.lock().push_back((data.into(), wake_lease));
        self.waiters.notify_fd_events(FdEvents::POLLIN);
    }
}

async fn run_qbg_device_event_loop(
    device_state: Arc<QbgDeviceState>,
    mut event_stream: ContainerWakingStream<fhvdcpopti::DataProviderEventStream>,
    shutdown: oneshot::Receiver<()>,
) {
    let mut shutdown = shutdown.fuse();
    loop {
        select! {
            _ = shutdown => { break; },

            event = event_stream.next() => {
                match event {
                    Some(Ok(event)) => {
                        device_state.handle_event(event);
                    }
                    Some(Err(e)) => {
                        log_error!("qbg: Received error from device event stream: {}", e);
                        break;
                    }
                    None => {
                        log_warn!("qbg: Exhausted device event stream");
                        break;
                    }
                }
            }
        }
    }

    device_state.waiters.notify_fd_events(FdEvents::POLLHUP);
}

fn spawn_qbg_device_tasks(
    device_state: Arc<QbgDeviceState>,
    channel: zx::Channel,
    current_task: &CurrentTask,
) {
    if let Some(shutdown) = device_state.shutdown.lock().take() {
        let _ = shutdown.send(());
    }
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    *device_state.shutdown.lock() = Some(shutdown_tx);
    let closure = async move |current_task: &CurrentTask| {
        let counter_name = "hvdcp_opti";
        let (proxy_channel, counter) =
            create_proxy_for_wake_events_counter(channel, counter_name.to_string());
        let message_counter = current_task
            .kernel()
            .suspend_resume_manager
            .add_message_counter(counter_name, Some(counter));
        run_qbg_device_event_loop(
            device_state,
            ContainerWakingStream::new(
                message_counter,
                fhvdcpopti::DataProviderProxy::new(fidl::AsyncChannel::from_channel(proxy_channel))
                    .take_event_stream(),
            ),
            shutdown_rx,
        )
        .await;
    };
    let req = SpawnRequestBuilder::new()
        .with_debug_name("qbg-device-events")
        .with_async_closure(closure)
        .build();
    current_task.kernel().kthreads.spawner().spawn_from_request(req);
}

struct QbgDeviceFile {
    hvdcpopti: fhvdcpopti::DeviceSynchronousProxy,
    state: Arc<QbgDeviceState>,
}

impl QbgDeviceFile {
    pub fn new() -> Self {
        let state = Arc::new(QbgDeviceState::new());
        let hvdcpopti = fhvdcpopti::DeviceSynchronousProxy::new(
            connect_to_device_channel("device").expect("Could not connect to hvdcpopti service"),
        );
        Self { hvdcpopti, state }
    }
}

impl FileOps for QbgDeviceFile {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn open(&self, _file: &FileObject, current_task: &CurrentTask) -> Result<(), Errno> {
        let data_provider = self
            .hvdcpopti
            .get_data_provider(zx::MonotonicInstant::INFINITE)
            .expect("GetDataProvider failed");
        spawn_qbg_device_tasks(self.state.clone(), data_provider.into(), current_task);
        Ok(())
    }

    fn close(self: Box<Self>, _file: &FileObjectState, _current_task: &CurrentTask) {
        if let Some(shutdown) = self.state.shutdown.lock().take() {
            let _ = shutdown.send(());
        }
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let user_addr = UserAddress::from(arg);

        match request {
            QBGIOCXCFG => {
                let config =
                    self.hvdcpopti.get_config(zx::MonotonicInstant::INFINITE).map_err(|e| {
                        log_error!("GetConfig failed: {:?}", e);
                        errno!(EINVAL)
                    })?;
                current_task.write_object(UserRef::new(user_addr), &config).map_err(|e| {
                    log_error!("GetConfig write_object failed: {:?}", e);
                    e
                })?;
                Ok(SUCCESS)
            }
            QBGIOCXEPR => {
                let params = self
                    .hvdcpopti
                    .get_essential_params(zx::MonotonicInstant::INFINITE)
                    .map_err(|e| {
                        log_error!("Failed to GetEssentialParams: {:?}", e);
                        errno!(EINVAL)
                    })?
                    .map_err(|e| {
                        log_error!("GetEssentialParams failed: {:?}", e);
                        errno!(EINVAL)
                    })?;
                current_task.write_object(UserRef::new(user_addr), &params).map_err(|e| {
                    log_error!("GetEssentialParams write_object failed: {:?}", e);
                    e
                })?;
                Ok(SUCCESS)
            }
            QBGIOCXEPW => {
                let params: [u8; fhvdcpopti::ESSENTIAL_PARAMS_LENGTH as usize] =
                    current_task.read_object(UserRef::new(user_addr)).map_err(|e| {
                        log_error!("SetEssentialParams read_object failed: {:?}", e);
                        e
                    })?;
                self.hvdcpopti
                    .set_essential_params(&params, zx::MonotonicInstant::INFINITE)
                    .map_err(|e| {
                        log_error!("Failed to SetEssentialParams: {:?}", e);
                        errno!(EINVAL)
                    })?
                    .map_err(|e| {
                        log_error!("SetEssentialParams failed: {:?}", e);
                        errno!(EINVAL)
                    })?;
                Ok(SUCCESS)
            }
            QBGIOCXSTEPCHGCFG => {
                let params = self
                    .hvdcpopti
                    .get_step_and_jeita_params(zx::MonotonicInstant::INFINITE)
                    .map_err(|e| {
                        log_error!("GetStepAndJeitaParams failed: {:?}", e);
                        errno!(EINVAL)
                    })?;
                current_task.write_object(UserRef::new(user_addr), &params).map_err(|e| {
                    log_error!("GetStepAndJeitaParams write_object failed: {:?}", e);
                    e
                })?;
                Ok(SUCCESS)
            }
            unknown_ioctl => {
                track_stub!(TODO("https://fxbug.dev/322874368"), "qbg ioctl", unknown_ioctl);
                error!(ENOSYS)
            }
        }?;

        Ok(SUCCESS)
    }

    fn read(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        buffer: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        file.blocking_op(current_task, FdEvents::POLLIN | FdEvents::POLLHUP, None, || {
            let mut queue = self.state.read_queue.lock();
            if queue.is_empty() {
                return error!(EAGAIN);
            }

            // Try and pull as much data from the queue as possible to fill the buffer.
            while buffer.available() > 0 {
                let Some((data, _)) = queue.front_mut() else {
                    break;
                };

                buffer.write_buffer(data)?;
                if data.available() == 0 {
                    queue.pop_front();
                }
            }

            Ok(buffer.bytes_written())
        })
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        buffer: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let data = buffer.read_all()?;
        let data_len = data.len();

        self.hvdcpopti
            .set_processed_fifo_data(&data.try_into().unwrap(), zx::MonotonicInstant::INFINITE)
            .map_err(|e| {
                log_error!("SetProcessedFifoData failed: {:?}", e);
                errno!(EINVAL)
            })?;
        Ok(data_len)
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        Some(self.state.waiters.wait_async_fd_events(waiter, events, handler))
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let mut events = FdEvents::POLLOUT;
        if !self.state.read_queue.lock().is_empty() {
            events |= FdEvents::POLLIN;
        }
        Ok(events)
    }
}
