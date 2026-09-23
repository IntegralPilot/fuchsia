// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::file::{
    BufferInfo, ConnectionMap, MagmaBuffer, MagmaConnection, MagmaDevice, MagmaSemaphore,
};
use crate::image_file::{ImageFile, ImageInfo};
use bstr::BString;
use magma::{
    MAGMA_QUERY_VENDOR_ID, MAGMA_STATUS_OK, MAGMA_VENDOR_ID_INTEL, MAGMA_VENDOR_ID_MALI,
    magma_buffer_export, magma_buffer_get_handle, magma_buffer_t, magma_command_descriptor,
    magma_connection_execute_command, magma_connection_execute_inline_commands,
    magma_connection_flush, magma_connection_import_semaphore2,
    magma_connection_read_notification_channel, magma_connection_t, magma_device_create_connection,
    magma_device_import, magma_device_query, magma_device_t, magma_exec_command_buffer,
    magma_exec_resource, magma_handle_t, magma_inline_command_buffer, magma_query_t,
    magma_status_t, virtio_magma_buffer_export_ctrl_t, virtio_magma_buffer_export_resp_t,
    virtio_magma_buffer_get_handle_ctrl_t, virtio_magma_buffer_get_handle_resp_t,
    virtio_magma_connection_execute_command_ctrl_t,
    virtio_magma_connection_execute_inline_commands_ctrl_t, virtio_magma_connection_flush_ctrl_t,
    virtio_magma_connection_flush_resp_t, virtio_magma_connection_read_notification_channel_ctrl_t,
    virtio_magma_connection_read_notification_channel_resp_t,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_EXPORT,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_GET_HANDLE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_FLUSH,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_READ_NOTIFICATION_CHANNEL,
    virtio_magma_device_import_ctrl_t, virtio_magma_device_query_resp_t,
    virtmagma_command_descriptor,
};
use starnix_core::mm::memory::MemoryObject;
use starnix_core::mm::{MemoryAccessor, MemoryAccessorExt};
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{Anon, FdFlags, FsNodeInfo, MemoryRegularFile};
use starnix_logging::track_stub;

use starnix_types::user_buffer::UserBuffer;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::{errno, error};
use std::mem::ManuallyDrop;
use std::sync::Arc;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Reads a sequence of objects starting at `addr`, ensuring at least one element is in the returned
/// Vec.
///
/// # Parameters
///   - `current_task`: The task from which to read the objects.
///   - `addr`: The address of the first item to read.
///   - `item_count`: The number of items to read. If 0, a 1-item vector will be returned to make
///                   sure that the calling code can safely pass `&mut vec[0]` to libmagma.
fn read_objects<T>(
    current_task: &CurrentTask,
    addr: UserAddress,
    item_count: usize,
) -> Result<Vec<T>, Errno>
where
    T: Default + Clone + FromBytes,
{
    Ok(if item_count > 0 {
        let user_ref: UserRef<T> = addr.into();
        current_task.read_objects_to_vec(user_ref, item_count)?
    } else {
        vec![T::default()]
    })
}

/// Creates a connection for a given device.
///
/// # Parameters
///   - `control`: The control struct containing the device to create a connection to.
///   - `response`: The struct that will be filled out to contain the response. This struct can be
///                 written back to userspace.
///
/// SAFETY: Makes an FFI call to populate the fields of `response`.
pub fn create_connection(device: magma_device_t) -> Result<MagmaConnection, magma_status_t> {
    let mut connection_out: magma_connection_t = 0;
    let result = {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            magma_device_create_connection(device, &mut connection_out)
        }
    };
    if result != MAGMA_STATUS_OK {
        return Err(result);
    }

    return Ok(MagmaConnection { handle: connection_out });
}

