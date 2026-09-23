// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::uinput::vfs::{CloseFreeSafe, NamespaceNode};
use crate::{InputEventsRelayHandle, InputFile, OpenedFiles};
use bit_vec::BitVec;
use fidl_fuchsia_ui_test_input::{
    self as futinput, CoordinateUnit, DisplayDimensions, KeyboardSimulateKeyEventRequest,
    RegistryRegisterKeyboardAndGetDeviceInfoRequest,
    RegistryRegisterTouchScreenAndGetDeviceInfoRequest,
};
use fuchsia_inspect;
use starnix_core::device::kobject::{Device, DeviceMetadata};
use starnix_core::device::{DeviceMode, DeviceOps};
use starnix_core::fileops_impl_seekless;
use starnix_core::mm::MemoryAccessorExt;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::{self, FileObject, FileOps, FsString, fileops_impl_noop_sync};
use starnix_logging::log_warn;
use starnix_modules_input_event_conversion::key_linux_to_fuchsia::LinuxKeyboardEventParser;
use starnix_modules_input_event_conversion::touch_linux_to_fuchsia::LinuxTouchEventParser;
use starnix_sync::{LockDepMutex, UinputDeviceStateLock};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::device_id::INPUT_MAJOR;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{MultiArchUserRef, UserRef};
use starnix_uapi::{device_id, errno, error, uapi};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

// Return the current uinput API version 5, it also told caller this uinput
// supports UI_DEV_SETUP.
const UINPUT_VERSION: u32 = 5;

type InputEventPtr = MultiArchUserRef<uapi::input_event, uapi::arch32::input_event>;

#[derive(Clone)]
enum DeviceId {
    Keyboard,
    Touchscreen(i32, i32),
}

pub fn register_uinput_device(
    kernel: &Kernel,
    input_event_relay_handle: Arc<InputEventsRelayHandle>,
) -> Result<(), Errno> {
    let registry = &kernel.device_registry;
    let misc_class = registry.objects.misc_class();
    let inspect_node = Arc::new(kernel.inspect_node.create_child("uinput"));
    let device = UinputDevice::new(input_event_relay_handle, inspect_node);
    registry.register_device(
        kernel,
        "uinput".into(),
        DeviceMetadata::new("uinput".into(), device_id::DeviceId::UINPUT, DeviceMode::Char),
        misc_class,
        device,
    )?;
    Ok(())
}

fn add_and_register_input_device(
    system_task: &CurrentTask,
    dev_ops: impl DeviceOps,
    device_id: u32,
) -> Result<Device, Errno> {
    let kernel = system_task.kernel();
    let registry = &kernel.device_registry;

    let input_class = registry.objects.input_class();

    registry.register_device(
        system_task.kernel(),
        FsString::from(format!("event{}", device_id)).as_ref(),
        DeviceMetadata::new(
            format!("input/event{}", device_id).into(),
            starnix_uapi::device_id::DeviceId::new(INPUT_MAJOR, device_id),
            DeviceMode::Char,
        ),
        input_class,
        dev_ops,
    )
}

#[derive(Clone)]
struct UinputDevice {
    input_event_relay: Arc<InputEventsRelayHandle>,
    inspect_node: Arc<fuchsia_inspect::Node>,
}

impl UinputDevice {
    pub fn new(
        input_event_relay: Arc<InputEventsRelayHandle>,
        inspect_node: Arc<fuchsia_inspect::Node>,
    ) -> Self {
        Self { input_event_relay, inspect_node }
    }
}

impl DeviceOps for UinputDevice {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _id: device_id::DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(UinputDeviceFile::new(
            self.input_event_relay.clone(),
            self.inspect_node.clone(),
        )))
    }
}

enum CreatedDevice {
    None,
    Keyboard(futinput::KeyboardSynchronousProxy, LinuxKeyboardEventParser),
    // LinuxTouchEventParser need to boxed to avoid warning: large-enum-variant.
    Touchscreen(futinput::TouchScreenSynchronousProxy, Box<LinuxTouchEventParser>),
}

struct Range {
    min: i32,
    max: i32,
}
struct UinputDeviceMutableState {
    enabled_evbits: BitVec,
    input_id: Option<uapi::input_id>,
    created_device: CreatedDevice,
    k_device: Option<Device>,
    device_id: Option<u32>,
    x_range: Option<Range>,
    y_range: Option<Range>,
}

