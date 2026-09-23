// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

use anyhow::Context;
use fidl_fuchsia_power_cpu_manager::BoostMarker;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_inspect::Property;
use starnix_core::device::DeviceOps;
use starnix_core::task::dynamic_thread_spawner::SpawnRequestBuilder;
use starnix_core::task::{CurrentTask, Kernel, LockupDetectorReceiver, ThreadLockupDetector};
use starnix_core::vfs::buffers::{InputBuffer, OutputBuffer};
use starnix_core::vfs::{
    CloseFreeSafe, FileObject, FileOps, NamespaceNode, fileops_impl_nonseekable,
    fileops_impl_noop_sync,
};
use starnix_logging::{log_error, log_info, log_warn};
use starnix_sync::{BootedLock, LockDepMutex};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use zerocopy::IntoBytes;

/// Initializes the boot notifier device.
pub fn booted_device_init(kernel: &Kernel, cpu_boost_duration: Option<zx::MonotonicDuration>) {
    let (booted_sender, booted_receiver) = ThreadLockupDetector::tracked_channel::<bool>();
    let node = fuchsia_inspect::component::inspector().root().create_child("boot");
    let device = BootedDevice::new(kernel, booted_sender, node, cpu_boost_duration)
        .expect("must be able to initialize booted device");
    device.clone().register(kernel);
    device.start_relay(kernel, booted_receiver);
}

#[derive(Clone)]
struct BootedDevice {
    inner: Arc<Inner>,
}

const INSPECT_KEY: &str = "boot_timestamp";

impl BootedDevice {
    pub fn new(
        kernel: &Kernel,
        booted_sender: Sender<bool>,
        inspect_node: fuchsia_inspect::Node,
        cpu_boost_duration: Option<zx::MonotonicDuration>,
    ) -> Result<Self, anyhow::Error> {
        let boot_timestamp = inspect_node.create_uint(INSPECT_KEY, 0);

        if let Some(duration) = cpu_boost_duration {
            match kernel.connect_to_protocol_at_container_svc::<BoostMarker>() {
                Ok(client_end) => {
                    log_info!("Enabling boot-time CPU boost");
                    let booster = client_end.into_proxy();
                    kernel.kthreads.spawn_future(
                        move || async move {
                            let token = match booster.boost().await {
                                Ok(Ok(token)) => token,
                                e => {
                                    log_warn!(e:?; "Failed to enable boot-time CPU boost");
                                    return;
                                }
                            };
                            fuchsia_async::Timer::new(zx::MonotonicInstant::after(duration)).await;
                            log_info!("Disabling boot-time CPU boost");
                            drop(token);
                        },
                        "boot_cpu_boost",
                    );
                }
                Err(e) => {
                    log_warn!(e:?; "Failed to connect to cpu boost protocol");
                }
            }
        }

        Ok(Self {
            inner: Arc::new(Inner {
                file: File::new(booted_sender),
                _inspect_node: inspect_node,
                boot_timestamp,
            }),
        })
    }

    pub fn register(self, kernel: &Kernel) {
        let registry = &kernel.device_registry;
        registry
            .register_dyn_device(kernel, "booted".into(), registry.objects.starnix_class(), self)
            .expect("can register booted device");
    }

    pub fn start_relay(&self, kernel: &Kernel, booted_receiver: LockupDetectorReceiver<bool>) {
        let this = self.inner.clone();
        let closure = move |_current_task: &CurrentTask| {
            let mut prev_booted = false;
            while let Ok(booted) = booted_receiver.recv() {
                if booted && !prev_booted {
                    match this.notify_boot_completed() {
                        Ok(()) => log_info!("Notified system boot completed"),
                        Err(e) => log_error!(e:?; "Failed to notify system boot completed"),
                    }
                }
                prev_booted = booted;
            }
            log_error!("booted relay was terminated unexpectedly.");
        };
        let req = SpawnRequestBuilder::new()
            .with_debug_name("boot-notifier-relay")
            .with_sync_closure(closure)
            .build();

        kernel.kthreads.spawner().spawn_from_request(req);
    }
}

struct Inner {
    file: Arc<File>,
    _inspect_node: fuchsia_inspect::Node,
    boot_timestamp: fuchsia_inspect::UintProperty,
}

impl Inner {
    fn notify_boot_completed(&self) -> Result<(), anyhow::Error> {
        log_info!("Boot has been marked completed");
        let client =
            fuchsia_component::client::connect_to_protocol_sync::<fsys::BootControllerMarker>()
                .context("connecting to BootController")?;
        client.notify(zx::MonotonicInstant::INFINITE).context("calling BootController/Notify")?;
        let ts = zx::BootInstant::get().into_nanos() as u64;
        let _ = self.boot_timestamp.set(ts);

        Ok(())
    }
}

struct File {
    booted: LockDepMutex<bool, BootedLock>,
    sender: Sender<bool>,
}

impl File {
    fn new(sender: Sender<bool>) -> Arc<Self> {
        Arc::new(Self { booted: false.into(), sender })
    }
}

/// `TouchPowerPolicyFile` doesn't implement the `close` method.
impl CloseFreeSafe for File {}
impl FileOps for File {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);
        let booted = self.booted.lock().to_owned();
        data.write_all(booted.as_bytes())
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let content = data.read_all()?;
        let booted = match &*content {
            b"0" | b"0\n" => false,
            b"1" | b"1\n" => true,
            _ => {
                log_error!("Invalid booted value - must be 0 or 1");
                return error!(EINVAL);
            }
        };
        *self.booted.lock() = booted;
        if let Err(e) = self.sender.send(booted) {
            log_error!("unable to send recent booted state to device relay: {:?}", e);
        }
        Ok(content.len())
    }
}

impl DeviceOps for BootedDevice {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _devt: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let file = self.inner.file.clone();
        Ok(Box::new(file))
    }
}
