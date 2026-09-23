// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::InputFile;
use crate::input_event_relay::OpenedFiles;
use futures::FutureExt;
use starnix_core::device::kobject::DeviceMetadata;
use starnix_core::device::{DeviceMode, DeviceOps};
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::{FileOps, FsString, NamespaceNode};
use starnix_sync::{InputDeviceFileNodesLock, LockDepMutex};
use starnix_uapi::device_id::{DeviceId as StarnixDeviceId, INPUT_MAJOR};
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{BUS_VIRTUAL, input_id};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

// Add a fuchsia-specific vendor ID. 0xfc1a is currently not allocated
// to any vendor in the USB spec.
//
// May not be zero, see below.
const FUCHSIA_VENDOR_ID: u16 = 0xfc1a;

// May not be zero, see below.
const FUCHSIA_TOUCH_PRODUCT_ID: u16 = 0x2;

// May not be zero, see below.
const FUCHSIA_KEYBOARD_PRODUCT_ID: u16 = 0x1;

// May not be zero, see below.
const FUCHSIA_MOUSE_PRODUCT_ID: u16 = 0x3;

// Touch, keyboard, and mouse input IDs should be distinct.
// Per https://www.linuxjournal.com/article/6429, the bus type should be populated with a
// sensible value, but other fields may not be.
//
// While this may be the case for Linux itself, Android is not so relaxed.
// Devices with apparently-invalid vendor or product IDs don't get extra
// device configuration.  So we must make a minimum effort to present
// sensibly-looking product and vendor IDs.  Zero version only means that
// version-specific config files will not be applied.
//
// For background, see:
//
// * Allowable file locations:
//   https://source.android.com/docs/core/interaction/input/input-device-configuration-files#location
// * Android configuration selection code:
//   https://source.corp.google.com/h/googleplex-android/platform/superproject/main/+/main:frameworks/native/libs/input/InputDevice.cpp;l=60;drc=285211e60bff87fc5a9c9b4105a4b4ccb7edffaf
const TOUCH_INPUT_ID: input_id = input_id {
    bustype: BUS_VIRTUAL as u16,
    // Make sure that vendor ID and product ID at least seem plausible.  See
    // above for details.
    vendor: FUCHSIA_VENDOR_ID,
    product: FUCHSIA_TOUCH_PRODUCT_ID,
    // Version is OK to be zero, but config files named `Product_yyyy_Vendor_zzzz_Version_ttt.*`
    // will not work.
    version: 0,
};
const KEYBOARD_INPUT_ID: input_id = input_id {
    bustype: BUS_VIRTUAL as u16,
    // Make sure that vendor ID and product ID at least seem plausible.  See
    // above for details.
    vendor: FUCHSIA_VENDOR_ID,
    product: FUCHSIA_KEYBOARD_PRODUCT_ID,
    version: 1,
};

const MOUSE_INPUT_ID: input_id = input_id {
    bustype: BUS_VIRTUAL as u16,
    // Make sure that vendor ID and product ID at least seem plausible.  See
    // above for details.
    vendor: FUCHSIA_VENDOR_ID,
    product: FUCHSIA_MOUSE_PRODUCT_ID,
    version: 1,
};

#[derive(Clone)]
enum InputDeviceId {
    // A touch device, containing (display width, display height).
    Touch(i32, i32),

    // A keyboard device.
    Keyboard,

    // A mouse device.
    Mouse,
}

/// An [`InputDeviceStatus`] is tied to an [`InputDeviceBinding`] and provides properties
/// detailing its Inspect status.
/// We expect all (non-timestamp) properties' counts to equal the sum of that property from all
/// files opened on that device. So for example, if a device had 3 separate input files opened, we
/// would expect it's `total_fidl_events_received_count` to equal the sum of
/// `fidl_events_received_count` from all 3 files and so forth.
pub struct InputDeviceStatus {
    /// A node that contains the state below.
    pub node: fuchsia_inspect::Node,

    /// Hold onto inspect nodes for files opened on this device, so that when these files are
    /// closed, their inspect data is maintained.
    pub file_nodes: LockDepMutex<Vec<fuchsia_inspect::Node>, InputDeviceFileNodesLock>,

    /// The number of FIDL events received by this device from Fuchsia input system.
    ///
    /// We expect:
    /// total_fidl_events_received_count = total_fidl_events_ignored_count +
    ///                                    total_fidl_events_unexpected_count +
    ///                                    total_fidl_events_converted_count
    /// otherwise starnix ignored events unexpectedly.
    ///
    /// total_fidl_events_unexpected_count should be 0, if not it hints issues from upstream of
    /// ui stack.
    pub total_fidl_events_received_count: AtomicU64,

    /// The number of FIDL events ignored by this device when attempting conversion to this
    /// module’s representation of a TouchEvent.
    pub total_fidl_events_ignored_count: AtomicU64,

    /// The unexpected number of FIDL events reached to this module should be filtered out
    /// earlier in the UI stack.
    /// It maybe unexpected format or unexpected order.
    pub total_fidl_events_unexpected_count: AtomicU64,

    /// The number of FIDL events converted by this device to this module’s representation of
    /// TouchEvent.
    pub total_fidl_events_converted_count: AtomicU64,

    /// The number of uapi::input_events generated by this device from TouchEvents.
    pub total_uapi_events_generated_count: AtomicU64,

    /// The event time of the last generated uapi::input_event by one of this device's InputFiles.
    pub last_generated_uapi_event_timestamp_ns: AtomicI64,

    /// The number of events that entered with wake leases.
    pub total_events_with_wake_lease_count: AtomicU64,

    /// The number of active incoming wake leases.
    pub active_wake_leases_count: AtomicU64,
}

impl InputDeviceStatus {
    pub fn new(node: fuchsia_inspect::Node) -> Arc<Self> {
        let status = Arc::new(Self {
            node,
            file_nodes: Default::default(),
            total_fidl_events_received_count: AtomicU64::new(0),
            total_fidl_events_ignored_count: AtomicU64::new(0),
            total_fidl_events_unexpected_count: AtomicU64::new(0),
            total_fidl_events_converted_count: AtomicU64::new(0),
            total_uapi_events_generated_count: AtomicU64::new(0),
            last_generated_uapi_event_timestamp_ns: AtomicI64::new(0),
            total_events_with_wake_lease_count: AtomicU64::new(0),
            active_wake_leases_count: AtomicU64::new(0),
        });

        let weak_status = Arc::downgrade(&status);
        status.node.record_lazy_values("status", move || {
            let status = weak_status.upgrade();
            async move {
                let inspector = fuchsia_inspect::Inspector::default();
                if let Some(status) = status {
                    let root = inspector.root();
                    root.record_uint(
                        "total_fidl_events_received_count",
                        status.total_fidl_events_received_count.load(Ordering::Relaxed),
                    );
                    root.record_uint(
                        "total_fidl_events_ignored_count",
                        status.total_fidl_events_ignored_count.load(Ordering::Relaxed),
                    );
                    root.record_uint(
                        "total_fidl_events_unexpected_count",
                        status.total_fidl_events_unexpected_count.load(Ordering::Relaxed),
                    );
                    root.record_uint(
                        "total_fidl_events_converted_count",
                        status.total_fidl_events_converted_count.load(Ordering::Relaxed),
                    );
                    root.record_uint(
                        "total_uapi_events_generated_count",
                        status.total_uapi_events_generated_count.load(Ordering::Relaxed),
                    );
                    root.record_int(
                        "last_generated_uapi_event_timestamp_ns",
                        status.last_generated_uapi_event_timestamp_ns.load(Ordering::Relaxed),
                    );
                    root.record_uint(
                        "total_events_with_wake_lease_count",
                        status.total_events_with_wake_lease_count.load(Ordering::Relaxed),
                    );
                    root.record_uint(
                        "active_wake_leases_count",
                        status.active_wake_leases_count.load(Ordering::Relaxed),
                    );
                }
                Ok(inspector)
            }
            .boxed()
        });

        status
    }

    pub fn count_total_received_events(&self, count: u64) {
        self.total_fidl_events_received_count.fetch_add(count, Ordering::Relaxed);
    }

    pub fn count_events_with_wake_lease(&self, count: u64) {
        self.total_events_with_wake_lease_count.fetch_add(count, Ordering::Relaxed);
    }

    pub fn increment_active_wake_leases(&self, count: u64) {
        self.active_wake_leases_count.fetch_add(count, Ordering::Relaxed);
    }

    pub fn decrement_active_wake_leases(&self, count: u64) {
        self.active_wake_leases_count.fetch_sub(count, Ordering::Relaxed);
    }

    pub fn count_total_ignored_events(&self, count: u64) {
        self.total_fidl_events_ignored_count.fetch_add(count, Ordering::Relaxed);
    }

    pub fn count_total_unexpected_events(&self, count: u64) {
        self.total_fidl_events_unexpected_count.fetch_add(count, Ordering::Relaxed);
    }

    pub fn count_total_converted_events(&self, count: u64) {
        self.total_fidl_events_converted_count.fetch_add(count, Ordering::Relaxed);
    }

