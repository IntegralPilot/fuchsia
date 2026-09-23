// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

use fidl_fuchsia_hardware_usb_policy::DeviceState;
use starnix_core::task::dynamic_thread_spawner::SpawnRequestBuilder;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_logging::log_error;

use starnix_uapi::errors::Errno;

use fidl_fuchsia_usb_policy::PolicyProviderMarker;
use fuchsia_component::client::connect_to_protocol;
use starnix_core::device::kobject::{Device, UEventAction};
use starnix_core::device::mem::DevNull;
use starnix_core::device::simple_device_ops;
use starnix_core::fs::sysfs::build_device_directory;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};
use starnix_core::vfs::{FsStr, FsString};
use starnix_uapi::file_mode::mode;
use std::borrow::Cow;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UsbGadgetState {
    Disconnected = 0,
    Configured = 1,
    Connected = 2,
    Unknown = 3,
}

impl From<u8> for UsbGadgetState {
    fn from(val: u8) -> Self {
        match val {
            0 => UsbGadgetState::Disconnected,
            1 => UsbGadgetState::Configured,
            2 => UsbGadgetState::Connected,
            _ => UsbGadgetState::Unknown,
        }
    }
}

impl UsbGadgetState {
    pub fn to_fs_str(&self) -> &'static FsStr {
        match self {
            UsbGadgetState::Disconnected => b"DISCONNECTED".into(),
            UsbGadgetState::Configured => b"CONFIGURED".into(),
            UsbGadgetState::Connected => b"CONNECTED".into(),
            UsbGadgetState::Unknown => b"UNKNOWN".into(),
        }
    }

    pub fn map_from_device_state(state: DeviceState, previous: Option<Self>) -> Option<Self> {
        match state {
            DeviceState::NotAttached => Some(UsbGadgetState::Disconnected),
            DeviceState::Attached => Some(UsbGadgetState::Connected),
            DeviceState::Powered => Some(UsbGadgetState::Connected),
            DeviceState::Default => Some(UsbGadgetState::Connected),
            DeviceState::Address => Some(UsbGadgetState::Connected),
            DeviceState::Configured => Some(UsbGadgetState::Configured),
            DeviceState::Suspended => previous, // No change when suspended
            _ => {
                log_error!("Unexpected DeviceState: {:?}", state);
                previous
            }
        }
    }
}

#[derive(Clone)]
struct UsbStateSysfsFile {
    state: Arc<AtomicU8>,
}

impl BytesFileOps for UsbStateSysfsFile {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let state_val = self.state.load(Ordering::Relaxed);
        let state = UsbGadgetState::from(state_val);
        let mut content = state.to_fs_str().to_vec();
        content.push(b'\n'); // standard sysfs newline
        Ok(Cow::Owned(content))
    }
}

pub fn usb_device_init(kernel: &Arc<Kernel>) -> Result<Device, Errno> {
    let registry = &kernel.device_registry;

    let android_usb_class =
        registry.objects.get_or_create_class("android_usb".into(), registry.objects.virtual_bus());

    let shared_state = Arc::new(AtomicU8::new(UsbGadgetState::Disconnected as u8));
    let state_clone = shared_state.clone();

    let device = registry.register_dyn_device_with_dir(
        kernel,
        "android0".into(),
        android_usb_class,
        |device, dir| {
            build_device_directory(device, dir);
            dir.entry(
                "state",
                BytesFile::new_node(UsbStateSysfsFile { state: state_clone }),
                mode!(IFREG, 0o444),
            );
        },
        simple_device_ops::<DevNull>,
    )?;

    let kernel_clone = Arc::clone(kernel);
    let device_clone = device.clone();
    kernel.kthreads.spawn_future(
        move || async move {
            monitor_usb_device_state(kernel_clone, device_clone, shared_state).await;
        },
        "usb_device_state_monitor",
    );

    Ok(device)
}

// Prepare a request to broadcast a USB state change and dispatch it on a kernel thread.
fn dispatch_usb_state_change(
    kernel: &Arc<Kernel>,
    device: &Device,
    usb_state: FsString,
) -> impl Future<Output = Result<(), Errno>> {
    let spawner = kernel.kthreads.spawner();
    let device_clone = device.clone();
    let closure = move |current_task: &CurrentTask| {
        if let Some(metadata) = &device_clone.metadata {
            metadata.properties.insert("USB_STATE".into(), usb_state);
        }

        current_task.kernel.device_registry.dispatch_uevent(UEventAction::Change, device_clone);
    };
    let (result, request) =
        SpawnRequestBuilder::new().with_sync_closure(closure).build_with_async_result();
    spawner.spawn_from_request(request);
    result
}

