// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::device::DeviceOps;
use starnix_core::mm::MemoryAccessorExt;
use starnix_core::task::{CurrentTask, EventHandler, Kernel, WaitCanceler, Waiter};
use starnix_core::vfs::{
    Anon, FdFlags, FileObject, FileOps, NamespaceNode, fileops_impl_dataless,
    fileops_impl_nonseekable, fileops_impl_noop_sync,
};

use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::{Errno, error};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::FdEvents;
use zerocopy::{FromBytes, Immutable, IntoBytes};

const GPIO_MAX_NAME_SIZE: usize = 32;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, FromBytes, IntoBytes, Immutable)]
pub struct gpio_v2_line_attribute {
    pub id: u32,
    pub padding: u32,
    pub value: u64,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, FromBytes, IntoBytes, Immutable)]
pub struct gpio_v2_line_config_attribute {
    pub attr: gpio_v2_line_attribute,
    pub mask: u64,
}

const GPIO_V2_LINE_NUM_ATTRS_MAX: usize = 10;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, FromBytes, IntoBytes, Immutable)]
pub struct gpio_v2_line_config {
    pub flags: u64,
    pub num_attrs: u32,
    pub padding: [u32; 5],
    pub attrs: [gpio_v2_line_config_attribute; GPIO_V2_LINE_NUM_ATTRS_MAX],
}

const GPIO_V2_LINES_MAX: usize = 64;

#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable)]
pub struct gpio_v2_line_request {
    pub offsets: [u32; GPIO_V2_LINES_MAX],
    pub consumer: [u8; GPIO_MAX_NAME_SIZE],
    pub config: gpio_v2_line_config,
    pub num_lines: u32,
    pub event_buffer_size: u32,
    pub padding: [u32; 5],
    pub fd: i32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, FromBytes, IntoBytes, Immutable)]
pub struct gpio_v2_line_values {
    pub bits: u64,
    pub mask: u64,
}

const GPIO_V2_GET_LINE_IOCTL: u32 = 0xC250B407;
const GPIO_V2_LINE_GET_VALUES_IOCTL: u32 = 0xC010B40E;
//const GPIO_V2_LINE_SET_VALUES_IOCTL: u32 = 0xC010B40F;

#[derive(Clone)]
struct GpioChipDevice;

impl DeviceOps for GpioChipDevice {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _id: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(GpioChipFile))
    }
}

struct GpioChipFile;

impl FileOps for GpioChipFile {
    fileops_impl_nonseekable!();
    fileops_impl_dataless!();
    fileops_impl_noop_sync!();

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        match request {
            GPIO_V2_GET_LINE_IOCTL => {
                let mut req: gpio_v2_line_request = current_task.read_object(arg.into())?;
                if req.num_lines == 0 || req.num_lines > GPIO_V2_LINES_MAX as u32 {
                    return error!(EINVAL);
                }

                let line_file = GpioLineFile {};

                let handle = Anon::new_private_file(
                    current_task,
                    Box::new(line_file),
                    OpenFlags::RDONLY | OpenFlags::CLOEXEC,
                    "gpiochipwake0",
                );
                let fd = current_task.add_file(handle, FdFlags::empty())?;
                req.fd = fd.raw();

                current_task.write_object(arg.into(), &req)?;
                Ok(SUCCESS)
            }
            _ => error!(ENOSYS),
        }
    }
}

struct GpioLineFile {}

impl FileOps for GpioLineFile {
    fileops_impl_nonseekable!();
    fileops_impl_dataless!();
    fileops_impl_noop_sync!();

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        match request {
            GPIO_V2_LINE_GET_VALUES_IOCTL => {
                let mut values: gpio_v2_line_values = current_task.read_object(arg.into())?;
                // Stubbed implementation will always return inactive here
                values.bits = 0;
                current_task.write_object(arg.into(), &values)?;
                Ok(SUCCESS)
            }
            _ => error!(ENOSYS),
        }
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        Ok(FdEvents::empty())
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        _events: FdEvents,
        _handler: EventHandler,
    ) -> Option<WaitCanceler> {
        Some(waiter.fake_wait())
    }
}

pub fn register_gpio_chip_device(kernel: &Kernel, name: &str) {
    let registry = &kernel.device_registry;
    let device_class =
        registry.objects.get_or_create_class("gpio".into(), registry.objects.virtual_bus());
    registry
        .register_dyn_device(kernel, name.as_bytes().into(), device_class, GpioChipDevice)
        .expect("Can register GPIO chip device");
}