impl UinputDeviceMutableState {
    fn get_id_and_device_type(&self) -> Option<(uapi::input_id, DeviceId)> {
        let input_id = match self.input_id {
            Some(input_id) => input_id,
            None => return None,
        };
        // Currently only support Keyboard and Touchscreen, if evbits contains
        // EV_ABS, consider it is Touchscreen. This need to be revisit when we
        // want to support more device types.
        let device_type = match self.enabled_evbits.clone().get(uapi::EV_ABS as usize) {
            Some(true) => {
                // TODO(b/304595635): Check if screen size is required.
                let mut touchscreen_width = 1000;
                let mut touchscreen_height = 1000;

                if self.x_range.is_some() && self.y_range.is_some() {
                    let x_range = self.x_range.as_ref().unwrap();
                    let y_range = self.y_range.as_ref().unwrap();
                    touchscreen_width = x_range.max - x_range.min;
                    touchscreen_height = y_range.max - y_range.min;
                }
                DeviceId::Touchscreen(touchscreen_width, touchscreen_height)
            }
            Some(false) | None => DeviceId::Keyboard,
        };

        Some((input_id, device_type))
    }
}

struct UinputDeviceFile {
    input_event_relay: Arc<InputEventsRelayHandle>,
    inner: LockDepMutex<UinputDeviceMutableState, UinputDeviceStateLock>,
    inspect_node: Arc<fuchsia_inspect::Node>,
}

impl UinputDeviceFile {
    pub fn new(
        input_event_relay: Arc<InputEventsRelayHandle>,
        inspect_node: Arc<fuchsia_inspect::Node>,
    ) -> Self {
        Self {
            input_event_relay,
            inner: UinputDeviceMutableState {
                enabled_evbits: BitVec::from_elem(uapi::EV_CNT as usize, false),
                input_id: None,
                created_device: CreatedDevice::None,
                k_device: None,
                device_id: None,
                x_range: None,
                y_range: None,
            }
            .into(),
            inspect_node,
        }
    }

    /// UI_SET_EVBIT caller pass a u32 as the event type "EV_*" to set this
    /// uinput device may handle events with the given event type.
    fn ui_set_evbit(&self, arg: SyscallArg) -> Result<SyscallResult, Errno> {
        let evbit: u32 = arg.into();
        match evbit {
            uapi::EV_KEY | uapi::EV_ABS => {
                self.inner.lock().enabled_evbits.set(evbit as usize, true);
                Ok(SUCCESS)
            }
            _ => {
                log_warn!("UI_SET_EVBIT with unsupported evbit {}", evbit);
                error!(EPERM)
            }
        }
    }

    /// UI_ABS_SETUP caller pass in event codes and min and max value for
    /// the event code.
    fn ui_abs_setup(
        &self,
        current_task: &CurrentTask,
        abs_setup: UserRef<starnix_uapi::uinput_abs_setup>,
    ) -> Result<SyscallResult, Errno> {
        let setup: starnix_uapi::uinput_abs_setup = current_task.read_object(abs_setup)?;
        let code: u32 = setup.code.into();
        match code {
            uapi::ABS_MT_POSITION_X => {
                self.inner.lock().x_range =
                    Some(Range { min: setup.absinfo.minimum, max: setup.absinfo.maximum });
            }
            uapi::ABS_MT_POSITION_Y => {
                self.inner.lock().y_range =
                    Some(Range { min: setup.absinfo.minimum, max: setup.absinfo.maximum });
            }
            _ => {
                log_warn!("UI_ABS_SETUP ignore event code {}", setup.code);
            }
        }
        Ok(SUCCESS)
    }

    /// UI_GET_VERSION caller pass a address for u32 to `arg` to receive the
    /// uinput version. ioctl returns SUCCESS(0) for success calls, and
    /// EFAULT(14) for given address is null.
    fn ui_get_version(
        &self,
        current_task: &CurrentTask,
        user_version: UserRef<u32>,
    ) -> Result<SyscallResult, Errno> {
        let response: u32 = UINPUT_VERSION;
        current_task.write_object(user_version, &response)?;
        Ok(SUCCESS)
    }