/// Attempts to open a device at a path. Fails if the device is not a supported one.
///
/// # Parameters
///   - 'path': The filesystem path to open.
///
/// SAFETY: Makes FFI calls to import and query devices.
fn attempt_open_path(
    path: std::path::PathBuf,
    supported_vendor_list: &Vec<u16>,
) -> Result<MagmaDevice, Errno> {
    let path = path.into_os_string().into_string().map_err(|_| errno!(EINVAL))?;
    let (client_channel, server_channel) = zx::Channel::create();

    fdio::service_connect(&path, server_channel).map_err(|_| errno!(EINVAL))?;
    // `magma_device_import` takes ownership of the channel, so don't drop it again.
    let client_channel = ManuallyDrop::new(client_channel);
    let device_channel = client_channel.raw_handle();

    let mut device_out: u64 = 0;
    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    let result = unsafe { magma_device_import(device_channel, &mut device_out as *mut u64) };

    if result != MAGMA_STATUS_OK {
        return error!(EINVAL);
    }
    let magma_device = MagmaDevice { handle: device_out };

    let mut result_out = 0;
    let mut result_buffer_out = 0;
    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    let query_result = unsafe {
        magma_device_query(
            device_out,
            MAGMA_QUERY_VENDOR_ID,
            &mut result_buffer_out,
            &mut result_out,
        )
    };
    if query_result != MAGMA_STATUS_OK {
        return error!(EINVAL);
    }

    let vendor_id = result_out as u16;
    if !supported_vendor_list.contains(&vendor_id) {
        return error!(EINVAL);
    }
    Ok(magma_device)
}

/// Imports a device to magma.
///
/// # Parameters
///   - `control`: The control struct containing the device channel to import from.
///   - `response`: The struct that will be filled out to contain the response. This struct can be
///                 written back to userspace.
///
/// SAFETY: Makes an FFI call to populate the fields of `response`.
pub fn device_import(
    supported_vendors: &Vec<u16>,
    _control: virtio_magma_device_import_ctrl_t,
) -> Result<MagmaDevice, Errno> {
    let entries = std::fs::read_dir("/svc/fuchsia.gpu.magma.Service")
        .map_err(|_| errno!(EINVAL))?
        .filter_map(|x| x.ok());

    let mut magma_devices = entries
        .filter_map(|entry| attempt_open_path(entry.path().join("device"), supported_vendors).ok());
    let magma_device = magma_devices.next().ok_or_else(|| errno!(EINVAL))?;

    if magma_devices.next().is_some() {
        track_stub!(TODO("https://fxbug.dev/297445280"), "expose multiple magma devices");
    }

    Ok(magma_device)
}

fn get_magma_vendor_id(supported_vendor_list: &Vec<u16>) -> Result<u64, Errno> {
    let entries = std::fs::read_dir("/svc/fuchsia.gpu.magma.Service")
        .map_err(|_| errno!(EINVAL))?
        .filter_map(|x| x.ok());

    let mut magma_devices = entries.filter_map(|entry| {
        attempt_open_path(entry.path().join("device"), supported_vendor_list).ok()
    });
    let magma_device = magma_devices.next().ok_or_else(|| errno!(EINVAL))?;
    let mut result_out = 0;
    let mut result_buffer_out = 0;
    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    let query_result = unsafe {
        magma_device_query(
            magma_device.handle,
            MAGMA_QUERY_VENDOR_ID,
            &mut result_buffer_out,
            &mut result_out,
        )
    };
    if query_result != MAGMA_STATUS_OK {
        return error!(EINVAL);
    }
    return Ok(result_out);
}

/// Returns a command-line that helps Android select the correct GPU driver.
pub fn get_magma_params(supported_vendor_list: &Vec<u16>) -> BString {
    if let Some(magma_vendor_id) = get_magma_vendor_id(supported_vendor_list).ok() {
        const MAGMA_VENDOR_ID_MALI_U64: u64 = MAGMA_VENDOR_ID_MALI as u64;
        const MAGMA_VENDOR_ID_INTEL_U64: u64 = MAGMA_VENDOR_ID_INTEL as u64;
        let gpu_type = match magma_vendor_id {
            MAGMA_VENDOR_ID_MALI_U64 => "mali",
            MAGMA_VENDOR_ID_INTEL_U64 => "intel",
            _ => "default",
        };
        return BString::from(
            "androidboot.vendor.apex.com.fuchsia.vulkan=com.fuchsia.vulkan.".to_owned() + gpu_type,
        );
    }
    return b"androidboot.vendor.apex.com.fuchsia.vulkan=com.fuchsia.vulkan.default".into();
}