/// Monitor the USB state via FIDL messages from the PolicyProvider and dispatch uevents when the
/// state changes.
async fn monitor_usb_device_state(
    kernel: Arc<Kernel>,
    device: Device,
    shared_state: Arc<AtomicU8>,
) {
    let provider = connect_to_protocol::<PolicyProviderMarker>()
        .expect("USB Failed to connect to PolicyProvider");
    let mut previous_mapped_state: Option<UsbGadgetState> = None;
    loop {
        match provider.watch_device_state().await {
            Ok(Ok(update)) => {
                let state = update.state.unwrap_or_else(DeviceState::unknown);

                // Map the incoming device state onto the new UsbGadgetState.
                let mapped = UsbGadgetState::map_from_device_state(state, previous_mapped_state);

                if previous_mapped_state != mapped {
                    if let Some(val) = mapped {
                        previous_mapped_state = Some(val);
                        shared_state.store(val as u8, Ordering::Relaxed);
                        if let Err(e) =
                            dispatch_usb_state_change(&kernel, &device, val.to_fs_str().to_owned())
                                .await
                        {
                            log_error!("Failed to dispatch USB state change: {:?}", e);
                        }
                    }
                }
            }
            Ok(Err(err)) => {
                log_error!("USB PolicyProvider returned error: {:?}", err);
                break;
            }
            Err(e) => {
                log_error!("USB PolicyProvider watch failed: {:?}", e);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starnix_core::testing::spawn_kernel_and_run;
    use starnix_rcu::RcuReadScope;

    #[::fuchsia::test]
    async fn test_usb_gadget_state_map_from_device_state() {
        assert_eq!(
            UsbGadgetState::map_from_device_state(DeviceState::NotAttached, None),
            Some(UsbGadgetState::Disconnected)
        );
        assert_eq!(
            UsbGadgetState::map_from_device_state(DeviceState::Configured, None),
            Some(UsbGadgetState::Configured)
        );
        assert_eq!(
            UsbGadgetState::map_from_device_state(DeviceState::Attached, None),
            Some(UsbGadgetState::Connected)
        );
        assert_eq!(
            UsbGadgetState::map_from_device_state(DeviceState::Powered, None),
            Some(UsbGadgetState::Connected)
        );
        assert_eq!(
            UsbGadgetState::map_from_device_state(DeviceState::Default, None),
            Some(UsbGadgetState::Connected)
        );
        assert_eq!(
            UsbGadgetState::map_from_device_state(DeviceState::Address, None),
            Some(UsbGadgetState::Connected)
        );
        assert_eq!(
            UsbGadgetState::map_from_device_state(
                DeviceState::Suspended,
                Some(UsbGadgetState::Configured)
            ),
            Some(UsbGadgetState::Configured)
        );
        assert_eq!(UsbGadgetState::map_from_device_state(DeviceState::Suspended, None), None);
        assert_eq!(UsbGadgetState::map_from_device_state(DeviceState::unknown(), None), None);
        assert_eq!(
            UsbGadgetState::map_from_device_state(
                DeviceState::unknown(),
                Some(UsbGadgetState::Configured)
            ),
            Some(UsbGadgetState::Configured)
        );
    }

    #[::fuchsia::test]
    async fn test_usb_sysfs_state_reads() {
        spawn_kernel_and_run(async |current_task| {
            let shared_state = Arc::new(AtomicU8::new(UsbGadgetState::Disconnected as u8));

            shared_state.store(UsbGadgetState::Configured as u8, Ordering::Relaxed);

            let sysfs_file = UsbStateSysfsFile { state: shared_state };
            let content = sysfs_file.read(current_task).expect("read failed");
            assert_eq!(content.as_ref(), b"CONFIGURED\n");
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_usb_device_state_to_fs_str() {
        assert_eq!(UsbGadgetState::Disconnected.to_fs_str(), b"DISCONNECTED");
        assert_eq!(UsbGadgetState::Configured.to_fs_str(), b"CONFIGURED");
        assert_eq!(UsbGadgetState::Connected.to_fs_str(), b"CONNECTED");
        assert_eq!(UsbGadgetState::Unknown.to_fs_str(), b"UNKNOWN");
    }

    #[::fuchsia::test]
    async fn test_usb_device_state_from_u8() {
        assert_eq!(UsbGadgetState::from(0), UsbGadgetState::Disconnected);
        assert_eq!(UsbGadgetState::from(1), UsbGadgetState::Configured);
        assert_eq!(UsbGadgetState::from(2), UsbGadgetState::Connected);
        assert_eq!(UsbGadgetState::from(255), UsbGadgetState::Unknown);
    }

    #[::fuchsia::test]
    async fn test_usb_device_init_sysfs() {
        spawn_kernel_and_run(async |current_task| {
            let device = usb_device_init(current_task.kernel()).expect("usb_device_init failed");

            assert_eq!(device.name.as_slice(), b"android0");
            let metadata = device.metadata.as_ref().expect("metadata not found");
            assert_eq!(metadata.devname.as_slice(), b"android0");
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_usb_device_metadata_property_injection() {
        spawn_kernel_and_run(async |current_task| {
            let device = usb_device_init(current_task.kernel()).expect("usb_device_init failed");

            let kernel = current_task.kernel();

            // Send the state change and wait for the background task to execute.
            dispatch_usb_state_change(
                kernel,
                &device,
                UsbGadgetState::Connected.to_fs_str().to_owned(),
            )
            .await
            .unwrap();

            let metadata = device.metadata.as_ref().expect("metadata not found");
            let scope = RcuReadScope::new();
            let value = metadata
                .properties
                .get(&scope, FsStr::new(b"USB_STATE"))
                .expect("property not found");
            assert_eq!(value.as_slice(), b"CONNECTED");
        })
        .await;
    }
}
