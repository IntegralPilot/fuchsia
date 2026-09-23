// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::kobject::DeviceMetadata;
use crate::device::terminal::{Terminal, TtyState};
use crate::device::{DeviceMode, DeviceOps};
use crate::fs::devpts::{TtyFile, new_pts_fs_with_state};
use crate::task::dynamic_thread_spawner::SpawnRequestBuilder;
use crate::task::{CurrentTask, EventHandler, Kernel, ThreadLockupDetector, Waiter};
use crate::vfs::{FileOps, FsString, NamespaceNode, VecInputBuffer, VecOutputBuffer};
use anyhow::Error;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_hardware_serial as fserial;
use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::{DeviceId, TTY_MAJOR};
use starnix_uapi::errors::Errno;
use starnix_uapi::from_status_like_fdio;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::FdEvents;
use std::sync::Arc;

struct ForwardTask {
    terminal: Arc<Terminal>,
    serial_proxy: Arc<fserial::DeviceSynchronousProxy>,
}

impl ForwardTask {
    fn new(terminal: Arc<Terminal>, serial_proxy: Arc<fserial::DeviceSynchronousProxy>) -> Self {
        terminal.main_open();
        Self { terminal, serial_proxy }
    }

    fn spawn_reader(&self, kernel: &Kernel) {
        let terminal = self.terminal.clone();
        let serial_proxy = self.serial_proxy.clone();
        let closure = move |current_task: &CurrentTask| {
            let _result = move || -> Result<(), Error> {
                let waiter = Waiter::new();
                loop {
                    // Register edge triggered waiter for POLLOUT on terminal main side
                    // This is waiting for the terminal to flag it can receive data
                    terminal.main_wait_async(&waiter, FdEvents::POLLOUT, EventHandler::None);

                    // Only await the event if it is not already asserted
                    if !terminal.main_query_events().contains(FdEvents::POLLOUT) {
                        waiter.wait(current_task)?;
                    }

                    let data = {
                        let _waiting_guard = ThreadLockupDetector::pause_tracking();
                        serial_proxy
                            .read(zx::MonotonicInstant::INFINITE)?
                            .map_err(|e: i32| from_status_like_fdio!(zx::Status::err_from_raw(e)))?
                    };
                    terminal.main_write(&mut VecInputBuffer::from(data))?;
                }
            }();
        };
        let req = SpawnRequestBuilder::new()
            .with_debug_name("serial-reader")
            .with_sync_closure(closure)
            .build();
        kernel.kthreads.spawner().spawn_from_request(req);
    }

    fn spawn_writer(&self, kernel: &Kernel) {
        let terminal = self.terminal.clone();
        let serial_proxy = self.serial_proxy.clone();
        let closure = move |current_task: &CurrentTask| {
            let _result = move || -> Result<(), Error> {
                let waiter = Waiter::new();
                loop {
                    // Register edge triggered waiter for POLLIN on terminal main side
                    // This is waiting for the terminal to flag it has data to send
                    terminal.main_wait_async(&waiter, FdEvents::POLLIN, EventHandler::None);

                    // Only await the event if it is not already asserted
                    if !terminal.main_query_events().contains(FdEvents::POLLIN) {
                        waiter.wait(current_task)?;
                    }

                    let size = terminal.read().get_available_read_size(true);
                    let mut buffer = VecOutputBuffer::new(size);
                    terminal.main_read(&mut buffer)?;
                    serial_proxy
                        .write(buffer.data(), zx::MonotonicInstant::INFINITE)?
                        .map_err(|e: i32| from_status_like_fdio!(zx::Status::err_from_raw(e)))?;
                }
            }();
        };
        let req = SpawnRequestBuilder::new()
            .with_debug_name("serial-writer")
            .with_sync_closure(closure)
            .build();
        kernel.kthreads.spawner().spawn_from_request(req);
    }
}

impl Drop for ForwardTask {
    fn drop(&mut self) {
        self.terminal.main_close();
        // TODO: How do we terminate the spawned threads?
    }
}

pub struct SerialDevice {
    terminal: Arc<Terminal>,
    _forward_task: ForwardTask,
}

impl SerialDevice {
    /// Create a serial device attached to the given fuchsia.hardware.serial endpoint.
    ///
    /// To register the device, call `register_serial_device`.
    pub fn new(
        kernel: &Kernel,
        serial_device: ClientEnd<fserial::DeviceMarker>,
        creds: FsCred,
    ) -> Result<Arc<Self>, Errno> {
        let state = Arc::new(TtyState::default());
        let fs = new_pts_fs_with_state(kernel, Default::default(), state.clone())?;
        let terminal = state.get_next_terminal(fs.root().clone(), creds)?;

        let serial_proxy = Arc::new(serial_device.into_sync_proxy());
        let forward_task = ForwardTask::new(terminal.clone(), serial_proxy);
        forward_task.spawn_reader(kernel);
        forward_task.spawn_writer(kernel);

        Ok(Arc::new(Self { terminal, _forward_task: forward_task }))
    }
}

impl DeviceOps for Arc<SerialDevice> {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _id: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(TtyFile::new(self.terminal.clone())))
    }
}

/// Register the given serial device.
///
/// The `index` should be the numerical value associated with the device. For example, if you want
/// to register /dev/ttyS<n>, then `index` should be `n`.
pub fn register_serial_device(
    kernel: &Kernel,
    index: u32,
    serial_device: Arc<SerialDevice>,
) -> Result<(), Errno> {
    // See https://www.kernel.org/doc/Documentation/admin-guide/devices.txt
    //  64 = /dev/ttyS0    First UART serial port
    const SERIAL_MINOR_BASE: u32 = 64;

    let name = FsString::from(format!("ttyS{}", index));

    let registry = &kernel.device_registry;
    registry.register_device(
        kernel,
        name.as_ref(),
        DeviceMetadata::new(
            name.clone(),
            DeviceId::new(TTY_MAJOR, SERIAL_MINOR_BASE + index),
            DeviceMode::Char,
        ),
        registry.objects.tty_class(),
        serial_device,
    )?;
    Ok(())
}