/// `WireDescriptor` matches the struct used by libmagma_virt to encode some fields of the magma
/// command descriptor.
#[repr(C)]
#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Default, Debug)]
struct WireDescriptor {
    resource_count: u32,
    command_buffer_count: u32,
    wait_semaphore_count: u32,
    signal_semaphore_count: u32,
    flags: u64,
}

/// Executes a magma command.
///
/// This function bridges between the virtmagma structs and the magma structures. It also copies the
/// data into starnix in order to be able to pass pointers to the resources, command buffers, and
/// semaphore ids to magma.
///
/// SAFETY: Makes an FFI call to magma_connection_execute_command().
pub fn execute_command<F>(
    current_task: &CurrentTask,
    control: virtio_magma_connection_execute_command_ctrl_t,
    connection: &Arc<MagmaConnection>,
    get_semaphore: F,
) -> Result<i32, Errno>
where
    F: Fn(u64) -> Result<Arc<MagmaSemaphore>, i32>,
{
    let virtmagma_command_descriptor_addr =
        UserRef::<virtmagma_command_descriptor>::new(control.descriptor.into());
    let command_descriptor = current_task.read_object(virtmagma_command_descriptor_addr)?;

    // Read the struct that contains the counts and flags for the magma command descriptor.
    let wire_descriptor: WireDescriptor =
        current_task.read_object(UserAddress::from(command_descriptor.descriptor).into())?;

    let semaphore_count =
        wire_descriptor.wait_semaphore_count + wire_descriptor.signal_semaphore_count;

    let mut status: i32 = MAGMA_STATUS_OK;
    let mut wait_semaphore_count: u32 = 0;
    let mut signal_semaphore_count: u32 = 0;
    let mut child_semaphore_ids: Vec<u64> = vec![];

    if semaphore_count > 0 {
        let semaphore_ids: Vec<u64> = read_objects(
            current_task,
            command_descriptor.semaphores.into(),
            semaphore_count as usize,
        )?;

        for (i, id) in semaphore_ids.iter().enumerate() {
            match get_semaphore(*id) {
                Ok(semaphore) => {
                    let ids_ref = &semaphore.ids;
                    for id in ids_ref {
                        child_semaphore_ids.push(*id);
                    }
                    if i < wire_descriptor.wait_semaphore_count as usize {
                        wait_semaphore_count += ids_ref.len() as u32;
                    } else {
                        signal_semaphore_count += ids_ref.len() as u32;
                    }
                }
                Err(s) => {
                    status = s;
                    break;
                }
            }
        }
    } else {
        // So we can dereference index 0 below
        child_semaphore_ids.push(0);
    }

    if status == MAGMA_STATUS_OK {
        // This is the command descriptor that will be populated from the virtmagma
        // descriptor and subsequently passed to magma_execute_command.
        let mut magma_command_descriptor = magma_command_descriptor {
            resource_count: wire_descriptor.resource_count,
            command_buffer_count: wire_descriptor.command_buffer_count,
            wait_semaphore_count,
            signal_semaphore_count,
            flags: wire_descriptor.flags,
            ..Default::default()
        };

        // Read all the passed in resources, commands, and semaphore ids.
        let mut resources: Vec<magma_exec_resource> = read_objects(
            current_task,
            command_descriptor.resources.into(),
            wire_descriptor.resource_count as usize,
        )?;
        let mut command_buffers: Vec<magma_exec_command_buffer> = read_objects(
            current_task,
            command_descriptor.command_buffers.into(),
            wire_descriptor.command_buffer_count as usize,
        )?;

        // Make sure the command descriptor contains valid pointers for the resources, command buffers,
        // and semaphore ids.
        magma_command_descriptor.resources = &mut resources[0] as *mut magma_exec_resource;
        magma_command_descriptor.command_buffers =
            &mut command_buffers[0] as *mut magma_exec_command_buffer;
        magma_command_descriptor.semaphore_ids = &mut child_semaphore_ids[0] as *mut u64;

        status = {
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            unsafe {
                magma_connection_execute_command(
                    connection.handle,
                    control.context_id,
                    &mut magma_command_descriptor as *mut magma_command_descriptor,
                )
            }
        };
    }

    Ok(status)
}