    /// UI_DEV_SETUP set the name of device and input_id (bustype, vendor id,
    /// product id, version) to the uinput device.
    fn ui_dev_setup(
        &self,
        current_task: &CurrentTask,
        user_uinput_setup: UserRef<uapi::uinput_setup>,
    ) -> Result<SyscallResult, Errno> {
        let uinput_setup = current_task.read_object(user_uinput_setup)?;
        self.inner.lock().input_id = Some(uinput_setup.id);
        Ok(SUCCESS)
    }

    fn ui_dev_create(&self, current_task: &CurrentTask) -> Result<SyscallResult, Errno> {
        // Only eng and userdebug builds include the `fuchsia.ui.test.input` service.
        let registry = match fuchsia_component::client::connect_to_protocol_sync::<
            futinput::RegistryMarker,
        >() {
            Ok(proxy) => Some(proxy),
            Err(_) => {
                log_warn!("Could not connect to fuchsia.ui.test.input/Registry");
                None
            }
        };
        self.ui_dev_create_inner(current_task, registry)
    }

    /// UI_DEV_CREATE calls create the uinput device with given information
    /// from previous ioctl() calls.
    fn ui_dev_create_inner(
        &self,
        current_task: &CurrentTask,
        // Takes `registry` arg so we can manually inject a mock registry in unit tests.
        registry: Option<futinput::RegistrySynchronousProxy>,
    ) -> Result<SyscallResult, Errno> {
        match registry {
            Some(proxy) => {
                let mut inner = self.inner.lock();
                let (input_id, device_type) = match inner.get_id_and_device_type() {
                    Some((id, dev)) => (id, dev),
                    None => return error!(EINVAL),
                };

                let open_files: OpenedFiles = Default::default();

                let (registered_device_id, inspect_status) = match device_type {
                    DeviceId::Keyboard => {
                        let (key_client, key_server) =
                            fidl::endpoints::create_sync_proxy::<futinput::KeyboardMarker>();
                        inner.created_device =
                            CreatedDevice::Keyboard(key_client, LinuxKeyboardEventParser::create());

                        // Register a keyboard
                        let register_res = proxy.register_keyboard_and_get_device_info(
                            RegistryRegisterKeyboardAndGetDeviceInfoRequest {
                                device: Some(key_server),
                                ..Default::default()
                            },
                            zx::MonotonicInstant::INFINITE,
                        );

                        match register_res {
                            Ok(resp) => match resp.device_id {
                                Some(device_id) => {
                                    inner.device_id = Some(device_id);
                                    let node = self
                                        .inspect_node
                                        .create_child(format!("uinput_device_{}", device_id));
                                    let inspect_status = crate::InputDeviceStatus::new(node);
                                    self.input_event_relay.add_keyboard_device(
                                        device_id,
                                        open_files.clone(),
                                        Some(inspect_status.clone()),
                                    );
                                    (device_id, inspect_status)
                                }
                                None => {
                                    log_warn!(
                                        "register_keyboard_and_get_device_info response does not include a device_id"
                                    );
                                    return error!(EPERM);
                                }
                            },
                            Err(e) => {
                                log_warn!(
                                    "Uinput could not register Keyboard device to Registry: {:?}",
                                    e
                                );
                                return error!(EPERM);
                            }
                        }
                    }
                    DeviceId::Touchscreen(_width, _height) => {
                        let (touch_client, touch_server) =
                            fidl::endpoints::create_sync_proxy::<futinput::TouchScreenMarker>();
                        inner.created_device = CreatedDevice::Touchscreen(
                            touch_client,
                            Box::new(LinuxTouchEventParser::create()),
                        );

                        let mut request = RegistryRegisterTouchScreenAndGetDeviceInfoRequest {
                            device: Some(touch_server),
                            coordinate_unit: Some(CoordinateUnit::PhysicalPixels),
                            ..Default::default()
                        };

                        if inner.x_range.is_some() && inner.y_range.is_some() {
                            request.coordinate_unit = Some(CoordinateUnit::RegisteredDimensions);
                            let x_range = inner.x_range.as_ref().unwrap();
                            let y_range = inner.y_range.as_ref().unwrap();
                            request.display_dimensions = Some(DisplayDimensions {
                                min_x: x_range.min.into(),
                                max_x: x_range.max.into(),
                                min_y: y_range.min.into(),
                                max_y: y_range.max.into(),
                            });
                        }

                        // Register a touchscreen
                        let register_res = proxy.register_touch_screen_and_get_device_info(
                            request,
                            zx::Instant::INFINITE,
                        );

                        match register_res {
                            Ok(resp) => match resp.device_id {
                                Some(device_id) => {
                                    inner.device_id = Some(device_id);
                                    let node = self
                                        .inspect_node
                                        .create_child(format!("uinput_device_{}", device_id));
                                    let inspect_status = crate::InputDeviceStatus::new(node);
                                    self.input_event_relay.add_touch_device(
                                        device_id,
                                        open_files.clone(),
                                        Some(inspect_status.clone()),
                                    );
                                    (device_id, inspect_status)
                                }
                                None => {
                                    log_warn!(
                                        "register_touch_screen_and_get_device_info response does not include a device_id"
                                    );
                                    return error!(EPERM);
                                }
                            },
                            Err(e) => {
                                log_warn!(
                                    "Uinput could not register Keyboard device to Registry: {:?}",
                                    e
                                );
                                return error!(EPERM);
                            }
                        }
                    }
                };

                let device = add_and_register_input_device(
                    current_task,
                    VirtualDevice { input_id, devt: device_type, open_files, inspect_status },
                    registered_device_id,
                )?;
                inner.k_device = Some(device);

                new_device();

                Ok(SUCCESS)
            }
            None => {
                log_warn!("No Registry available for Uinput.");
                error!(EPERM)
            }
        }
    }