    pub fn count_total_generated_events(&self, count: u64, event_time_ns: i64) {
        self.total_uapi_events_generated_count.fetch_add(count, Ordering::Relaxed);
        self.last_generated_uapi_event_timestamp_ns.store(event_time_ns, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct InputDevice {
    device_type: InputDeviceId,

    pub open_files: OpenedFiles,

    pub inspect_status: Arc<InputDeviceStatus>,
}

impl InputDevice {
    pub fn new_touch(
        display_width: i32,
        display_height: i32,
        inspect_node: &fuchsia_inspect::Node,
    ) -> Self {
        let node = inspect_node.create_child("touch_device");
        InputDevice {
            device_type: InputDeviceId::Touch(display_width, display_height),
            open_files: Default::default(),
            inspect_status: InputDeviceStatus::new(node),
        }
    }

    pub fn new_keyboard(inspect_node: &fuchsia_inspect::Node) -> Self {
        let node = inspect_node.create_child("keyboard_device");
        InputDevice {
            device_type: InputDeviceId::Keyboard,
            open_files: Default::default(),
            inspect_status: InputDeviceStatus::new(node),
        }
    }

    pub fn new_mouse(inspect_node: &fuchsia_inspect::Node) -> Self {
        let node = inspect_node.create_child("mouse_device");
        InputDevice {
            device_type: InputDeviceId::Mouse,
            open_files: Default::default(),
            inspect_status: InputDeviceStatus::new(node),
        }
    }

    pub fn register(self, kernel: &Kernel, device_id: u32) -> Result<(), Errno> {
        let registry = &kernel.device_registry;

        let input_class = registry.objects.input_class();
        registry.register_device(
            kernel,
            FsString::from(format!("event{}", device_id)).as_ref(),
            DeviceMetadata::new(
                format!("input/event{}", device_id).into(),
                StarnixDeviceId::new(INPUT_MAJOR, device_id),
                DeviceMode::Char,
            ),
            input_class,
            self,
        )?;
        Ok(())
    }

    pub fn open_internal(&self) -> Box<dyn FileOps> {
        let input_file = match self.device_type {
            InputDeviceId::Touch(display_width, display_height) => {
                let mut file_nodes = self.inspect_status.file_nodes.lock();
                let child_node = self
                    .inspect_status
                    .node
                    .create_child(format!("touch_file_{}", file_nodes.len()));
                let file = Arc::new(InputFile::new_touch(
                    TOUCH_INPUT_ID,
                    display_width,
                    display_height,
                    &child_node,
                ));
                file_nodes.push(child_node);
                file
            }
            InputDeviceId::Keyboard => {
                let mut file_nodes = self.inspect_status.file_nodes.lock();
                let child_node = self
                    .inspect_status
                    .node
                    .create_child(format!("keyboard_file_{}", file_nodes.len()));
                let file = Arc::new(InputFile::new_keyboard(KEYBOARD_INPUT_ID, &child_node));
                file_nodes.push(child_node);
                file
            }
            InputDeviceId::Mouse => {
                let mut file_nodes = self.inspect_status.file_nodes.lock();
                let child_node = self
                    .inspect_status
                    .node
                    .create_child(format!("mouse_file_{}", file_nodes.len()));
                let file = Arc::new(InputFile::new_mouse(MOUSE_INPUT_ID, &child_node));
                file_nodes.push(child_node);
                file
            }
        };
        input_file.init_inspect_status();
        self.open_files.lock().push(Arc::downgrade(&input_file));
        Box::new(crate::input_file::ArcInputFile(input_file))
    }

    #[cfg(test)]
    pub fn open_test(
        &self,
        current_task: &CurrentTask,
    ) -> Result<starnix_core::vfs::FileHandle, Errno> {
        let input_file = self.open_internal();
        let root_namespace_node = current_task
            .lookup_path_from_root(".".into())
            .expect("failed to get namespace node for root");

        let file_object = starnix_core::vfs::FileObject::new(
            current_task,
            input_file,
            root_namespace_node,
            OpenFlags::empty(),
        )
        .expect("FileObject::new failed");
        Ok(file_object)
    }
}

impl DeviceOps for InputDevice {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _id: StarnixDeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let input_file = self.open_internal();
        Ok(input_file)
    }
}

#[cfg(test)]
mod test {
    #![allow(clippy::unused_unit)] // for compatibility with `test_case`

    use super::*;
    use crate::input_event_relay::{self, EventProxyMode};
    use anyhow::anyhow;
    use assert_matches::assert_matches;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fidl::endpoints::RequestStream as _;
    use fidl_fuchsia_ui_input::MediaButtonsEvent;
    use fidl_fuchsia_ui_input3 as fuiinput;
    use fidl_fuchsia_ui_pointer as fuipointer;
    use fidl_fuchsia_ui_policy as fuipolicy;
    use fuipointer::{
        EventPhase, TouchEvent, TouchInteractionId, TouchPointerSample, TouchSourceV2Marker,
    };
    use futures::StreamExt as _;
    use pretty_assertions::assert_eq;
    use starnix_core::mm::{MemoryAccessor as _, MemoryAccessorExt as _};
    use starnix_core::task::dynamic_thread_spawner::SpawnRequestBuilder;
    use starnix_core::task::{EventHandler, Waiter};
    use starnix_core::testing::{map_memory, spawn_kernel_and_run};
    use starnix_core::vfs::FileHandle;
    use starnix_core::vfs::buffers::VecOutputBuffer;
    use starnix_syscalls::SUCCESS;
    use starnix_types::time::timeval_from_time;
    use starnix_uapi::errors::EAGAIN;
    use starnix_uapi::uapi;
    use starnix_uapi::user_address::UserAddress;
    use starnix_uapi::vfs::FdEvents;
    use test_case::test_case;
    use test_util::assert_near;
    use zerocopy::FromBytes as _;

    const INPUT_EVENT_SIZE: usize = std::mem::size_of::<uapi::input_event>();

    async fn start_touch_input(
        current_task: &CurrentTask,
    ) -> (InputDevice, FileHandle, fuipointer::TouchSourceV2RequestStream) {
        let inspector = fuchsia_inspect::Inspector::default();
        start_touch_input_inspect_and_dimensions(current_task, 700, 1200, &inspector).await
    }

    async fn start_touch_input_inspect(
        current_task: &CurrentTask,
        inspector: &fuchsia_inspect::Inspector,
    ) -> (InputDevice, FileHandle, fuipointer::TouchSourceV2RequestStream) {
        start_touch_input_inspect_and_dimensions(current_task, 700, 1200, &inspector).await
    }

    async fn init_keyboard_listener(
        keyboard_stream: &mut fuiinput::KeyboardRequestStream,
    ) -> fuiinput::KeyboardListenerProxy {
        let keyboard_listener = match keyboard_stream.next().await {
            Some(Ok(fuiinput::KeyboardRequest::AddListener {
                view_ref: _,
                listener,
                responder,
            })) => {
                let _ = responder.send();
                listener.into_proxy()
            }
            _ => {
                panic!("Failed to get event");
            }
        };

        keyboard_listener
    }

    async fn init_button_listeners(
        device_listener_stream: &mut fuipolicy::DeviceListenerRegistryRequestStream,
    ) -> (fuipolicy::MediaButtonsListenerProxy, fuipolicy::TouchButtonsListenerProxy) {
        let media_buttons_listener = match device_listener_stream.next().await {
            Some(Ok(fuipolicy::DeviceListenerRegistryRequest::RegisterListener {
                listener,
                responder,
            })) => {
                let _ = responder.send();
                listener.into_proxy()
            }
            _ => {
                panic!("Failed to get event");
            }
        };

        let touch_buttons_listener = match device_listener_stream.next().await {
            Some(Ok(fuipolicy::DeviceListenerRegistryRequest::RegisterTouchButtonsListener {
                listener,
                responder,
            })) => {
                let _ = responder.send();
                listener.into_proxy()
            }
            _ => {
                panic!("Failed to get event");
            }
        };

        (media_buttons_listener, touch_buttons_listener)
    }

    async fn start_touch_input_inspect_and_dimensions(
        current_task: &CurrentTask,
        x_max: i32,
        y_max: i32,
        inspector: &fuchsia_inspect::Inspector,
    ) -> (InputDevice, FileHandle, fuipointer::TouchSourceV2RequestStream) {
        let input_device = InputDevice::new_touch(x_max, y_max, inspector.root());
        let input_file = input_device.open_test(current_task).expect("Failed to create input file");

        let (touch_source_client_end, touch_source_stream) =
            fidl::endpoints::create_request_stream::<TouchSourceV2Marker>();

        let (mouse_source_client_end, _mouse_source_stream) =
            fidl::endpoints::create_request_stream::<fuipointer::MouseSourceV2Marker>();

        let (keyboard_proxy, mut keyboard_stream) =
            fidl::endpoints::create_sync_proxy_and_stream::<fuiinput::KeyboardMarker>();
        let view_ref_pair =
            fuchsia_scenic::ViewRefPair::new().expect("Failed to create ViewRefPair");

        let (device_registry_proxy, mut device_listener_stream) =
            fidl::endpoints::create_sync_proxy_and_stream::<fuipolicy::DeviceListenerRegistryMarker>(
            );

        let (relay, _relay_handle) = input_event_relay::new_input_relay();
        relay.start_relays(
            &current_task.kernel(),
            EventProxyMode::None,
            touch_source_client_end,
            keyboard_proxy,
            mouse_source_client_end,
            view_ref_pair.view_ref,
            device_registry_proxy,
            input_device.open_files.clone(),
            Default::default(),
            Default::default(),
            Some(input_device.inspect_status.clone()),
            None,
            None,
        );

        let _ = init_keyboard_listener(&mut keyboard_stream).await;
        let _ = init_button_listeners(&mut device_listener_stream).await;

        (input_device, input_file, touch_source_stream)
    }

    async fn start_keyboard_input(
        current_task: &CurrentTask,
    ) -> (InputDevice, FileHandle, fuiinput::KeyboardListenerProxy) {
        let inspector = fuchsia_inspect::Inspector::default();
        start_keyboard_input_inspect(current_task, &inspector).await
    }

    async fn start_keyboard_input_inspect(
        current_task: &CurrentTask,
        inspector: &fuchsia_inspect::Inspector,
    ) -> (InputDevice, FileHandle, fuiinput::KeyboardListenerProxy) {
        let input_device = InputDevice::new_keyboard(inspector.root());
        let input_file = input_device.open_test(current_task).expect("Failed to create input file");
        let (keyboard_proxy, mut keyboard_stream) =
            fidl::endpoints::create_sync_proxy_and_stream::<fuiinput::KeyboardMarker>();
        let view_ref_pair =
            fuchsia_scenic::ViewRefPair::new().expect("Failed to create ViewRefPair");

        let (device_registry_proxy, mut device_listener_stream) =
            fidl::endpoints::create_sync_proxy_and_stream::<fuipolicy::DeviceListenerRegistryMarker>(
            );

        let (touch_source_client_end, _touch_source_stream) =
            fidl::endpoints::create_request_stream::<TouchSourceV2Marker>();

        let (mouse_source_client_end, _mouse_source_stream) =
            fidl::endpoints::create_request_stream::<fuipointer::MouseSourceV2Marker>();

        let (relay, _relay_handle) = input_event_relay::new_input_relay();
        relay.start_relays(
            current_task.kernel(),
            EventProxyMode::None,
            touch_source_client_end,
            keyboard_proxy,
            mouse_source_client_end,
            view_ref_pair.view_ref,
            device_registry_proxy,
            Default::default(),
            input_device.open_files.clone(),
            Default::default(),
            None,
            Some(input_device.inspect_status.clone()),
            None,
        );

        let keyboad_listener = init_keyboard_listener(&mut keyboard_stream).await;
        let _ = init_button_listeners(&mut device_listener_stream).await;

        (input_device, input_file, keyboad_listener)
    }

    async fn start_button_input(
        current_task: &CurrentTask,
    ) -> (InputDevice, FileHandle, fuipolicy::MediaButtonsListenerProxy) {
        let inspector = fuchsia_inspect::Inspector::default();
        start_button_input_inspect(current_task, &inspector).await
    }

    async fn start_button_input_inspect(
        current_task: &CurrentTask,
        inspector: &fuchsia_inspect::Inspector,
    ) -> (InputDevice, FileHandle, fuipolicy::MediaButtonsListenerProxy) {
        let input_device = InputDevice::new_keyboard(inspector.root());
        let input_file = input_device.open_test(current_task).expect("Failed to create input file");
        let (device_registry_proxy, mut device_listener_stream) =
            fidl::endpoints::create_sync_proxy_and_stream::<fuipolicy::DeviceListenerRegistryMarker>(
            );

        let (touch_source_client_end, _touch_source_stream) =
            fidl::endpoints::create_request_stream::<TouchSourceV2Marker>();
        let (mouse_source_client_end, _mouse_source_stream) =
            fidl::endpoints::create_request_stream::<fuipointer::MouseSourceV2Marker>();
        let (keyboard_proxy, mut keyboard_stream) =
            fidl::endpoints::create_sync_proxy_and_stream::<fuiinput::KeyboardMarker>();
        let view_ref_pair =
            fuchsia_scenic::ViewRefPair::new().expect("Failed to create ViewRefPair");

        let (relay, _relay_handle) = input_event_relay::new_input_relay();
        relay.start_relays(
            current_task.kernel(),
            EventProxyMode::None,
            touch_source_client_end,
            keyboard_proxy,
            mouse_source_client_end,
            view_ref_pair.view_ref,
            device_registry_proxy,
            Default::default(),
            input_device.open_files.clone(),
            Default::default(),
            None,
            Some(input_device.inspect_status.clone()),
            None,
        );

        let _ = init_keyboard_listener(&mut keyboard_stream).await;
        let (button_listener, _) = init_button_listeners(&mut device_listener_stream).await;

        (input_device, input_file, button_listener)
    }

    async fn start_mouse_input(
        current_task: &CurrentTask,
    ) -> (InputDevice, FileHandle, fuipointer::MouseSourceV2RequestStream) {
        let inspector = fuchsia_inspect::Inspector::default();
        start_mouse_input_inspect(current_task, &inspector).await
    }

    async fn start_mouse_input_inspect(
        current_task: &CurrentTask,
        inspector: &fuchsia_inspect::Inspector,
    ) -> (InputDevice, FileHandle, fuipointer::MouseSourceV2RequestStream) {
        let input_device = InputDevice::new_mouse(inspector.root());
        let input_file = input_device.open_test(current_task).expect("Failed to create input file");

        let (touch_source_client_end, _touch_source_stream) =
            fidl::endpoints::create_request_stream::<TouchSourceV2Marker>();

        let (mouse_source_client_end, mouse_source_stream) =
            fidl::endpoints::create_request_stream::<fuipointer::MouseSourceV2Marker>();

        let (keyboard_proxy, mut keyboard_stream) =
            fidl::endpoints::create_sync_proxy_and_stream::<fuiinput::KeyboardMarker>();
        let view_ref_pair =
            fuchsia_scenic::ViewRefPair::new().expect("Failed to create ViewRefPair");

        let (device_registry_proxy, mut device_listener_stream) =
            fidl::endpoints::create_sync_proxy_and_stream::<fuipolicy::DeviceListenerRegistryMarker>(
            );

        let (relay, _relay_handle) = input_event_relay::new_input_relay();
        relay.start_relays(
            &current_task.kernel(),
            EventProxyMode::None,
            touch_source_client_end,
            keyboard_proxy,
            mouse_source_client_end,
            view_ref_pair.view_ref,
            device_registry_proxy,
            Default::default(),
            Default::default(),
            input_device.open_files.clone(),
            None,
            None,
            Some(input_device.inspect_status.clone()),
        );

        let _ = init_keyboard_listener(&mut keyboard_stream).await;
        let _ = init_button_listeners(&mut device_listener_stream).await;

        (input_device, input_file, mouse_source_stream)
    }

    fn make_touch_event(pointer_id: u32) -> fuipointer::TouchEvent {
        // Default to `Change`, because that has the fewest side effects.
        make_touch_event_with_phase(EventPhase::Change, pointer_id)
    }

    fn make_touch_event_with_phase(phase: EventPhase, pointer_id: u32) -> fuipointer::TouchEvent {
        make_touch_event_with_coords_phase(0.0, 0.0, phase, pointer_id)
    }

    fn make_touch_event_with_coords_phase(
        x: f32,
        y: f32,
        phase: EventPhase,
        pointer_id: u32,
    ) -> fuipointer::TouchEvent {
        make_touch_event_with_coords_phase_timestamp(x, y, phase, pointer_id, 0)
    }

    fn make_touch_event_with_coords(x: f32, y: f32, pointer_id: u32) -> fuipointer::TouchEvent {
        make_touch_event_with_coords_phase(x, y, EventPhase::Change, pointer_id)
    }

    fn make_touch_event_with_coords_phase_timestamp(
        x: f32,
        y: f32,
        phase: EventPhase,
        pointer_id: u32,
        time_nanos: i64,
    ) -> fuipointer::TouchEvent {
        make_touch_event_with_coords_phase_timestamp_device_id(
            x, y, phase, pointer_id, time_nanos, 0,
        )
    }

    fn make_empty_touch_event() -> fuipointer::TouchEvent {
        TouchEvent {
            pointer_sample: Some(TouchPointerSample {
                interaction: Some(TouchInteractionId {
                    pointer_id: 0,
                    device_id: 0,
                    interaction_id: 0,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn make_touch_event_with_coords_phase_timestamp_device_id(
        x: f32,
        y: f32,
        phase: EventPhase,
        pointer_id: u32,
        time_nanos: i64,
        device_id: u32,
    ) -> fuipointer::TouchEvent {
        TouchEvent {
            timestamp: Some(time_nanos),
            pointer_sample: Some(TouchPointerSample {
                position_in_viewport: Some([x, y]),
                // Default to `Change`, because that has the fewest side effects.
                phase: Some(phase),
                interaction: Some(TouchInteractionId { pointer_id, device_id, interaction_id: 0 }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn make_mouse_wheel_event(ticks: i64) -> fuipointer::MouseEvent {
        make_mouse_wheel_event_with_timestamp(ticks, 0)
    }

    fn make_mouse_wheel_event_with_timestamp(ticks: i64, timestamp: i64) -> fuipointer::MouseEvent {
        fuipointer::MouseEvent {
            timestamp: Some(timestamp),
            pointer_sample: Some(fuipointer::MousePointerSample {
                device_id: Some(0),
                scroll_v: Some(ticks),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn read_uapi_events(file: &FileHandle, current_task: &CurrentTask) -> Vec<uapi::input_event> {
        std::iter::from_fn(|| {
            let mut event_bytes = VecOutputBuffer::new(INPUT_EVENT_SIZE);
            match file.read(current_task, &mut event_bytes) {
                Ok(INPUT_EVENT_SIZE) => Some(
                    uapi::input_event::read_from_bytes(Vec::from(event_bytes).as_slice())
                        .map_err(|_| anyhow!("failed to read input_event from buffer")),
                ),
                Ok(other_size) => {
                    Some(Err(anyhow!("got {} bytes (expected {})", other_size, INPUT_EVENT_SIZE)))
                }
                Err(Errno { code: EAGAIN, .. }) => None,
                Err(other_error) => Some(Err(anyhow!("read failed: {:?}", other_error))),
            }
        })
        .enumerate()
        .map(|(i, read_res)| match read_res {
            Ok(event) => event,
            Err(e) => panic!("unexpected result {:?} on iteration {}", e, i),
        })
        .collect()
    }

    // Sends `touch_events` to the client stream and waits for `AcknowledgeEvents`.
    async fn answer_next_touch_watch_request(
        request_stream: &mut fuipointer::TouchSourceV2RequestStream,
        touch_events: Vec<TouchEvent>,
    ) {
        let control_handle = request_stream.control_handle();
        control_handle
            .send_on_touch_events(touch_events, 1)
            .expect("failure sending OnTouchEvents");
        match request_stream.next().await {
            Some(Ok(fuipointer::TouchSourceV2Request::AcknowledgeEvents { .. })) => {}
            unexpected_request => panic!("unexpected request {:?}", unexpected_request),
        }
    }

    // Sends `mouse_events` to the client stream and waits for `AcknowledgeEvents`.
    async fn answer_next_mouse_watch_request(
        request_stream: &mut fuipointer::MouseSourceV2RequestStream,
        mouse_events: Vec<fuipointer::MouseEvent>,
    ) {
        let control_handle = request_stream.control_handle();
        control_handle
            .send_on_mouse_events(mouse_events, 1)
            .expect("failure sending OnMouseEvents");
        match request_stream.next().await {
            Some(Ok(fuipointer::MouseSourceV2Request::AcknowledgeEvents { .. })) => {}
            unexpected_request => panic!("unexpected request {:?}", unexpected_request),
        }
    }

    #[::fuchsia::test]
    async fn touch_source_v2_acknowledges_events_with_matching_stamp() {
        spawn_kernel_and_run(async move |current_task| {
            let (_input_device, _input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;

            let control_handle = touch_source_stream.control_handle();
            control_handle
                .send_on_touch_events(vec![make_empty_touch_event(), make_empty_touch_event()], 10)
                .expect("failure sending OnTouchEvents");
            match touch_source_stream.next().await {
                Some(Ok(fuipointer::TouchSourceV2Request::AcknowledgeEvents {
                    last_acknowledged_event_stamp,
                    ..
                })) => assert_eq!(last_acknowledged_event_stamp, 10),
                unexpected => panic!("unexpected request {:?}", unexpected),
            }

            control_handle
                .send_on_touch_events(
                    vec![
                        make_empty_touch_event(),
                        make_empty_touch_event(),
                        make_empty_touch_event(),
                        make_empty_touch_event(),
                        make_empty_touch_event(),
                    ],
                    20,
                )
                .expect("failure sending OnTouchEvents");
            match touch_source_stream.next().await {
                Some(Ok(fuipointer::TouchSourceV2Request::AcknowledgeEvents {
                    last_acknowledged_event_stamp,
                    ..
                })) => assert_eq!(last_acknowledged_event_stamp, 20),
                unexpected => panic!("unexpected request {:?}", unexpected),
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn notifies_polling_waiters_of_new_data() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;
            let waiter1 = Waiter::new();
            let waiter2 = Waiter::new();

            // Ask `input_file` to notify waiters when data is available to read.
            [&waiter1, &waiter2].iter().for_each(|waiter| {
                input_file.wait_async(&current_task, waiter, FdEvents::POLLIN, EventHandler::None);
            });
            assert_matches!(waiter1.wait_until(&current_task, zx::MonotonicInstant::ZERO), Err(_));
            assert_matches!(waiter2.wait_until(&current_task, zx::MonotonicInstant::ZERO), Err(_));

            // Reply to first `Watch` request.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Add, 1)],
            )
            .await;
            answer_next_touch_watch_request(&mut touch_source_stream, vec![make_touch_event(1)])
                .await;

            // `InputFile` should be done processing the first reply, since it has sent its second
            // request. And, as part of processing the first reply, `InputFile` should have notified
            // the interested waiters.
            assert_eq!(waiter1.wait_until(&current_task, zx::MonotonicInstant::ZERO), Ok(()));
            assert_eq!(waiter2.wait_until(&current_task, zx::MonotonicInstant::ZERO), Ok(()));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn notifies_blocked_waiter_of_new_data() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.
            let kernel = current_task.kernel().clone();
            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;
            let waiter = Waiter::new();

            // Ask `input_file` to notify `waiter` when data is available to read.
            input_file.wait_async(&current_task, &waiter, FdEvents::POLLIN, EventHandler::None);

            let closure = move |task: &CurrentTask| waiter.wait(&task);

            let (waiter_thread, req) = SpawnRequestBuilder::new()
                .with_debug_name("input-device-waiter")
                .with_sync_closure(closure)
                .build_with_async_result();
            kernel.kthreads.spawner().spawn_from_request(req);

            let mut waiter_thread = Box::pin(waiter_thread);
            assert_matches!(futures::poll!(&mut waiter_thread), futures::task::Poll::Pending);

            // Reply to first `Watch` request.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Add, 1)],
            )
            .await;

            // Wait for another `Watch`.
            //
            // TODO(https://fxbug.dev/42075452): Without this, `relay_thread` gets stuck `await`-ing
            // the reply to its first request. Figure out why that happens, and remove this second
            // reply.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![make_touch_event(1)])
                .await;
        })
        .await;
    }

    #[::fuchsia::test]
    async fn does_not_notify_polling_waiters_without_new_data() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;
            let waiter1 = Waiter::new();
            let waiter2 = Waiter::new();

            // Ask `input_file` to notify waiters when data is available to read.
            [&waiter1, &waiter2].iter().for_each(|waiter| {
                input_file.wait_async(&current_task, waiter, FdEvents::POLLIN, EventHandler::None);
            });
            assert_matches!(waiter1.wait_until(&current_task, zx::MonotonicInstant::ZERO), Err(_));
            assert_matches!(waiter2.wait_until(&current_task, zx::MonotonicInstant::ZERO), Err(_));

            // Reply to first `Watch` request with an empty set of events.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![]).await;

            // `InputFile` should be done processing the first reply. Since there
            // were no touch_events given, `InputFile` should not have notified the
            // interested waiters.
            assert_matches!(waiter1.wait_until(&current_task, zx::MonotonicInstant::ZERO), Err(_));
            assert_matches!(waiter2.wait_until(&current_task, zx::MonotonicInstant::ZERO), Err(_));
        })
        .await;
    }

    // Note: a user program may also want to be woken if events were already ready at the
    // time that the program called `epoll_wait()`. However, there's no test for that case
    // in this module, because:
    //
    // 1. Not all programs will want to be woken in such a case. In particular, some programs
    //    use "edge-triggered" mode instead of "level-tiggered" mode. For details on the
    //    two modes, see https://man7.org/linux/man-pages/man7/epoll.7.html.
    // 2. For programs using "level-triggered" mode, the relevant behavior is implemented in
    //    the `epoll` module, and verified by `epoll::tests::test_epoll_ready_then_wait()`.
    //
    // See also: the documentation for `FileOps::wait_async()`.

    #[::fuchsia::test]
    async fn honors_wait_cancellation() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up input resources.

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;
            let waiter1 = Waiter::new();
            let waiter2 = Waiter::new();

            // Ask `input_file` to notify `waiter` when data is available to read.
            let waitkeys = [&waiter1, &waiter2]
                .iter()
                .map(|waiter| {
                    input_file
                        .wait_async(&current_task, waiter, FdEvents::POLLIN, EventHandler::None)
                        .expect("wait_async")
                })
                .collect::<Vec<_>>();

            // Cancel wait for `waiter1`.
            waitkeys.into_iter().next().expect("failed to get first waitkey").cancel();

            // Reply to first `Watch` request.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Add, 1)],
            )
            .await;
            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![make_touch_event(1)])
                .await;

            // `InputFile` should be done processing the first reply, since it has sent its second
            // request. And, as part of processing the first reply, `InputFile` should have notified
            // the interested waiters.
            assert_matches!(waiter1.wait_until(&current_task, zx::MonotonicInstant::ZERO), Err(_));
            assert_eq!(waiter2.wait_until(&current_task, zx::MonotonicInstant::ZERO), Ok(()));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn query_events() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;

            // Check initial expectation.
            assert_eq!(
                input_file.query_events(&current_task).expect("query_events"),
                FdEvents::empty(),
                "events should be empty before data arrives"
            );

            // Reply to first `Watch` request.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Add, 1)],
            )
            .await;

            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![make_touch_event(1)])
                .await;

            // Check post-watch expectation.
            assert_eq!(
                input_file.query_events(&current_task).expect("query_events"),
                FdEvents::POLLIN | FdEvents::POLLRDNORM,
                "events should be POLLIN after data arrives"
            );
        })
        .await;
    }

    fn make_uapi_input_event(ty: u32, code: u32, value: i32) -> uapi::input_event {
        make_uapi_input_event_with_timestamp(ty, code, value, 0)
    }

    fn make_uapi_input_event_with_timestamp(
        ty: u32,
        code: u32,
        value: i32,
        time_nanos: i64,
    ) -> uapi::input_event {
        uapi::input_event {
            time: timeval_from_time(zx::MonotonicInstant::from_nanos(time_nanos)),
            type_: ty as u16,
            code: code as u16,
            value,
        }
    }

    #[::fuchsia::test]
    async fn touch_event_ignored() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.
            let inspector = fuchsia_inspect::Inspector::default();

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input_inspect(&current_task, &inspector).await;

            // Touch add for pointer 1. This should be counted as a received event and a converted
            // event. It should also yield 6 generated events.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Add, 1)],
            )
            .await;

            // Wait for another `Watch` to ensure input_file done processing the first reply.
            // Use an empty `TouchEvent`, to minimize the chance that this event creates unexpected
            // `uapi::input_event`s. This should be counted as a received event and an ignored event.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_empty_touch_event()],
            )
            .await;

            // Consume all of the `uapi::input_event`s that are available.
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(events.len(), 6);

            // Reply to `Watch` request of empty event. This should be counted as a received event and
            // an ignored event.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_empty_touch_event()],
            )
            .await;

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(events, vec![]);
            assert_data_tree!(inspector, root: {
                touch_device: {
                    active_wake_leases_count: 0u64,
                    total_events_with_wake_lease_count: 0u64,
                    total_fidl_events_received_count: 3u64,
                    total_fidl_events_ignored_count: 2u64,
                    total_fidl_events_unexpected_count: 0u64,
                    total_fidl_events_converted_count: 1u64,
                    total_uapi_events_generated_count: 6u64,
                    last_generated_uapi_event_timestamp_ns: 0i64,
                    touch_file_0: {
                        fidl_events_received_count: 3u64,
                        fidl_events_ignored_count: 2u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 1u64,
                        uapi_events_generated_count: 6u64,
                        uapi_events_read_count: 6u64,
                        fd_read_count: 8u64,
                        fd_notify_count: 1u64,
                        last_generated_uapi_event_timestamp_ns: 0i64,
                        last_read_uapi_event_timestamp_ns: 0i64,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    },
                }
            });
        })
        .await;
    }

    #[test_case(make_touch_event_with_phase(EventPhase::Add, 1); "touch add for pointer already added")]
    #[test_case(make_touch_event_with_phase(EventPhase::Change, 2); "touch change for pointer not added")]
    #[test_case(make_touch_event_with_phase(EventPhase::Remove, 2); "touch remove for pointer not added")]
    #[test_case(make_touch_event_with_phase(EventPhase::Cancel, 1); "touch cancel")]
    #[::fuchsia::test]
    async fn touch_event_unexpected(event: TouchEvent) {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.
            let inspector = fuchsia_inspect::Inspector::default();

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input_inspect(&current_task, &inspector).await;

            // Touch add for pointer 1. This should be counted as a received event and a converted
            // event. It should also yield 6 generated events.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Add, 1)],
            )
            .await;

            // Wait for another `Watch` to ensure input_file done processing the first reply.
            // Use an empty `TouchEvent`, to minimize the chance that this event creates unexpected
            // `uapi::input_event`s. This should be counted as a received event and an ignored event.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_empty_touch_event()],
            )
            .await;