/// Executes magma immediate commands.
///
/// This function bridges between the virtmagma structs and the magma structures. It also copies the
/// data into starnix in order to be able to pass pointers to the resources, command buffers, and
/// semaphore ids to magma.
///
/// SAFETY: Makes an FFI call to magma_execute_inline_commands().
pub fn execute_inline_commands<F>(
    current_task: &CurrentTask,
    control: virtio_magma_connection_execute_inline_commands_ctrl_t,
    connection: &Arc<MagmaConnection>,
    get_semaphore: F,
) -> Result<magma_status_t, Errno>
where
    F: Fn(u64) -> Result<Arc<MagmaSemaphore>, i32>,
{
    let command_buffers_addr = UserAddress::from(control.command_buffers);

    // For virtio-magma, "command_buffers" is an array of virtmagma_command_descriptor instead of
    // magma_inline_command_buffer.
    let descriptors: Vec<virtmagma_command_descriptor> =
        read_objects(current_task, command_buffers_addr, control.command_count as usize)?;

    let mut commands =
        vec![magma_inline_command_buffer::default(); std::cmp::max(descriptors.len(), 1)];

    let mut commands_vec = Vec::<Vec<u8>>::with_capacity(control.command_count as usize);
    let mut semaphore_ids_vec = Vec::<Vec<u64>>::with_capacity(control.command_count as usize);

    let mut status: i32 = MAGMA_STATUS_OK;

    for i in 0..control.command_count as usize {
        let size = descriptors[i].command_buffer_size;
        let data = current_task.read_buffer(&UserBuffer {
            address: UserAddress::from(descriptors[i].command_buffers),
            length: size as usize,
        })?;
        commands_vec.push(data);
        commands[i].size = size;

        let semaphore_count =
            (descriptors[i].semaphore_size / core::mem::size_of::<u64>() as u64) as u32;

        let mut child_semaphore_ids: Vec<u64> = vec![];
        if semaphore_count > 0 {
            let semaphore_ids = read_objects(
                current_task,
                UserAddress::from(descriptors[i].semaphores),
                semaphore_count as usize,
            )?;

            for id in semaphore_ids.iter() {
                match get_semaphore(*id) {
                    Ok(semaphore) => {
                        let child_ids_ref = &semaphore.ids;
                        for child_id in child_ids_ref {
                            child_semaphore_ids.push(*child_id);
                        }
                    }
                    Err(s) => {
                        status = s;
                        break;
                    }
                }
            }
        }
        commands[i].semaphore_count = child_semaphore_ids.len() as u32;
        semaphore_ids_vec.push(child_semaphore_ids);
    }

    if status == MAGMA_STATUS_OK {
        status = {
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            unsafe {
                for i in 0..control.command_count as usize {
                    commands[i].data = &mut commands_vec[i][0] as *mut u8 as *mut std::ffi::c_void;
                    commands[i].semaphore_ids = if commands[i].semaphore_count == 0 {
                        std::ptr::null_mut()
                    } else {
                        &mut semaphore_ids_vec[i][0]
                    };
                }
                magma_connection_execute_inline_commands(
                    connection.handle,
                    control.context_id,
                    control.command_count,
                    &mut commands[0],
                )
            }
        };
    }

    Ok(status)
}