    fn ui_dev_destroy(&self, current_task: &CurrentTask) -> Result<SyscallResult, Errno> {
        let mut inner = self.inner.lock();
        match inner.device_id {
            Some(device_id) => {
                self.input_event_relay.remove_device(device_id);
            }
            None => {
                // This is possible if caller does not call create device but calls destroy.
                // No cleanup is needed for input event relay in this case.
            }
        }

        match inner.k_device.clone() {
            Some(device) => {
                let kernel = current_task.kernel();
                kernel.device_registry.remove_device(current_task, device);
            }
            None => {
                log_warn!("UI_DEV_DESTROY kHandle not found");
                return error!(EPERM);
            }
        }
        inner.k_device = None;
        inner.created_device = CreatedDevice::None;

        destroy_device();

        Ok(SUCCESS)
    }
}

// TODO(b/312467059): Remove once ESC -> Power workaround can be remove.
static COUNT_OF_UINPUT_DEVICE: AtomicI32 = AtomicI32::new(0);

fn new_device() {
    let _ = COUNT_OF_UINPUT_DEVICE.fetch_add(1, Ordering::SeqCst);
}

fn destroy_device() {
    let _ = COUNT_OF_UINPUT_DEVICE.fetch_sub(1, Ordering::SeqCst);
}

pub fn uinput_running() -> bool {
    COUNT_OF_UINPUT_DEVICE.load(Ordering::SeqCst) > 0
}

