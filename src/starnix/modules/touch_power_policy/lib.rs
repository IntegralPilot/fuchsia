// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::device::DeviceOps;
use starnix_core::task::dynamic_thread_spawner::SpawnRequestBuilder;
use starnix_core::task::{CurrentTask, Kernel, LockupDetectorReceiver};
use starnix_core::vfs::buffers::{InputBuffer, OutputBuffer};
use starnix_core::vfs::{
    CloseFreeSafe, FileObject, FileOps, NamespaceNode, fileops_impl_nonseekable,
    fileops_impl_noop_sync,
};
use starnix_logging::{log_error, log_info};
use starnix_sync::{LockDepMutex, TouchPowerPolicyEnabledLock};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use zerocopy::IntoBytes;

#[derive(Clone)]
pub struct TouchPowerPolicyDevice {
    touch_power_file: Arc<TouchPowerPolicyFile>,
}

impl TouchPowerPolicyDevice {
    pub fn new(touch_standby_sender: Sender<bool>) -> Self {
        TouchPowerPolicyDevice { touch_power_file: TouchPowerPolicyFile::new(touch_standby_sender) }
    }

    pub fn register(self, kernel: &Kernel) {
        let registry = &kernel.device_registry;
        registry
            .register_dyn_device(
                kernel,
                "touch_standby".into(),
                registry.objects.starnix_class(),
                self,
            )
            .expect("can register touch_standby device");
    }

    pub fn start_relay(
        &self,
        kernel: &Kernel,
        touch_standby_receiver: LockupDetectorReceiver<bool>,
    ) {
        let slf = self.clone();
        let closure = move |_current_task: &CurrentTask| {
            let mut prev_enabled = true;
            while let Ok(touch_enabled) = touch_standby_receiver.recv() {
                if touch_enabled != prev_enabled {
                    slf.notify_standby_state_changed(touch_enabled);
                }
                prev_enabled = touch_enabled;
            }
            log_error!("touch_standby relay was terminated unexpectedly.");
        };
        let req = SpawnRequestBuilder::new()
            .with_debug_name("touch-power-policy-relay")
            .with_sync_closure(closure)
            .build();
        kernel.kthreads.spawner().spawn_from_request(req);
    }

    fn notify_standby_state_changed(&self, touch_enabled: bool) {
        // TODO(b/341142285): notify input pipeline that touch_standby state has changed
        log_info!("touch enabled: {:?}", touch_enabled);
    }
}

impl DeviceOps for TouchPowerPolicyDevice {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _devt: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let touch_policy_file = self.touch_power_file.clone();
        Ok(Box::new(touch_policy_file))
    }
}

pub struct TouchPowerPolicyFile {
    // When false, Input Pipeline suspends processing of all touch events.
    touch_enabled: LockDepMutex<bool, TouchPowerPolicyEnabledLock>,
    // Sender used to send changes to `touch_standby` to the device relay
    touch_standby_sender: Sender<bool>,
}

impl TouchPowerPolicyFile {
    pub fn new(touch_standby_sender: Sender<bool>) -> Arc<Self> {
        Arc::new(TouchPowerPolicyFile { touch_enabled: true.into(), touch_standby_sender })
    }
}

/// `TouchPowerPolicyFile` doesn't implement the `close` method.
impl CloseFreeSafe for TouchPowerPolicyFile {}
impl FileOps for TouchPowerPolicyFile {
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
        let touch_enabled = self.touch_enabled.lock().to_owned();
        data.write_all(touch_enabled.as_bytes())
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let content = data.read_all()?;
        let sys_touch_standby = match &*content {
            b"0" | b"0\n" => false,
            b"1" | b"1\n" => true,
            _ => {
                log_error!("Invalid touch_standby value - must be 0 or 1");
                return error!(EINVAL);
            }
        };
        *self.touch_enabled.lock() = sys_touch_standby;
        if let Err(e) = self.touch_standby_sender.send(sys_touch_standby) {
            log_error!("unable to send recent touch_standby state to device relay: {:?}", e);
        }
        Ok(content.len())
    }
}