/// Exports the provided magma buffer into a `zx::Vmo`, which is then wrapped in a file and added
/// to `current_task`'s files.
///
/// The file's `fd` is then written to the response object, which allows the client to interact with
/// the exported buffer.
///
/// # Parameters
///   - `current_task`: The task that is exporting the buffer.
///   - `control`: The control message that contains the buffer to export.
///   - `response`: The response message that will be updated to write back to user space.
///
/// Returns an error if adding the file to `current_task` fails.
///
/// SAFETY: Makes an FFI call to populate the fields of `response`, and creates a `zx::Vmo` from
/// a raw handle provided by magma.
pub fn export_buffer(
    current_task: &CurrentTask,
    _control: virtio_magma_buffer_export_ctrl_t,
    response: &mut virtio_magma_buffer_export_resp_t,
    buffer: &Arc<MagmaBuffer>,
    connections: &ConnectionMap,
) -> Result<(), Errno> {
    let mut buffer_handle_out = 0;
    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    let status = unsafe {
        magma_buffer_export(
            buffer.handle as magma_buffer_t,
            &mut buffer_handle_out as *mut magma_handle_t,
        )
    };
    if status == MAGMA_STATUS_OK {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let vmo = unsafe { zx::Vmo::from(zx::NullableHandle::from_raw(buffer_handle_out)) };
        let memory = MemoryObject::from(vmo);

        let mut image_info_opt: Option<ImageInfo> = None;
        'outer: for image_map in connections.values() {
            for (image, info) in image_map.buffer_map.iter() {
                if *image == buffer.handle {
                    if let BufferInfo::Image(image_info) = info.clone() {
                        image_info_opt = Some(image_info);
                        break 'outer;
                    }
                }
            }
        }

        let file = {
            if let Some(image_info) = image_info_opt {
                ImageFile::new_file(current_task, image_info, memory)
            } else {
                // TODO: https://fxbug.dev/404739824 - Confirm whether to handle this as a "private" node.
                Anon::new_private_file(
                    current_task,
                    Box::new(MemoryRegularFile::new(Arc::new(memory))),
                    OpenFlags::RDWR,
                    "[fuchsia:magma_export_buffer]",
                )
            }
        };

        let fd = current_task.add_file(file, FdFlags::empty())?;
        response.buffer_handle_out = fd.raw() as u64;
    }

    response.result_return = status as u64;
    response.hdr.type_ = virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_EXPORT as u32;

    Ok(())
}

/// Calls flush on the provided `control.connection`.
///
/// SAFETY: Makes an FFI call to magma, which is expected to handle invalid connection parameters.
pub fn flush(
    _control: virtio_magma_connection_flush_ctrl_t,
    response: &mut virtio_magma_connection_flush_resp_t,
    connection: &Arc<MagmaConnection>,
) {
    response.result_return = {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            magma_connection_flush(connection.handle) as u64
        }
    };
    response.hdr.type_ = virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_FLUSH as u32;
}

/// Fetches a VMO handles from magma wraps it in a file, then adds that file to `current_task`.
///
/// # Parameters
///   - `current_task`: The task that the created file is added to, in `anon_fs`.
///   - `control`: The control message containing the image handle.
///   - `response`: The response which will contain the file descriptor for the created file.
///
/// SAFETY: Makes an FFI call to fetch a VMO handle. The VMO handle is expected to be valid if the
/// FFI call succeeds. Either way, creating a `zx::Vmo` with an invalid handle is safe.
pub fn get_buffer_handle(
    current_task: &CurrentTask,
    _control: virtio_magma_buffer_get_handle_ctrl_t,
    response: &mut virtio_magma_buffer_get_handle_resp_t,
    buffer: &Arc<MagmaBuffer>,
) -> Result<(), Errno> {
    let mut buffer_handle_out = 0;
    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    let status = unsafe {
        magma_buffer_get_handle(
            buffer.handle as magma_buffer_t,
            &mut buffer_handle_out as *mut magma_handle_t,
        )
    };

    if status != MAGMA_STATUS_OK {
        response.result_return = status as u64;
    } else {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let vmo = unsafe { zx::Vmo::from(zx::NullableHandle::from_raw(buffer_handle_out)) };
        let memory = MemoryObject::from(vmo);
        // TODO: https://fxbug.dev/404739824 - Confirm whether to handle this as a "private" node.
        let file = Anon::new_private_file(
            current_task,
            Box::new(MemoryRegularFile::new(Arc::new(memory))),
            OpenFlags::RDWR,
            "[fuchsia:magma_buffer]",
        );
        let fd = current_task.add_file(file, FdFlags::empty())?;
        response.handle_out = fd.raw() as u64;
        response.result_return = MAGMA_STATUS_OK as u64;
    }

    response.hdr.type_ = virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_GET_HANDLE as u32;

    Ok(())
}