/// `UinputDeviceFile` doesn't implement the `close` method.
impl CloseFreeSafe for UinputDeviceFile {}
impl FileOps for UinputDeviceFile {
    fileops_impl_seekless!();
    fileops_impl_noop_sync!();

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        match request {
            uapi::UI_GET_VERSION => self.ui_get_version(current_task, arg.into()),
            uapi::UI_SET_EVBIT => self.ui_set_evbit(arg),
            uapi::UI_ABS_SETUP => self.ui_abs_setup(current_task, arg.into()),
            // `fuchsia.ui.test.input.Registry` does not use some uinput ioctl
            // request, just ignore the request and return SUCCESS, even args
            // is invalid.
            uapi::UI_SET_KEYBIT
            | uapi::UI_SET_ABSBIT
            | uapi::UI_SET_PHYS
            | uapi::UI_SET_PROPBIT => Ok(SUCCESS),
            uapi::UI_DEV_SETUP => self.ui_dev_setup(current_task, arg.into()),
            uapi::UI_DEV_CREATE => self.ui_dev_create(current_task),
            uapi::UI_DEV_DESTROY => self.ui_dev_destroy(current_task),
            // default_ioctl() handles file system related requests and reject
            // others.
            _ => {
                log_warn!("receive unknown ioctl request: {:?}", request);
                error!(ENOTTY)
            }
        }
    }

    fn write(
        &self,
        _file: &vfs::FileObject,
        current_task: &starnix_core::task::CurrentTask,
        _offset: usize,
        data: &mut dyn vfs::buffers::InputBuffer,
    ) -> Result<usize, Errno> {
        let content = data.read_all()?;
        let event =
            InputEventPtr::read_from_prefix(current_task, &content).map_err(|_| errno!(EINVAL))?;
        let mut inner = self.inner.lock();

        match &mut inner.created_device {
            CreatedDevice::Keyboard(proxy, parser) => {
                let input_report = parser.handle(event);
                match input_report {
                    Ok(Some(report)) => {
                        if let Some(keyboard_report) = report.keyboard {
                            let res = proxy.simulate_key_event(
                                &KeyboardSimulateKeyEventRequest {
                                    pressed_keys: keyboard_report.pressed_keys3,
                                    ..Default::default()
                                },
                                zx::MonotonicInstant::INFINITE,
                            );
                            if res.is_err() {
                                return error!(EIO);
                            }
                        }
                    }
                    Ok(None) => (),
                    Err(e) => return Err(e),
                }
            }
            CreatedDevice::Touchscreen(proxy, parser) => {
                let input_report: Result<Option<futinput::TouchInputReport>, Errno> =
                    parser.handle(event);
                match input_report {
                    Ok(Some(report)) => {
                        let res =
                            proxy.simulate_touch_event(&report, zx::MonotonicInstant::INFINITE);
                        if res.is_err() {
                            return error!(EIO);
                        }
                    }
                    Ok(None) => (),
                    Err(e) => return Err(e),
                }
            }
            CreatedDevice::None => return error!(EINVAL),
        }

        Ok(content.len())
    }

    fn read(
        &self,
        _file: &vfs::FileObject,
        _current_task: &starnix_core::task::CurrentTask,
        _offset: usize,
        _data: &mut dyn vfs::buffers::OutputBuffer,
    ) -> Result<usize, Errno> {
        log_warn!("uinput FD does not support read().");
        error!(EINVAL)
    }
}

#[derive(Clone)]
pub struct VirtualDevice {
    input_id: uapi::input_id,
    devt: DeviceId,
    open_files: OpenedFiles,
    inspect_status: Arc<crate::InputDeviceStatus>,
}

impl DeviceOps for VirtualDevice {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _id: device_id::DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let mut file_nodes = self.inspect_status.file_nodes.lock();
        let child_node =
            self.inspect_status.node.create_child(format!("file_{}", file_nodes.len()));
        let input_file = match &self.devt {
            DeviceId::Keyboard => Arc::new(InputFile::new_keyboard(self.input_id, &child_node)),
            DeviceId::Touchscreen(width, height) => Arc::new(InputFile::new_touch(
                self.input_id,
                width.clone(),
                height.clone(),
                &child_node,
            )),
        };

        file_nodes.push(child_node);
        input_file.init_inspect_status();
        self.open_files.lock().push(Arc::downgrade(&input_file));

        Ok(Box::new(crate::input_file::ArcInputFile(input_file)))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{EventProxyMode, start_input_relays_for_test};
    use starnix_core::testing::spawn_kernel_and_run;
    use starnix_core::vfs::FileHandle;
    use std::sync::Arc;
    use test_case::test_case;

    async fn new_kernel_objects(current_task: &CurrentTask) -> (Arc<UinputDeviceFile>, FileHandle) {
        let (input_relay_handle, _, _, _, _, _, _, _, _, _, _, _) =
            start_input_relays_for_test(current_task, EventProxyMode::None).await;
        let inspector = fuchsia_inspect::Inspector::default();
        let dev = Arc::new(UinputDeviceFile::new(
            input_relay_handle,
            Arc::new(inspector.root().create_child("uinput")),
        ));

        let root_namespace_node = current_task
            .lookup_path_from_root(".".into())
            .expect("failed to get namespace node for root");

        let file_object = FileObject::new(
            current_task,
            Box::new(dev.clone()),
            // The input node doesn't really live at the root of the filesystem.
            // But the test doesn't need to be 100% representative of production.
            root_namespace_node,
            OpenFlags::empty(),
        )
        .expect("FileObject::new failed");
        (dev, file_object)
    }