            // Consume all of the `uapi::input_event`s that are available.
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(events.len(), 6);

            // Reply to `Watch` request of given event. This should be counted as a received event and
            // an unexpected event.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![event]).await;

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(events, vec![]);
            assert_data_tree!(inspector, root: {
                touch_device: {
                    active_wake_leases_count: 0u64,
                    total_events_with_wake_lease_count: 0u64,
                    total_fidl_events_received_count: 3u64,
                    total_fidl_events_ignored_count: 1u64,
                    total_fidl_events_unexpected_count: 1u64,
                    total_fidl_events_converted_count: 1u64,
                    total_uapi_events_generated_count: 6u64,
                    last_generated_uapi_event_timestamp_ns: 0i64,
                    touch_file_0: {
                        fidl_events_received_count: 3u64,
                        fidl_events_ignored_count: 1u64,
                        fidl_events_unexpected_count: 1u64,
                        fidl_events_converted_count: 1u64,
                        uapi_events_generated_count: 6u64,
                        uapi_events_read_count: 6u64,
                        fd_read_count: 8u64,
                        fd_notify_count: 1u64,
                        last_generated_uapi_event_timestamp_ns: 0i64,
                        last_read_uapi_event_timestamp_ns: 0i64,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    },
                }
            });
        })
        .await;
    }

    #[::fuchsia::test]
    async fn translates_touch_add() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;

            // Touch add for pointer 1.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Add, 1)],
            )
            .await;

            // Wait for another `Watch` to ensure input_file done processing the first reply.
            // Use an empty `TouchEvent`, to minimize the chance that this event
            // creates unexpected `uapi::input_event`s.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_empty_touch_event()],
            )
            .await;

            // Consume all of the `uapi::input_event`s that are available.
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(
                events,
                vec![
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 0),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 0),
                    make_uapi_input_event(uapi::EV_KEY, uapi::BTN_TOUCH, 1),
                    make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
                ]
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn translates_touch_change() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;

            // Touch add for pointer 1.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Add, 1)],
            )
            .await;

            // Wait for another `Watch` to ensure input_file done processing the first reply.
            // Use an empty `TouchEvent`, to minimize the chance that this event
            // creates unexpected `uapi::input_event`s.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;

            // Consume all of the `uapi::input_event`s that are available.
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(events.len(), 6);

            // Reply to touch change.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_coords(10.0, 20.0, 1)],
            )
            .await;

            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(
                events,
                vec![
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 10),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 20),
                    make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
                ]
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn translates_touch_remove() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;

            // Touch add for pointer 1.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Add, 1)],
            )
            .await;

            // Wait for another `Watch` to ensure input_file done processing the first reply.
            // Use an empty `TouchEvent`, to minimize the chance that this event
            // creates unexpected `uapi::input_event`s.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;

            // Consume all of the `uapi::input_event`s that are available.
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(events.len(), 6);

            // Reply to touch change.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Remove, 1)],
            )
            .await;

            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(
                events,
                vec![
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, -1),
                    make_uapi_input_event(uapi::EV_KEY, uapi::BTN_TOUCH, 0),
                    make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
                ]
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn multi_touch_event_sequence() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;

            // Touch add for pointer 1.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Add, 1)],
            )
            .await;

            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(events.len(), 6);

            // Touch add for pointer 2.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![
                    make_touch_event_with_coords(10.0, 20.0, 1),
                    make_touch_event_with_phase(EventPhase::Add, 2),
                ],
            )
            .await;

            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(
                events,
                vec![
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 10),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 20),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 2),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 0),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 0),
                    make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
                ]
            );

            // Both pointers move.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![
                    make_touch_event_with_coords(11.0, 21.0, 1),
                    make_touch_event_with_coords(101.0, 201.0, 2),
                ],
            )
            .await;

            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(
                events,
                vec![
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 11),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 21),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 101),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 201),
                    make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
                ]
            );

            // Pointer 1 up.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![
                    make_touch_event_with_phase(EventPhase::Remove, 1),
                    make_touch_event_with_coords(102.0, 202.0, 2),
                ],
            )
            .await;

            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(
                events,
                vec![
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, -1),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 102),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 202),
                    make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
                ]
            );

            // Pointer 2 up.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase(EventPhase::Remove, 2)],
            )
            .await;

            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(
                events,
                vec![
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 1),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, -1),
                    make_uapi_input_event(uapi::EV_KEY, uapi::BTN_TOUCH, 0),
                    make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
                ]
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn multi_event_sequence_unsorted_in_one_watch() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;

            // Touch add for pointer 1.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![
                    make_touch_event_with_coords_phase_timestamp(
                        10.0,
                        20.0,
                        EventPhase::Change,
                        1,
                        100,
                    ),
                    make_touch_event_with_coords_phase_timestamp(0.0, 0.0, EventPhase::Add, 1, 1),
                ],
            )
            .await;

            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;
            let events = read_uapi_events(&input_file, &current_task);

            assert_eq!(
                events,
                vec![
                    make_uapi_input_event_with_timestamp(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0, 1),
                    make_uapi_input_event_with_timestamp(
                        uapi::EV_ABS,
                        uapi::ABS_MT_TRACKING_ID,
                        1,
                        1
                    ),
                    make_uapi_input_event_with_timestamp(
                        uapi::EV_ABS,
                        uapi::ABS_MT_POSITION_X,
                        0,
                        1
                    ),
                    make_uapi_input_event_with_timestamp(
                        uapi::EV_ABS,
                        uapi::ABS_MT_POSITION_Y,
                        0,
                        1
                    ),
                    make_uapi_input_event_with_timestamp(uapi::EV_KEY, uapi::BTN_TOUCH, 1, 1),
                    make_uapi_input_event_with_timestamp(uapi::EV_SYN, uapi::SYN_REPORT, 0, 1),
                    make_uapi_input_event_with_timestamp(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0, 100),
                    make_uapi_input_event_with_timestamp(
                        uapi::EV_ABS,
                        uapi::ABS_MT_POSITION_X,
                        10,
                        100
                    ),
                    make_uapi_input_event_with_timestamp(
                        uapi::EV_ABS,
                        uapi::ABS_MT_POSITION_Y,
                        20,
                        100
                    ),
                    make_uapi_input_event_with_timestamp(uapi::EV_SYN, uapi::SYN_REPORT, 0, 100),
                ]
            );
        })
        .await;
    }

    #[test_case((0.0, 0.0); "origin")]
    #[test_case((100.7, 200.7); "above midpoint")]
    #[test_case((100.3, 200.3); "below midpoint")]
    #[test_case((100.5, 200.5); "midpoint")]
    #[::fuchsia::test]
    async fn sends_acceptable_coordinates((x, y): (f32, f32)) {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.
            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;

            // Touch add.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_coords_phase(x, y, EventPhase::Add, 1)],
            )
            .await;

            // Wait for another `Watch`.
            answer_next_touch_watch_request(&mut touch_source_stream, vec![TouchEvent::default()])
                .await;
            let events = read_uapi_events(&input_file, &current_task);

            // Check that the reported positions are within the acceptable error. The acceptable
            // error is chosen to allow either rounding or truncation.
            const ACCEPTABLE_ERROR: f32 = 1.0;
            let actual_x = events
                .iter()
                .find(|event| {
                    event.type_ == uapi::EV_ABS as u16
                        && event.code == uapi::ABS_MT_POSITION_X as u16
                })
                .unwrap_or_else(|| panic!("did not find `ABS_X` event in {:?}", events))
                .value;
            let actual_y = events
                .iter()
                .find(|event| {
                    event.type_ == uapi::EV_ABS as u16
                        && event.code == uapi::ABS_MT_POSITION_Y as u16
                })
                .unwrap_or_else(|| panic!("did not find `ABS_Y` event in {:?}", events))
                .value;
            assert_near!(x, actual_x as f32, ACCEPTABLE_ERROR);
            assert_near!(y, actual_y as f32, ACCEPTABLE_ERROR);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn sends_appropriate_ack_to_touch_source_server() {
        spawn_kernel_and_run(async move |current_task| {
            let (_input_device, _input_file, mut touch_source_stream) =
                start_touch_input(&current_task).await;

            let control_handle = touch_source_stream.control_handle();
            control_handle
                .send_on_touch_events(vec![make_touch_event_with_phase(EventPhase::Add, 2)], 42)
                .expect("failure sending OnTouchEvents");
            match touch_source_stream.next().await {
                Some(Ok(fuipointer::TouchSourceV2Request::AcknowledgeEvents {
                    last_acknowledged_event_stamp,
                    ..
                })) => assert_eq!(last_acknowledged_event_stamp, 42),
                unexpected => panic!("unexpected request {:?}", unexpected),
            }
        })
        .await;
    }

    #[test_case(fidl_fuchsia_input::Key::Escape, uapi::KEY_POWER; "Esc maps to Power")]
    #[test_case(fidl_fuchsia_input::Key::A, uapi::KEY_A; "A maps to A")]
    #[::fuchsia::test]
    async fn sends_keyboard_events(fkey: fidl_fuchsia_input::Key, lkey: u32) {
        spawn_kernel_and_run(async move |current_task| {
            let (_keyboard_device, keyboard_file, keyboard_listener) =
                start_keyboard_input(&current_task).await;

            let key_event = fuiinput::KeyEvent {
                timestamp: Some(0),
                type_: Some(fuiinput::KeyEventType::Pressed),
                key: Some(fkey),
                ..Default::default()
            };

            let _ = keyboard_listener.on_key_event(&key_event).await;
            std::mem::drop(keyboard_listener); // Close Zircon channel.
            let events = read_uapi_events(&keyboard_file, &current_task);
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].code, lkey as u16);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn skips_unknown_keyboard_events() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();
            let (_keyboard_device, keyboard_file, keyboard_listener) =
                start_keyboard_input_inspect(&current_task, &inspector).await;

            let key_event = fuiinput::KeyEvent {
                timestamp: Some(0),
                type_: Some(fuiinput::KeyEventType::Pressed),
                key: Some(fidl_fuchsia_input::Key::AcRefresh),
                ..Default::default()
            };

            let _ = keyboard_listener.on_key_event(&key_event).await;
            std::mem::drop(keyboard_listener); // Close Zircon channel.
            let events = read_uapi_events(&keyboard_file, &current_task);
            assert_eq!(events.len(), 0);

            // The unknown key is counted as received-but-ignored, and must not be
            // counted as converted.
            assert_data_tree!(inspector, root: {
                keyboard_device: {
                    active_wake_leases_count: AnyProperty,
                    total_events_with_wake_lease_count: AnyProperty,
                    total_fidl_events_received_count: 1u64,
                    total_fidl_events_ignored_count: 1u64,
                    total_fidl_events_unexpected_count: 0u64,
                    total_fidl_events_converted_count: 0u64,
                    total_uapi_events_generated_count: 0u64,
                    last_generated_uapi_event_timestamp_ns: 0i64,
                    keyboard_file_0: {
                        fidl_events_received_count: 1u64,
                        fidl_events_ignored_count: 1u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 0u64,
                        uapi_events_generated_count: 0u64,
                        uapi_events_read_count: 0u64,
                        fd_read_count: AnyProperty,
                        fd_notify_count: AnyProperty,
                        last_generated_uapi_event_timestamp_ns: 0i64,
                        last_read_uapi_event_timestamp_ns: AnyProperty,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    }
                }
            });
        })
        .await;
    }

    #[::fuchsia::test]
    async fn sends_power_button_events() {
        spawn_kernel_and_run(async move |current_task| {
            let (_input_device, input_file, buttons_listener) =
                start_button_input(&current_task).await;

            let power_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(true),
                function: Some(false),
                ..Default::default()
            };

            let _ = buttons_listener.on_event(power_event).await;
            std::mem::drop(buttons_listener); // Close Zircon channel.

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].code, uapi::KEY_POWER as u16);
            assert_eq!(events[0].value, 1);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn sends_function_button_events() {
        spawn_kernel_and_run(async move |current_task| {
            let (_input_device, input_file, buttons_listener) =
                start_button_input(&current_task).await;

            let function_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(false),
                function: Some(true),
                ..Default::default()
            };

            let _ = buttons_listener.on_event(function_event).await;
            std::mem::drop(buttons_listener); // Close Zircon channel.

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].code, uapi::KEY_VOLUMEDOWN as u16);
            assert_eq!(events[0].value, 1);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn sends_overlapping_button_events() {
        spawn_kernel_and_run(async move |current_task| {
            let (_input_device, input_file, buttons_listener) =
                start_button_input(&current_task).await;

            let power_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(true),
                function: Some(false),
                ..Default::default()
            };

            let function_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(true),
                function: Some(true),
                ..Default::default()
            };

            let function_release_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(true),
                function: Some(false),
                ..Default::default()
            };

            let power_release_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(false),
                function: Some(false),
                ..Default::default()
            };

            let _ = buttons_listener.on_event(power_event).await;
            let _ = buttons_listener.on_event(function_event).await;
            let _ = buttons_listener.on_event(function_release_event).await;
            let _ = buttons_listener.on_event(power_release_event).await;
            std::mem::drop(buttons_listener); // Close Zircon channel.

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(events.len(), 8);
            assert_eq!(events[0].code, uapi::KEY_POWER as u16);
            assert_eq!(events[0].value, 1);
            assert_eq!(events[2].code, uapi::KEY_VOLUMEDOWN as u16);
            assert_eq!(events[2].value, 1);
            assert_eq!(events[4].code, uapi::KEY_VOLUMEDOWN as u16);
            assert_eq!(events[4].value, 0);
            assert_eq!(events[6].code, uapi::KEY_POWER as u16);
            assert_eq!(events[6].value, 0);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn sends_simultaneous_button_events() {
        spawn_kernel_and_run(async move |current_task| {
            let (_input_device, input_file, buttons_listener) =
                start_button_input(&current_task).await;

            let power_and_function_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(true),
                function: Some(true),
                ..Default::default()
            };

            let _ = buttons_listener.on_event(power_and_function_event).await;
            std::mem::drop(buttons_listener); // Close Zircon channel.

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(events.len(), 4);
            assert_eq!(events[0].code, uapi::KEY_POWER as u16);
            assert_eq!(events[0].value, 1);
            assert_eq!(events[2].code, uapi::KEY_VOLUMEDOWN as u16);
            assert_eq!(events[2].value, 1);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn sends_volume_up_button_events() {
        spawn_kernel_and_run(async move |current_task| {
            let (_input_device, input_file, buttons_listener) =
                start_button_input(&current_task).await;

            let volume_up_event = MediaButtonsEvent {
                volume: Some(1),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(false),
                function: Some(false),
                ..Default::default()
            };

            let _ = buttons_listener.on_event(volume_up_event).await;
            std::mem::drop(buttons_listener); // Close Zircon channel.

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].code, uapi::KEY_VOLUMEUP as u16);
            assert_eq!(events[0].value, 1);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn sends_volume_down_button_events() {
        spawn_kernel_and_run(async move |current_task| {
            let (_input_device, input_file, buttons_listener) =
                start_button_input(&current_task).await;

            let volume_down_event = MediaButtonsEvent {
                volume: Some(-1),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(false),
                function: Some(false),
                ..Default::default()
            };

            let _ = buttons_listener.on_event(volume_down_event).await;
            std::mem::drop(buttons_listener); // Close Zircon channel.

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].code, uapi::KEY_VOLUMEDOWN as u16);
            assert_eq!(events[0].value, 1);
        })
        .await;
    }

    #[test_case(1; "Scroll up")]
    #[test_case(-1; "Scroll down")]
    #[::fuchsia::test]
    async fn sends_mouse_wheel_events(ticks: i64) {
        spawn_kernel_and_run(async move |current_task| {
            let time = 100;
            let uapi_event = uapi::input_event {
                time: timeval_from_time(zx::MonotonicInstant::from_nanos(time)),
                type_: uapi::EV_REL as u16,
                code: uapi::REL_WHEEL as u16,
                value: ticks as i32,
            };

            let (_mouse_device, mouse_file, mut mouse_stream) =
                start_mouse_input(&current_task).await;

            answer_next_mouse_watch_request(
                &mut mouse_stream,
                vec![make_mouse_wheel_event_with_timestamp(ticks, time)],
            )
            .await;

            // Wait for another `Watch` to ensure mouse_file is done processing the other replies.
            // Use an empty vec, to ensure no unexpected `uapi::input_event`s are created.
            answer_next_mouse_watch_request(&mut mouse_stream, vec![]).await;

            let events = read_uapi_events(&mouse_file, &current_task);
            assert_eq!(events.len(), 2);
            assert_eq!(events[0], uapi_event);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn handles_mouse_move_and_click_events() {
        spawn_kernel_and_run(async move |current_task| {
            let mouse_move_event = fuipointer::MouseEvent {
                timestamp: Some(0),
                pointer_sample: Some(fuipointer::MousePointerSample {
                    device_id: Some(0),
                    relative_motion: Some([10.0, 20.0]),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let mouse_click_event = fuipointer::MouseEvent {
                timestamp: Some(0),
                pointer_sample: Some(fuipointer::MousePointerSample {
                    device_id: Some(0),
                    pressed_buttons: Some(vec![1]),
                    ..Default::default()
                }),
                ..Default::default()
            };

            let (_mouse_device, mouse_file, mut mouse_stream) =
                start_mouse_input(&current_task).await;

            // Expect mouse relay to handle MouseEvents with movement and buttons.
            answer_next_mouse_watch_request(&mut mouse_stream, vec![mouse_move_event]).await;
            answer_next_mouse_watch_request(&mut mouse_stream, vec![mouse_click_event]).await;

            // Wait for another `Watch` to ensure mouse_file is done processing the other replies.
            // Use an empty vec, to ensure no unexpected `uapi::input_event`s are created.
            answer_next_mouse_watch_request(&mut mouse_stream, vec![]).await;

            let events = read_uapi_events(&mouse_file, &current_task);
            assert!(!events.is_empty());
            assert!(events.iter().any(|e| e.type_ == uapi::EV_REL as u16
                && e.code == uapi::REL_X as u16
                && e.value == 10));
            assert!(events.iter().any(|e| e.type_ == uapi::EV_REL as u16
                && e.code == uapi::REL_Y as u16
                && e.value == 20));
            assert!(events.iter().any(|e| e.type_ == uapi::EV_KEY as u16
                && e.code == uapi::BTN_LEFT as u16
                && e.value == 1));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn touch_input_initialized_with_inspect_node() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();
            let touch_device = InputDevice::new_touch(
                1200, /* screen width */
                720,  /* screen height */
                &inspector.root(),
            );
            let _file_obj = touch_device.open_test(&current_task);

            assert_data_tree!(inspector, root: {
                touch_device: {
                    active_wake_leases_count: 0u64,
                    total_events_with_wake_lease_count: 0u64,
                    total_fidl_events_received_count: 0u64,
                    total_fidl_events_ignored_count: 0u64,
                    total_fidl_events_unexpected_count: 0u64,
                    total_fidl_events_converted_count: 0u64,
                    total_uapi_events_generated_count: 0u64,
                    last_generated_uapi_event_timestamp_ns: 0i64,
                    touch_file_0: {
                        fidl_events_received_count: 0u64,
                        fidl_events_ignored_count: 0u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 0u64,
                        uapi_events_generated_count: 0u64,
                        uapi_events_read_count: 0u64,
                        fd_read_count: 0u64,
                        fd_notify_count: 0u64,
                        last_generated_uapi_event_timestamp_ns: 0i64,
                        last_read_uapi_event_timestamp_ns: 0i64,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    }
                }
            });
        })
        .await;
    }

    #[::fuchsia::test]
    async fn touch_relay_updates_touch_inspect_status() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();

            let (_input_device, input_file, mut touch_source_stream) =
                start_touch_input_inspect(&current_task, &inspector).await;

            // Send 2 TouchEvents to proxy that should be counted as `received` by InputFile
            // A TouchEvent::default() has no pointer sample so these events should be discarded.
            let control_handle = touch_source_stream.control_handle();
            control_handle
                .send_on_touch_events(vec![make_empty_touch_event(), make_empty_touch_event()], 2)
                .expect("failure sending OnTouchEvents");
            match touch_source_stream.next().await {
                Some(Ok(fuipointer::TouchSourceV2Request::AcknowledgeEvents {
                    last_acknowledged_event_stamp,
                    ..
                })) => assert_eq!(last_acknowledged_event_stamp, 2),
                unexpected_request => panic!("unexpected request {:?}", unexpected_request),
            }

            // Send 5 TouchEvents with pointer sample to proxy, these should be received and converted
            // Add/Remove events generate 5 uapi events each. Change events generate 3 uapi events each.
            control_handle
                .send_on_touch_events(
                    vec![
                        make_touch_event_with_coords_phase_timestamp(
                            0.0,
                            0.0,
                            EventPhase::Add,
                            1,
                            1000,
                        ),
                        make_touch_event_with_coords_phase_timestamp(
                            1.0,
                            1.0,
                            EventPhase::Change,
                            1,
                            2000,
                        ),
                        make_touch_event_with_coords_phase_timestamp(
                            2.0,
                            2.0,
                            EventPhase::Change,
                            1,
                            3000,
                        ),
                        make_touch_event_with_coords_phase_timestamp(
                            3.0,
                            3.0,
                            EventPhase::Change,
                            1,
                            4000,
                        ),
                        make_touch_event_with_coords_phase_timestamp(
                            3.0,
                            3.0,
                            EventPhase::Remove,
                            1,
                            5000,
                        ),
                    ],
                    7,
                )
                .expect("failure sending OnTouchEvents");
            match touch_source_stream.next().await {
                Some(Ok(fuipointer::TouchSourceV2Request::AcknowledgeEvents {
                    last_acknowledged_event_stamp,
                    ..
                })) => assert_eq!(last_acknowledged_event_stamp, 7),
                unexpected_request => panic!("unexpected request {:?}", unexpected_request),
            }

            let _events = read_uapi_events(&input_file, &current_task);
            assert_data_tree!(inspector, root: {
                touch_device: {
                    active_wake_leases_count: 0u64,
                    total_events_with_wake_lease_count: 0u64,
                    total_fidl_events_received_count: 7u64,
                    total_fidl_events_ignored_count: 2u64,
                    total_fidl_events_unexpected_count: 0u64,
                    total_fidl_events_converted_count: 5u64,
                    total_uapi_events_generated_count: 22u64,
                    last_generated_uapi_event_timestamp_ns: 5000i64,
                    touch_file_0: {
                        fidl_events_received_count: 7u64,
                        fidl_events_ignored_count: 2u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 5u64,
                        uapi_events_generated_count: 22u64,
                        uapi_events_read_count: 22u64,
                        fd_read_count: 23u64,
                        fd_notify_count: 1u64,
                        last_generated_uapi_event_timestamp_ns: 5000i64,
                        last_read_uapi_event_timestamp_ns: 5000i64,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    },
                }
            });
        })
        .await;
    }

    #[::fuchsia::test]
    async fn new_file_updates_inspect_status() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();

            let input_device = InputDevice::new_touch(700, 700, inspector.root());
            let input_file_0 =
                input_device.open_test(&current_task).expect("Failed to create input file");

            let (touch_source_client_end, mut touch_source_stream) =
                fidl::endpoints::create_request_stream::<TouchSourceV2Marker>();
            let (mouse_source_client_end, _mouse_source_stream) =
                fidl::endpoints::create_request_stream::<fuipointer::MouseSourceV2Marker>();
            let (keyboard_proxy, mut keyboard_stream) =
                fidl::endpoints::create_sync_proxy_and_stream::<fuiinput::KeyboardMarker>();
            let view_ref_pair =
                fuchsia_scenic::ViewRefPair::new().expect("Failed to create ViewRefPair");
            let (device_registry_proxy, mut device_listener_stream) =
                fidl::endpoints::create_sync_proxy_and_stream::<
                    fuipolicy::DeviceListenerRegistryMarker,
                >();

            let (relay, relay_handle) = input_event_relay::new_input_relay();
            relay.start_relays(
                &current_task.kernel(),
                EventProxyMode::None,
                touch_source_client_end,
                keyboard_proxy,
                mouse_source_client_end,
                view_ref_pair.view_ref,
                device_registry_proxy,
                input_device.open_files.clone(),
                Default::default(),
                Default::default(),
                Some(input_device.inspect_status.clone()),
                None,
                None,
            );

            let _ = init_keyboard_listener(&mut keyboard_stream).await;
            let _ = init_button_listeners(&mut device_listener_stream).await;

            relay_handle.add_touch_device(
                0,
                input_device.open_files.clone(),
                Some(input_device.inspect_status.clone()),
            );

            // Send 2 TouchEvents to proxy that should be counted as `received` by InputFile
            // A TouchEvent::default() has no pointer sample so these events should be discarded.
            let control_handle = touch_source_stream.control_handle();
            control_handle
                .send_on_touch_events(vec![make_empty_touch_event(), make_empty_touch_event()], 2)
                .expect("failure sending OnTouchEvents");
            match touch_source_stream.next().await {
                Some(Ok(fuipointer::TouchSourceV2Request::AcknowledgeEvents {
                    last_acknowledged_event_stamp,
                    ..
                })) => assert_eq!(last_acknowledged_event_stamp, 2),
                unexpected_request => panic!("unexpected request {:?}", unexpected_request),
            }

            // Verify file node & properties remain in inspect tree when file is closed
            input_device.open_files.lock().clear();
            drop(input_file_0);

            // Open new file which should receive input_device's subsequent events
            let input_file_1 =
                input_device.open_test(&current_task).expect("Failed to create input file");

            // Send 5 TouchEvents with pointer sample to proxy, these should be received and converted
            // Add/Remove events generate 5 uapi events each. Change events generate 3 uapi events each.
            control_handle
                .send_on_touch_events(
                    vec![
                        make_touch_event_with_coords_phase_timestamp(
                            0.0,
                            0.0,
                            EventPhase::Add,
                            1,
                            1000,
                        ),
                        make_touch_event_with_coords_phase_timestamp(
                            1.0,
                            1.0,
                            EventPhase::Change,
                            1,
                            2000,
                        ),
                        make_touch_event_with_coords_phase_timestamp(
                            2.0,
                            2.0,
                            EventPhase::Change,
                            1,
                            3000,
                        ),
                        make_touch_event_with_coords_phase_timestamp(
                            3.0,
                            3.0,
                            EventPhase::Change,
                            1,
                            4000,
                        ),
                        make_touch_event_with_coords_phase_timestamp(
                            3.0,
                            3.0,
                            EventPhase::Remove,
                            1,
                            5000,
                        ),
                    ],
                    7,
                )
                .expect("failure sending OnTouchEvents");
            match touch_source_stream.next().await {
                Some(Ok(fuipointer::TouchSourceV2Request::AcknowledgeEvents {
                    last_acknowledged_event_stamp,
                    ..
                })) => assert_eq!(last_acknowledged_event_stamp, 7),
                unexpected_request => panic!("unexpected request {:?}", unexpected_request),
            }

            let _events = read_uapi_events(&input_file_1, &current_task);

            // Verify file node & properties remain in inspect tree when file is closed
            input_device.open_files.lock().clear();
            drop(input_file_1);

            assert_data_tree!(inspector, root: {
                touch_device: {
                    active_wake_leases_count: 0u64,
                    total_events_with_wake_lease_count: 0u64,
                    total_fidl_events_received_count: 7u64,
                    total_fidl_events_ignored_count: 2u64,
                    total_fidl_events_unexpected_count: 0u64,
                    total_fidl_events_converted_count: 5u64,
                    total_uapi_events_generated_count: 22u64,
                    last_generated_uapi_event_timestamp_ns: 5000i64,
                    touch_file_0: {
                        fidl_events_received_count: 2u64,
                        fidl_events_ignored_count: 2u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 0u64,
                        uapi_events_generated_count: 0u64,
                        uapi_events_read_count: 0u64,
                        fd_read_count: 0u64,
                        fd_notify_count: 0u64,
                        last_generated_uapi_event_timestamp_ns: 0i64,
                        last_read_uapi_event_timestamp_ns: 0i64,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    },
                    touch_file_1: {
                        fidl_events_received_count: 5u64,
                        fidl_events_ignored_count: 0u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 5u64,
                        uapi_events_generated_count: 22u64,
                        uapi_events_read_count: 22u64,
                        fd_read_count: 23u64,
                        fd_notify_count: 1u64,
                        last_generated_uapi_event_timestamp_ns: 5000i64,
                        last_read_uapi_event_timestamp_ns: 5000i64,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    },
                }
            });
        })
        .await;
    }

    #[::fuchsia::test]
    async fn file_status_inspect_not_empty_after_close() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();

            let input_device = InputDevice::new_touch(700, 700, inspector.root());
            let file_handle =
                input_device.open_test(&current_task).expect("Failed to create input file");

            let status = file_handle
                .downcast_file::<crate::input_file::ArcInputFile>()
                .unwrap()
                .0
                .inspect_status
                .clone()
                .expect("touch file must have status");
            status.count_received_events(5);

            assert_data_tree!(inspector, root: {
                touch_device: {
                    active_wake_leases_count: AnyProperty,
                    last_generated_uapi_event_timestamp_ns: AnyProperty,
                    total_events_with_wake_lease_count: AnyProperty,
                    total_fidl_events_converted_count: AnyProperty,
                    total_fidl_events_ignored_count: AnyProperty,
                    total_fidl_events_received_count: AnyProperty,
                    total_fidl_events_unexpected_count: AnyProperty,
                    total_uapi_events_generated_count: AnyProperty,
                    touch_file_0: {
                        fidl_events_received_count: 5u64,
                        fd_notify_count: 0u64,
                        fd_read_count: 0u64,
                        fidl_events_converted_count: 0u64,
                        fidl_events_ignored_count: 0u64,
                        fidl_events_unexpected_count: 0u64,
                        last_generated_uapi_event_timestamp_ns: 0i64,
                        last_read_uapi_event_timestamp_ns: 0i64,
                        uapi_events_generated_count: 0u64,
                        uapi_events_read_count: 0u64,
                        opened_without_nonblock: true,
                        open_timestamp_ns: AnyProperty,
                        closed: false,
                        close_timestamp_ns: 0i64,
                    }
                }
            });

            drop(status);
            input_device.open_files.lock().clear();
            drop(file_handle);

            assert_data_tree!(inspector, root: {
                touch_device: {
                    active_wake_leases_count: AnyProperty,
                    last_generated_uapi_event_timestamp_ns: AnyProperty,
                    total_events_with_wake_lease_count: AnyProperty,
                    total_fidl_events_converted_count: AnyProperty,
                    total_fidl_events_ignored_count: AnyProperty,
                    total_fidl_events_received_count: AnyProperty,
                    total_fidl_events_unexpected_count: AnyProperty,
                    total_uapi_events_generated_count: AnyProperty,
                    touch_file_0: {
                        fidl_events_received_count: 5u64,
                        fd_notify_count: 0u64,
                        fd_read_count: 0u64,
                        fidl_events_converted_count: 0u64,
                        fidl_events_ignored_count: 0u64,
                        fidl_events_unexpected_count: 0u64,
                        last_generated_uapi_event_timestamp_ns: 0i64,
                        last_read_uapi_event_timestamp_ns: 0i64,
                        uapi_events_generated_count: 0u64,
                        uapi_events_read_count: 0u64,
                        opened_without_nonblock: true,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    }
                }
            });
        })
        .await;
    }

    #[::fuchsia::test]
    async fn keyboard_input_initialized_with_inspect_node() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();
            let keyboard_device = InputDevice::new_keyboard(&inspector.root());
            let _file_obj = keyboard_device.open_test(&current_task);

            assert_data_tree!(inspector, root: {
                keyboard_device: {
                    active_wake_leases_count: 0u64,
                    total_events_with_wake_lease_count: 0u64,
                    total_fidl_events_received_count: 0u64,
                    total_fidl_events_ignored_count: 0u64,
                    total_fidl_events_unexpected_count: 0u64,
                    total_fidl_events_converted_count: 0u64,
                    total_uapi_events_generated_count: 0u64,
                    last_generated_uapi_event_timestamp_ns: 0i64,
                    keyboard_file_0: {
                        fidl_events_received_count: 0u64,
                        fidl_events_ignored_count: 0u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 0u64,
                        uapi_events_generated_count: 0u64,
                        uapi_events_read_count: 0u64,
                        fd_read_count: 0u64,
                        fd_notify_count: 0u64,
                        last_generated_uapi_event_timestamp_ns: 0i64,
                        last_read_uapi_event_timestamp_ns: 0i64,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    }
                }
            });
        })
        .await;
    }

    #[::fuchsia::test]
    async fn button_relay_updates_keyboard_inspect_status() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();

            let (_input_device, input_file, buttons_listener) =
                start_button_input_inspect(&current_task, &inspector).await;

            // Each of these events should count toward received and converted.
            // They also generate 2 uapi events each.
            let power_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(true),
                function: Some(false),
                ..Default::default()
            };

            let power_release_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(false),
                function: Some(false),
                ..Default::default()
            };

            let _ = buttons_listener.on_event(power_event).await;
            let _ = buttons_listener.on_event(power_release_event).await;

            let events = read_uapi_events(&input_file, &current_task);
            assert_eq!(events.len(), 4);
            assert_eq!(events[0].code, uapi::KEY_POWER as u16);
            assert_eq!(events[0].value, 1);
            assert_eq!(events[2].code, uapi::KEY_POWER as u16);
            assert_eq!(events[2].value, 0);

            let _events = read_uapi_events(&input_file, &current_task);

            assert_data_tree!(inspector, root: {
                keyboard_device: {
                    active_wake_leases_count: 0u64,
                    total_events_with_wake_lease_count: 0u64,
                    total_fidl_events_received_count: 2u64,
                    total_fidl_events_ignored_count: 0u64,
                    total_fidl_events_unexpected_count: 0u64,
                    total_fidl_events_converted_count: 2u64,
                    total_uapi_events_generated_count: 4u64,
                    // Button events perform a realtime clockread, so any value will do.
                    last_generated_uapi_event_timestamp_ns: AnyProperty,
                    keyboard_file_0: {
                        fidl_events_received_count: 2u64,
                        fidl_events_ignored_count: 0u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 2u64,
                        uapi_events_generated_count: 4u64,
                        uapi_events_read_count: 4u64,
                        fd_read_count: 6u64,
                        fd_notify_count: 2u64,
                        // Button events perform a realtime clockread, so any value will do.
                        last_generated_uapi_event_timestamp_ns: AnyProperty,
                        last_read_uapi_event_timestamp_ns: AnyProperty,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    },
                }
            });
        })
        .await;
    }

    #[::fuchsia::test]
    async fn mouse_input_initialized_with_inspect_node() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();
            let mouse_device = InputDevice::new_mouse(&inspector.root());
            let _file_obj = mouse_device.open_test(&current_task);

            assert_data_tree!(inspector, root: {
                mouse_device: {
                    active_wake_leases_count: 0u64,
                    total_events_with_wake_lease_count: 0u64,
                    total_fidl_events_received_count: 0u64,
                    total_fidl_events_ignored_count: 0u64,
                    total_fidl_events_unexpected_count: 0u64,
                    total_fidl_events_converted_count: 0u64,
                    total_uapi_events_generated_count: 0u64,
                    last_generated_uapi_event_timestamp_ns: 0i64,
                    mouse_file_0: {
                        fidl_events_received_count: 0u64,
                        fidl_events_ignored_count: 0u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 0u64,
                        uapi_events_generated_count: 0u64,
                        uapi_events_read_count: 0u64,
                        fd_read_count: 0u64,
                        fd_notify_count: 0u64,
                        last_generated_uapi_event_timestamp_ns: 0i64,
                        last_read_uapi_event_timestamp_ns: 0i64,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    }
                }
            });
        })
        .await;
    }

    #[::fuchsia::test]
    async fn mouse_relay_updates_mouse_inspect_status() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();

            let (_input_device, input_file, mut mouse_source_stream) =
                start_mouse_input_inspect(&current_task, &inspector).await;

            let mouse_move_event = fuipointer::MouseEvent {
                timestamp: Some(0),
                pointer_sample: Some(fuipointer::MousePointerSample {
                    device_id: Some(0),
                    position_in_viewport: Some([50.0, 50.0]),
                    scroll_v: Some(0),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let mouse_click_event = fuipointer::MouseEvent {
                timestamp: Some(0),
                pointer_sample: Some(fuipointer::MousePointerSample {
                    device_id: Some(0),
                    scroll_v: Some(0),
                    pressed_buttons: Some(vec![1]),
                    ..Default::default()
                }),
                ..Default::default()
            };

            // Send 2 MouseEvents: one move event with no relative motion (ignored) and one click
            // event with button 1 pressed (converted).
            answer_next_mouse_watch_request(
                &mut mouse_source_stream,
                vec![mouse_move_event, mouse_click_event],
            )
            .await;

            // Send 5 MouseEvents with non-zero scroll_v delta to proxy, these should be received and
            // converted to 1 uapi event each, with an extra sync event to signify end of the batch.
            answer_next_mouse_watch_request(
                &mut mouse_source_stream,
                (0..5).map(|_| make_mouse_wheel_event(1)).collect(),
            )
            .await;

            // Send a final mouse wheel event and ensure the inspect tree reflects it's timestamp under
            // last_generated_uapi_event_timestamp_ns and last_read_uapi_event_timestamp_ns.
            answer_next_mouse_watch_request(
                &mut mouse_source_stream,
                vec![make_mouse_wheel_event_with_timestamp(-1, 5000)],
            )
            .await;

            // Wait for another `Watch` to ensure mouse_file is done processing the other replies.
            // Use an empty vec, to ensure no unexpected `uapi::input_event`s are created.
            answer_next_mouse_watch_request(&mut mouse_source_stream, vec![]).await;

            let _events = read_uapi_events(&input_file, &current_task);
            assert_data_tree!(inspector, root: {
                mouse_device: {
                    active_wake_leases_count: 0u64,
                    total_events_with_wake_lease_count: 0u64,
                    total_fidl_events_received_count: 8u64,
                    total_fidl_events_ignored_count: 1u64,
                    total_fidl_events_unexpected_count: 0u64,
                    total_fidl_events_converted_count: 7u64,
                    total_uapi_events_generated_count: 7u64,
                    last_generated_uapi_event_timestamp_ns: 5000i64,
                    mouse_file_0: {
                        fidl_events_received_count: 8u64,
                        fidl_events_ignored_count: 1u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 7u64,
                        uapi_events_generated_count: 7u64,
                        uapi_events_read_count: 7u64,
                        fd_read_count: 8u64,
                        fd_notify_count: 3u64,
                        last_generated_uapi_event_timestamp_ns: 5000i64,
                        last_read_uapi_event_timestamp_ns: 5000i64,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    },
                }
            });
        })
        .await;
    }

    #[::fuchsia::test]
    async fn eviocgname_zero_buffer_length_succeeds() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();
            let touch_device = InputDevice::new_touch(700, 1200, &inspector.root());
            let file =
                touch_device.open_test(&current_task).expect("Failed to open touch input file");

            let user_addr = starnix_core::testing::map_memory(
                &current_task,
                starnix_uapi::user_address::UserAddress::default(),
                4096,
            );

            // EVIOCGNAME with 0 buffer length should return SUCCESS and not panic or write memory.
            let res = file.ioctl(&current_task, uapi::EVIOCGNAME_0, user_addr.into());
            assert_eq!(res, Ok(starnix_syscalls::SUCCESS));

            // EVIOCGNAME with non-zero buffer length should succeed and return bytes written.
            let name_req = uapi::EVIOCGNAME_0 | (256 << 16);
            let res = file.ioctl(&current_task, name_req, user_addr.into());
            assert!(res.is_ok());
            let bytes_written = res.unwrap().value() as usize;
            assert!(bytes_written > 0);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn evdev_variable_length_ioctls_succeed() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();
            let touch_device = InputDevice::new_touch(700, 1200, &inspector.root());
            let file =
                touch_device.open_test(&current_task).expect("Failed to open touch input file");

            let user_addr = starnix_core::testing::map_memory(
                &current_task,
                starnix_uapi::user_address::UserAddress::default(),
                4096,
            );

            // Test EVIOCGRAB succeeds.
            let res = file.ioctl(&current_task, uapi::EVIOCGRAB, 1u64.into());
            assert_eq!(res, Ok(starnix_syscalls::SUCCESS));

            // Test EVIOCGPHYS with variable length.
            let phys_req = crate::input_file::EVIOCGPHYS_BASE | (256 << 16);
            let res = file.ioctl(&current_task, phys_req, user_addr.into());
            assert!(res.is_ok());
            let bytes_written = res.unwrap().value() as usize;
            assert!(bytes_written > 0);

            // Test EVIOCGUNIQ returns ENOENT (synthetic devices have no serial/unique identifier).
            let uniq_req = crate::input_file::EVIOCGUNIQ_BASE | (256 << 16);
            let res = file.ioctl(&current_task, uniq_req, user_addr.into());
            assert_eq!(res, starnix_uapi::error!(ENOENT));

            // Test EVIOCGPROP with variable length.
            let prop_req = crate::input_file::EVIOCGPROP_BASE | (64 << 16);
            let res = file.ioctl(&current_task, prop_req, user_addr.into());
            assert_eq!(res, Ok(starnix_syscalls::SUCCESS));

            // Test EVIOCGBIT(0) with variable length.
            let bit0_req = crate::input_file::EVIOCGBIT_0_BASE | (64 << 16);
            let res = file.ioctl(&current_task, bit0_req, user_addr.into());
            assert_eq!(res, Ok(starnix_syscalls::SUCCESS));

            // Test EVIOCGBIT(EV_SND) succeeds and returns zeros.
            let snd_req = crate::input_file::EVIOCGBIT_EV_SND_BASE | (64 << 16);
            let res = file.ioctl(&current_task, snd_req, user_addr.into());
            assert_eq!(res, Ok(starnix_syscalls::SUCCESS));

            // Test invalid EVIOCGBIT(0x1f) returns EINVAL.
            let invalid_bit_req = (uapi::_IOC_READ << uapi::_IOC_DIRSHIFT)
                | ((b'E' as u32) << uapi::_IOC_TYPESHIFT)
                | (64 << 16)
                | (0x20 + 0x1f);
            let res = file.ioctl(&current_task, invalid_bit_req, user_addr.into());
            assert_eq!(res, starnix_uapi::error!(EINVAL));

            // Test unrecognized evdev ioctl code with valid type/dir returns EINVAL via fallback.
            let unrecognized_evdev_req = (uapi::_IOC_READ << uapi::_IOC_DIRSHIFT)
                | ((b'E' as u32) << uapi::_IOC_TYPESHIFT)
                | 0xFF;
            let res = file.ioctl(&current_task, unrecognized_evdev_req, user_addr.into());
            assert_eq!(res, starnix_uapi::error!(EINVAL));

            // Test invalid direction/type returns EINVAL.
            let invalid_dir_req = (uapi::_IOC_WRITE << uapi::_IOC_DIRSHIFT)
                | ((b'E' as u32) << uapi::_IOC_TYPESHIFT)
                | 0x20;
            let res = file.ioctl(&current_task, invalid_dir_req, user_addr.into());
            assert_eq!(res, starnix_uapi::error!(EINVAL));
        })
        .await;
    }

    // The evdev current-state queries report what the device is doing right now,
    // as opposed to the EVIOCGBIT queries which report what it is capable of.
    // Starnix tracks none of this state and reports all-zeros, but these must not
    // fail: `xf86-input-evdev` issues all four during PreInit and treats an error
    // as fatal, so returning EINVAL here means X11 brings up no input devices.
    #[::fuchsia::test]
    async fn evdev_current_state_queries_succeed() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();
            let touch_device = InputDevice::new_touch(700, 1200, &inspector.root());
            let file =
                touch_device.open_test(&current_task).expect("Failed to open touch input file");

            let user_addr = starnix_core::testing::map_memory(
                &current_task,
                starnix_uapi::user_address::UserAddress::default(),
                4096,
            );

            const BUF_LEN: usize = 96;
            for (name, base) in [
                ("EVIOCGKEY", crate::input_file::EVIOCGKEY_BASE),
                ("EVIOCGLED", crate::input_file::EVIOCGLED_BASE),
                ("EVIOCGSND", crate::input_file::EVIOCGSND_BASE),
                ("EVIOCGSW", crate::input_file::EVIOCGSW_BASE),
            ] {
                // Poison the buffer first, so that observing zeros afterwards
                // proves the ioctl wrote them rather than finding them.
                current_task
                    .write_memory(user_addr, &[0xAAu8; BUF_LEN])
                    .expect("failed to poison buffer");

                let req = base | ((BUF_LEN as u32) << 16);
                let res = file.ioctl(&current_task, req, user_addr.into());
                assert_eq!(res, Ok(starnix_syscalls::SUCCESS), "{name} should succeed");

                let read_back = current_task
                    .read_memory_to_vec(user_addr, BUF_LEN)
                    .expect("failed to read back buffer");
                assert_eq!(
                    read_back,
                    vec![0u8; BUF_LEN],
                    "{name} should report nothing currently active"
                );
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn keyboard_input_file_ioctls() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();
            let keyboard_device = InputDevice::new_keyboard(&inspector.root());
            let file = keyboard_device
                .open_test(&current_task)
                .expect("Failed to open keyboard input file");

            let user_addr = map_memory(&current_task, UserAddress::default(), 4096);

            // Test EVIOCGNAME
            let name_req = crate::input_file::EVIOCGNAME_BASE | (256 << 16);
            let res = file.ioctl(&current_task, name_req, user_addr.into());
            assert!(res.is_ok());

            // Test EVIOCGPROP - should have INPUT_PROP_DIRECT, not INPUT_PROP_POINTER
            let prop_req = crate::input_file::EVIOCGPROP_BASE | (64 << 16);
            let res = file.ioctl(&current_task, prop_req, user_addr.into());
            assert_eq!(res, Ok(SUCCESS));
            let prop_buf = current_task.read_memory_to_vec(user_addr, 8).unwrap();
            let byte_idx = (uapi::INPUT_PROP_DIRECT / 8) as usize;
            let bit_idx = uapi::INPUT_PROP_DIRECT % 8;
            assert_ne!(prop_buf[byte_idx] & (1 << bit_idx), 0);

            // Test EVIOCGBIT_EV_KEY - should have KEY_A, KEY_1, KEY_ENTER, KEY_POWER, etc.
            let key_req = crate::input_file::EVIOCGBIT_EV_KEY_BASE | (64 << 16);
            let res = file.ioctl(&current_task, key_req, user_addr.into());
            assert_eq!(res, Ok(SUCCESS));
            let key_buf = current_task.read_memory_to_vec(user_addr, 64).unwrap();

            let check_key = |key_code: u32| {
                let byte = (key_code / 8) as usize;
                let bit = key_code % 8;
                assert_ne!(
                    key_buf[byte] & (1 << bit),
                    0,
                    "Keycode {} should be set in evdev bitmask",
                    key_code
                );
            };
            check_key(uapi::KEY_A);
            check_key(uapi::KEY_Z);
            check_key(uapi::KEY_1);
            check_key(uapi::KEY_ENTER);
            check_key(uapi::KEY_SPACE);
            check_key(uapi::KEY_ESC);
            check_key(uapi::KEY_POWER);
            check_key(uapi::KEY_VOLUMEUP);
            check_key(uapi::KEY_VOLUMEDOWN);

            // Buttons that are not keyboard keys must NOT be advertised. `KEY_MAP` carries a
            // block of test-only keycodes (b/311425670) covering BTN_JOYSTICK..BTN_THUMBR; if
            // those leak into the capability bitmask, evdev clients classify this device as a
            // gamepad as well as a keyboard.
            let check_key_absent = |key_code: u32, name: &str| {
                let byte = (key_code / 8) as usize;
                let bit = key_code % 8;
                assert_eq!(
                    key_buf[byte] & (1 << bit),
                    0,
                    "{} ({}) should NOT be set in the keyboard evdev bitmask",
                    name,
                    key_code
                );
            };
            check_key_absent(uapi::BTN_LEFT, "BTN_LEFT");
            check_key_absent(uapi::BTN_TRIGGER, "BTN_TRIGGER");
            check_key_absent(uapi::BTN_SOUTH, "BTN_SOUTH");
            check_key_absent(uapi::BTN_THUMBR, "BTN_THUMBR");
        })
        .await;
    }

    #[::fuchsia::test]
    async fn keyboard_events_update_inspect_counters() {
        spawn_kernel_and_run(async move |current_task| {
            let inspector = fuchsia_inspect::Inspector::default();
            let (_keyboard_device, keyboard_file, keyboard_listener) =
                start_keyboard_input_inspect(&current_task, &inspector).await;

            let key_event = fuiinput::KeyEvent {
                timestamp: Some(12345),
                type_: Some(fuiinput::KeyEventType::Pressed),
                key: Some(fidl_fuchsia_input::Key::A),
                ..Default::default()
            };

            let _ = keyboard_listener.on_key_event(&key_event).await;
            std::mem::drop(keyboard_listener);

            // Read events to verify delivery
            let events = read_uapi_events(&keyboard_file, &current_task);
            assert_eq!(events.len(), 2);

            // `converted` counts FIDL events, so one key event is 1 here even though it
            // produces 2 uapi events (the key plus a SYN). Keep this distinct from
            // `total_uapi_events_generated_count`.
            assert_data_tree!(inspector, root: {
                keyboard_device: {
                    active_wake_leases_count: AnyProperty,
                    total_events_with_wake_lease_count: AnyProperty,
                    total_fidl_events_received_count: 1u64,
                    total_fidl_events_ignored_count: 0u64,
                    total_fidl_events_unexpected_count: 0u64,
                    total_fidl_events_converted_count: 1u64,
                    total_uapi_events_generated_count: 2u64,
                    last_generated_uapi_event_timestamp_ns: 12345i64,
                    keyboard_file_0: {
                        fidl_events_received_count: 1u64,
                        fidl_events_ignored_count: 0u64,
                        fidl_events_unexpected_count: 0u64,
                        fidl_events_converted_count: 1u64,
                        uapi_events_generated_count: 2u64,
                        uapi_events_read_count: 2u64,
                        fd_read_count: AnyProperty,
                        fd_notify_count: AnyProperty,
                        last_generated_uapi_event_timestamp_ns: 12345i64,
                        last_read_uapi_event_timestamp_ns: AnyProperty,
                        opened_without_nonblock: AnyProperty,
                        open_timestamp_ns: AnyProperty,
                        closed: AnyProperty,
                        close_timestamp_ns: AnyProperty,
                    }
                }
            });
        })
        .await;
    }
}