/// Runs a magma query.
///
/// This function will create a new file in `current_task.files` if the magma query returns a VMO
/// handle. The file takes ownership of the VMO handle, and the file descriptor of the file is
/// returned to the client via `response`.
///
/// SAFETY: Makes an FFI call to populate the fields of `response`.
pub fn query(
    current_task: &CurrentTask,
    device: magma_device_t,
    id: magma_query_t,
    response: &mut virtio_magma_device_query_resp_t,
) -> Result<(), Errno> {
    let mut result_buffer_out = 0;
    let mut result_out = 0;
    response.result_return = {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            magma_device_query(device, id, &mut result_buffer_out, &mut result_out) as u64
        }
    };

    if result_buffer_out != zx::sys::ZX_HANDLE_INVALID {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let vmo = unsafe { zx::Vmo::from(zx::NullableHandle::from_raw(result_buffer_out)) };
        let memory = MemoryObject::from(vmo);
        let memory_size = memory.get_size();
        // TODO: https://fxbug.dev/404739824 - Confirm whether to handle this as a "private" node.
        let mut info = FsNodeInfo::new(FileMode::from_bits(0o600), current_task.current_fscred());
        // Enable seek for file size discovery.
        info.size = memory_size as usize;
        let file = Anon::new_private_file_extended(
            current_task,
            Box::new(MemoryRegularFile::new(Arc::new(memory))),
            OpenFlags::RDWR,
            "[fuchsia:magma_vmo]",
            info,
        );
        let fd = current_task.add_file(file, FdFlags::empty())?;
        response.result_buffer_out = fd.raw() as u64;
    } else {
        response.result_buffer_out = u64::MAX;
    }

    response.result_out = result_out;

    Ok(())
}

/// Reads a notification from the connection channel and writes it to `control.buffer`.
///
/// Upon completion, `response.more_data_out` will be true if there is more data waiting to be read.
/// `response.buffer_size_out` contains the size of the returned buffer.
///
/// SAFETY: Makes an FFI call to magma with a buffer that is populated with data. The passed in
/// buffer pointer always points to a valid vector, even if the provided buffer length is 0.
pub fn read_notification_channel(
    current_task: &CurrentTask,
    control: virtio_magma_connection_read_notification_channel_ctrl_t,
    response: &mut virtio_magma_connection_read_notification_channel_resp_t,
    connection: &Arc<MagmaConnection>,
) -> Result<(), Errno> {
    // Buffer has a min length of 1 to make sure the call to
    // `magma_read_notification_channel2` uses a valid reference.
    let mut buffer = vec![0; std::cmp::max(control.buffer_size as usize, 1)];
    let mut buffer_size_out = 0;
    let mut more_data_out: u8 = 0;

    response.result_return = {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            magma_connection_read_notification_channel(
                connection.handle,
                &mut buffer[0] as *mut u8 as *mut std::ffi::c_void,
                control.buffer_size,
                &mut buffer_size_out,
                &mut more_data_out as *mut u8,
            ) as u64
        }
    };

    response.more_data_out = more_data_out as u64;
    response.buffer_size_out = buffer_size_out;
    response.hdr.type_ =
        virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_READ_NOTIFICATION_CHANNEL as u32;

    current_task.write_memory(UserAddress::from(control.buffer), &buffer)?;

    Ok(())
}

pub fn import_semaphore2(
    connection: &Arc<MagmaConnection>,
    counter: zx::Counter,
    flags: u64,
) -> (i32, u64, u64) {
    let mut semaphore = 0;
    let mut semaphore_id = 0;
    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    let status = unsafe {
        magma_connection_import_semaphore2(
            connection.handle,
            counter.into_raw(),
            flags,
            &mut semaphore,
            &mut semaphore_id,
        )
    };
    (status, semaphore, semaphore_id)
}