    #[test_case(uapi::EV_KEY, vec![uapi::EV_KEY as usize] => Ok(SUCCESS))]
    #[test_case(uapi::EV_ABS, vec![uapi::EV_ABS as usize] => Ok(SUCCESS))]
    #[test_case(uapi::EV_REL, vec![] => error!(EPERM))]
    #[::fuchsia::test]
    async fn ui_set_evbit(bit: u32, expected_evbits: Vec<usize>) -> Result<SyscallResult, Errno> {
        spawn_kernel_and_run(async move |current_task| {
            let (dev, file_object) = new_kernel_objects(current_task).await;
            let r = dev.ioctl(
                &file_object,
                current_task,
                uapi::UI_SET_EVBIT,
                SyscallArg::from(bit as u64),
            );
            for expected_evbit in expected_evbits {
                assert!(dev.inner.lock().enabled_evbits.get(expected_evbit).unwrap());
            }
            r
        })
        .await
    }

    #[::fuchsia::test]
    async fn ui_set_evbit_call_multi() {
        spawn_kernel_and_run(async move |current_task| {
            let (dev, file_object) = new_kernel_objects(current_task).await;
            let r = dev.ioctl(
                &file_object,
                current_task,
                uapi::UI_SET_EVBIT,
                SyscallArg::from(uapi::EV_KEY as u64),
            );
            assert_eq!(r, Ok(SUCCESS));
            let r = dev.ioctl(
                &file_object,
                current_task,
                uapi::UI_SET_EVBIT,
                SyscallArg::from(uapi::EV_ABS as u64),
            );
            assert_eq!(r, Ok(SUCCESS));
            assert!(dev.inner.lock().enabled_evbits.get(uapi::EV_KEY as usize).unwrap());
            assert!(dev.inner.lock().enabled_evbits.get(uapi::EV_ABS as usize).unwrap());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn ui_set_keybit() {
        spawn_kernel_and_run(async move |current_task| {
            let (dev, file_object) = new_kernel_objects(current_task).await;
            let r = dev.ioctl(
                &file_object,
                current_task,
                uapi::UI_SET_KEYBIT,
                SyscallArg::from(uapi::BTN_TOUCH as u64),
            );
            assert_eq!(r, Ok(SUCCESS));

            // also test call multi times.
            let r = dev.ioctl(
                &file_object,
                current_task,
                uapi::UI_SET_KEYBIT,
                SyscallArg::from(uapi::KEY_SPACE as u64),
            );
            assert_eq!(r, Ok(SUCCESS));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn ui_set_absbit() {
        spawn_kernel_and_run(async move |current_task| {
            let (dev, file_object) = new_kernel_objects(current_task).await;
            let r = dev.ioctl(
                &file_object,
                current_task,
                uapi::UI_SET_ABSBIT,
                SyscallArg::from(uapi::ABS_MT_SLOT as u64),
            );
            assert_eq!(r, Ok(SUCCESS));

            // also test call multi times.
            let r = dev.ioctl(
                &file_object,
                current_task,
                uapi::UI_SET_ABSBIT,
                SyscallArg::from(uapi::ABS_MT_TOUCH_MAJOR as u64),
            );
            assert_eq!(r, Ok(SUCCESS));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn ui_set_propbit() {
        spawn_kernel_and_run(async move |current_task| {
            let (dev, file_object) = new_kernel_objects(current_task).await;
            let r = dev.ioctl(
                &file_object,
                current_task,
                uapi::UI_SET_PROPBIT,
                SyscallArg::from(uapi::INPUT_PROP_DIRECT as u64),
            );
            assert_eq!(r, Ok(SUCCESS));

            // also test call multi times.
            let r = dev.ioctl(
                &file_object,
                current_task,
                uapi::UI_SET_PROPBIT,
                SyscallArg::from(uapi::INPUT_PROP_DIRECT as u64),
            );
            assert_eq!(r, Ok(SUCCESS));
        })
        .await;
    }
}
