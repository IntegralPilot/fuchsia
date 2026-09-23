// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_upper_case_globals)]

use crate::ffi::{
    create_connection, device_import, execute_command, execute_inline_commands, export_buffer,
    flush, get_buffer_handle, import_semaphore2, query, read_notification_channel,
};
use crate::image_file::{ImageFile, ImageInfo};
use crate::magma::{StarnixPollItem, read_control_and_response, read_magma_command_and_type};

use magma::{
    MAGMA_CACHE_POLICY_CACHED, MAGMA_IMPORT_SEMAPHORE_ONE_SHOT, MAGMA_POLL_CONDITION_SIGNALED,
    MAGMA_POLL_TYPE_SEMAPHORE, MAGMA_PRIORITY_MEDIUM, MAGMA_STATUS_INVALID_ARGS,
    MAGMA_STATUS_MEMORY_ERROR, MAGMA_STATUS_OK, MAGMA_STATUS_TIMED_OUT, magma_buffer_clean_cache,
    magma_buffer_get_cache_policy, magma_buffer_get_info, magma_buffer_id_t, magma_buffer_info_t,
    magma_buffer_set_cache_policy, magma_buffer_set_name, magma_buffer_t, magma_cache_operation_t,
    magma_cache_policy_t, magma_connection_create_buffer, magma_connection_create_context,
    magma_connection_create_context2, magma_connection_get_error,
    magma_connection_get_notification_channel_handle, magma_connection_import_buffer,
    magma_connection_map_buffer, magma_connection_perform_buffer_op, magma_connection_release,
    magma_connection_release_buffer, magma_connection_release_context,
    magma_connection_release_semaphore, magma_connection_t, magma_connection_unmap_buffer,
    magma_device_release, magma_device_t, magma_initialize_logging, magma_poll, magma_poll_item,
    magma_poll_item_t, magma_semaphore_export, magma_semaphore_id_t, magma_semaphore_reset,
    magma_semaphore_signal, magma_semaphore_t, virtio_magma_buffer_clean_cache_ctrl_t,
    virtio_magma_buffer_clean_cache_resp_t, virtio_magma_buffer_export_ctrl_t,
    virtio_magma_buffer_export_resp_t, virtio_magma_buffer_get_cache_policy_ctrl_t,
    virtio_magma_buffer_get_cache_policy_resp_t, virtio_magma_buffer_get_handle_ctrl_t,
    virtio_magma_buffer_get_handle_resp_t, virtio_magma_buffer_get_info_ctrl_t,
    virtio_magma_buffer_get_info_resp_t, virtio_magma_buffer_set_cache_policy_ctrl_t,
    virtio_magma_buffer_set_cache_policy_resp_t, virtio_magma_buffer_set_name_ctrl_t,
    virtio_magma_buffer_set_name_resp_t, virtio_magma_connection_create_buffer_ctrl_t,
    virtio_magma_connection_create_buffer_resp_t, virtio_magma_connection_create_context_ctrl_t,
    virtio_magma_connection_create_context_resp_t, virtio_magma_connection_create_context2_ctrl_t,
    virtio_magma_connection_create_context2_resp_t,
    virtio_magma_connection_create_semaphore_ctrl_t,
    virtio_magma_connection_create_semaphore_resp_t,
    virtio_magma_connection_execute_command_ctrl_t, virtio_magma_connection_execute_command_resp_t,
    virtio_magma_connection_execute_inline_commands_ctrl_t,
    virtio_magma_connection_execute_inline_commands_resp_t, virtio_magma_connection_flush_ctrl_t,
    virtio_magma_connection_flush_resp_t, virtio_magma_connection_get_error_ctrl_t,
    virtio_magma_connection_get_error_resp_t,
    virtio_magma_connection_get_notification_channel_handle_ctrl_t,
    virtio_magma_connection_get_notification_channel_handle_resp_t,
    virtio_magma_connection_import_buffer_ctrl_t, virtio_magma_connection_import_buffer_resp_t,
    virtio_magma_connection_import_semaphore2_ctrl_t,
    virtio_magma_connection_import_semaphore2_resp_t, virtio_magma_connection_map_buffer_ctrl_t,
    virtio_magma_connection_map_buffer_resp_t, virtio_magma_connection_perform_buffer_op_ctrl_t,
    virtio_magma_connection_perform_buffer_op_resp_t,
    virtio_magma_connection_read_notification_channel_ctrl_t,
    virtio_magma_connection_read_notification_channel_resp_t,
    virtio_magma_connection_release_buffer_ctrl_t, virtio_magma_connection_release_buffer_resp_t,
    virtio_magma_connection_release_context_ctrl_t, virtio_magma_connection_release_context_resp_t,
    virtio_magma_connection_release_ctrl_t, virtio_magma_connection_release_resp_t,
    virtio_magma_connection_release_semaphore_ctrl_t,
    virtio_magma_connection_release_semaphore_resp_t, virtio_magma_connection_unmap_buffer_ctrl_t,
    virtio_magma_connection_unmap_buffer_resp_t,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_CLEAN_CACHE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_EXPORT,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_GET_CACHE_POLICY,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_GET_HANDLE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_GET_INFO,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_SET_CACHE_POLICY,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_SET_NAME,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_CREATE_BUFFER,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_CREATE_CONTEXT,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_CREATE_CONTEXT2,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_CREATE_SEMAPHORE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_EXECUTE_COMMAND,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_EXECUTE_INLINE_COMMANDS,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_FLUSH,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_GET_ERROR,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_GET_NOTIFICATION_CHANNEL_HANDLE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_IMPORT_BUFFER,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_IMPORT_SEMAPHORE2,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_MAP_BUFFER,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_PERFORM_BUFFER_OP,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_READ_NOTIFICATION_CHANNEL,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_RELEASE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_RELEASE_BUFFER,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_RELEASE_CONTEXT,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_RELEASE_SEMAPHORE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_UNMAP_BUFFER,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_DEVICE_CREATE_CONNECTION,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_DEVICE_IMPORT,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_DEVICE_QUERY,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_DEVICE_RELEASE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_POLL,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_SEMAPHORE_EXPORT,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_SEMAPHORE_RESET,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_SEMAPHORE_SIGNAL,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_CLEAN_CACHE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_GET_CACHE_POLICY,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_GET_INFO,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_SET_CACHE_POLICY,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_SET_NAME,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_CREATE_BUFFER,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_CREATE_CONTEXT,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_CREATE_CONTEXT2,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_CREATE_SEMAPHORE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_EXECUTE_COMMAND,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_EXECUTE_INLINE_COMMANDS,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_GET_ERROR,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_GET_NOTIFICATION_CHANNEL_HANDLE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_IMPORT_BUFFER,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_IMPORT_SEMAPHORE2,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_MAP_BUFFER,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_PERFORM_BUFFER_OP,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_RELEASE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_RELEASE_BUFFER,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_RELEASE_CONTEXT,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_RELEASE_SEMAPHORE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_UNMAP_BUFFER,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_DEVICE_CREATE_CONNECTION,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_DEVICE_IMPORT,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_DEVICE_QUERY,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_DEVICE_RELEASE,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_POLL,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_SEMAPHORE_EXPORT,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_SEMAPHORE_RESET,
    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_SEMAPHORE_SIGNAL,
    virtio_magma_device_create_connection_ctrl, virtio_magma_device_create_connection_resp_t,
    virtio_magma_device_import_ctrl_t, virtio_magma_device_import_resp_t,
    virtio_magma_device_query_ctrl_t, virtio_magma_device_query_resp_t,
    virtio_magma_device_release_ctrl_t, virtio_magma_device_release_resp_t,
    virtio_magma_poll_ctrl_t, virtio_magma_poll_resp_t, virtio_magma_semaphore_export_ctrl_t,
    virtio_magma_semaphore_export_resp_t, virtio_magma_semaphore_reset_ctrl_t,
    virtio_magma_semaphore_reset_resp_t, virtio_magma_semaphore_signal_ctrl_t,
    virtio_magma_semaphore_signal_resp_t, virtmagma_buffer_set_name_wrapper,
};
use starnix_core::fileops_impl_nonseekable;
use starnix_core::fs::fuchsia::sync_file::{SyncFence, SyncFile, SyncPoint, Timeline};
use starnix_core::fs::fuchsia::{
    AnonymousRemoteFileObject, RemoteFileObject, RemoteZxioFileObject,
};
use starnix_core::mm::memory::MemoryObject;
use starnix_core::mm::{MemoryAccessorExt, ProtectionFlags};
use starnix_core::task::CurrentTask;
use starnix_core::vfs::buffers::{InputBuffer, OutputBuffer};
use starnix_core::vfs::{
    FdFlags, FdNumber, FileObject, FileOps, FsNode, MemoryRegularFile, fileops_impl_noop_sync,
};
use starnix_lifecycle::AtomicCounter;
use starnix_logging::{impossible_error, log_error, log_warn, track_stub};
use starnix_sync::{
    LockDepMutex, MagmaBuffersLock, MagmaConnectionsLock, MagmaDevicesLock, MagmaSemaphoresLock,
};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_types::user_buffer::UserBuffer;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::{errno, error};
use std::collections::HashMap;
use std::sync::{Arc, Once};
use zerocopy::IntoBytes;

#[derive(Clone)]
pub enum BufferInfo {
    Default,
    Image(ImageInfo),
}

/// A `MagmaConnection` is an RAII wrapper around a `magma_connection_t`.
pub struct MagmaConnection {
    pub handle: magma_connection_t,
}

impl Drop for MagmaConnection {
    fn drop(&mut self) {
        #[allow(clippy::undocumented_unsafe_blocks)]
        unsafe {
            magma_connection_release(self.handle)
        }
    }
}

/// A `MagmaDevice` is an RAII wrapper around a `magma_device_t`.
pub struct MagmaDevice {
    pub handle: magma_device_t,
}

impl Drop for MagmaDevice {
    /// SAFETY: Makes an FFI call to release a handle that was imported using `magma_device_import`.
    fn drop(&mut self) {
        #[allow(clippy::undocumented_unsafe_blocks)]
        unsafe {
            magma_device_release(self.handle)
        }
    }
}

/// A `MagmaBuffer` is an RAII wrapper around a `magma_buffer_t`.
pub struct MagmaBuffer {
    // This reference is needed to release the buffer.
    pub connection: Arc<MagmaConnection>,
    pub handle: magma_buffer_t,
}

impl Drop for MagmaBuffer {
    /// SAFETY: Makes an FFI call to release a a `magma_buffer_t` handle. `connection.handle` must
    /// be valid because `connection` is refcounted.
    fn drop(&mut self) {
        #[allow(clippy::undocumented_unsafe_blocks)]
        unsafe {
            magma_connection_release_buffer(self.connection.handle, self.handle)
        }
    }
}

/// A `MagmaSemaphore` is an RAII wrapper around one or more `magma_semaphore_t`.  Multiple are
/// supported because a sync file may be imported.
pub struct MagmaSemaphore {
    // This reference is needed to release the semaphores.
    pub connection: Arc<MagmaConnection>,
    pub handles: Vec<magma_semaphore_t>,
    pub ids: Vec<magma_semaphore_id_t>,
}

impl Drop for MagmaSemaphore {
    /// SAFETY: Makes an FFI call to release the `magma_semaphore_t` handles. `connection.handle` must
    /// be valid because `connection` is refcounted.
    fn drop(&mut self) {
        for handle in &self.handles {
            #[allow(clippy::undocumented_unsafe_blocks)]
            unsafe {
                magma_connection_release_semaphore(self.connection.handle, *handle)
            }
        }
    }
}

/// A `BufferMap` stores all the magma buffers for a given connection.
type BufferMap = HashMap<magma_buffer_id_t, BufferInfo>;

/// A `ConnectionMap` stores the `ConnectionInfo`s associated with each magma connection.
pub type ConnectionMap = HashMap<u64, ConnectionInfo>;

pub struct ConnectionInfo {
    pub connection: Arc<MagmaConnection>,
    pub buffer_map: BufferMap,
}

impl ConnectionInfo {
    pub fn new(connection: Arc<MagmaConnection>) -> Self {
        Self { connection, buffer_map: HashMap::new() }
    }
}

pub type DeviceMap = HashMap<u64, Arc<MagmaDevice>>;

pub struct MagmaFile {
    supported_vendors: Vec<u16>,
    devices: Arc<LockDepMutex<DeviceMap, MagmaDevicesLock>>,
    connections: Arc<LockDepMutex<ConnectionMap, MagmaConnectionsLock>>,
    buffers: Arc<LockDepMutex<HashMap<magma_buffer_id_t, Arc<MagmaBuffer>>, MagmaBuffersLock>>,
    semaphores:
        Arc<LockDepMutex<HashMap<magma_semaphore_t, Arc<MagmaSemaphore>>, MagmaSemaphoresLock>>,
    semaphore_id_generator: AtomicCounter<u64>,
}

impl MagmaFile {
    pub fn init() {
        // Enable the magma client library to emit logs for debug and error cases.
        let (server_end, client_end) = zx::Channel::create();

        let result = fuchsia_component::client::connect_channel_to_protocol::<
            fidl_fuchsia_logger::LogSinkMarker,
        >(server_end);

        if result.is_ok() {
            #[allow(clippy::undocumented_unsafe_blocks)]
            unsafe {
                magma_initialize_logging(client_end.into_raw());
            }
        }
    }

    pub fn new_file(
        _current_task: &CurrentTask,
        _dev: DeviceId,
        _node: &FsNode,
        _flags: OpenFlags,
        supported_vendors: Vec<u16>,
    ) -> Result<Box<dyn FileOps>, Errno> {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            Self::init();
        });

        Ok(Box::new(Self {
            supported_vendors,
            devices: Default::default(),
            connections: Default::default(),
            buffers: Default::default(),
            semaphores: Default::default(),
            semaphore_id_generator: AtomicCounter::new(1),
        }))
    }

    /// Returns a duplicate of the VMO associated with the file at `fd`, as well as a `BufferInfo`
    /// of the correct type for that file.
    ///
    /// Returns an error if the file does not contain a buffer.
    fn get_memory_and_magma_buffer(
        current_task: &CurrentTask,
        fd: FdNumber,
    ) -> Result<(MemoryObject, BufferInfo), Errno> {
        let file = current_task.files().get(fd)?;
        if let Some(file) = file.downcast_file::<ImageFile>() {
            let buffer = BufferInfo::Image(file.info.clone());
            Ok((
                file.memory.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(impossible_error)?,
                buffer,
            ))
        } else if let Some(file) = file.downcast_file::<MemoryRegularFile>() {
            let buffer = BufferInfo::Default;
            Ok((
                file.memory.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(impossible_error)?,
                buffer,
            ))
        } else if file.downcast_file::<RemoteFileObject>().is_some()
            || file.downcast_file::<RemoteZxioFileObject>().is_some()
            || file.downcast_file::<AnonymousRemoteFileObject>().is_some()
        {
            // TODO: Currently this does not preserve BufferInfo::Image fields across allocation via
            // HIDL/AIDL gralloc followed by import here. If that turns out to be needed, we can add
            // to system the ability to get ImageFormatConstraints from a sysmem VMO, which could be
            // used instead of ImageFile. Or if not needed, maybe we can remove ImageFile without
            // any replacement.
            //
            // TODO: Consider if we can have binder related code in starnix use MemoryRegularFile for
            // any FD wrapping a VMO, or if that's not workable, we may want to have magma related
            // code use RemoteFileObject.
            let buffer = BufferInfo::Default;
            // Map any failure to EINVAL; any failure here is most likely to be an FD that isn't
            // a gralloc buffer.
            let memory = file
                .get_memory(current_task, None, ProtectionFlags::READ | ProtectionFlags::WRITE)
                .map_err(|_| errno!(EINVAL))?;
            Ok((
                memory.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(impossible_error)?,
                buffer,
            ))
        } else {
            error!(EINVAL)
        }
    }

    /// Adds a `BufferInfo` for the given `magma_buffer_t`, associated with the specified
    /// connection.  Note, the hashmaps key type is magma_device_t and the values we use for this
    /// type are actually the magma buffer ids, because we don't want to expose object-pointers
    /// to userspace.
    fn add_buffer_info(
        &self,
        connection_id: u64,
        connection: Arc<MagmaConnection>,
        buffer: magma_buffer_t,
        buffer_id: magma_buffer_id_t,
        buffer_info: BufferInfo,
    ) {
        let arc_buffer = Arc::new(MagmaBuffer { connection, handle: buffer });
        self.connections
            .lock()
            .get_mut(&connection_id)
            .map(|connection_info| connection_info.buffer_map.insert(buffer_id, buffer_info));
        self.buffers.lock().insert(buffer_id, arc_buffer);
    }

    fn get_device(&self, device_id: u64) -> Result<Arc<MagmaDevice>, Errno> {
        Ok(self.devices.lock().get(&device_id).ok_or_else(|| errno!(EINVAL))?.clone())
    }

    fn get_connection(&self, connection: u64) -> Result<Arc<MagmaConnection>, Errno> {
        Ok(self
            .connections
            .lock()
            .get(&connection)
            .ok_or_else(|| errno!(EINVAL))?
            .connection
            .clone())
    }

    fn get_buffer(&self, buffer: magma_buffer_t) -> Result<Arc<MagmaBuffer>, Errno> {
        Ok(self.buffers.lock().get(&buffer).ok_or_else(|| errno!(EINVAL))?.clone())
    }

    fn get_semaphore(&self, semaphore: magma_semaphore_t) -> Result<Arc<MagmaSemaphore>, i32> {
        Ok(self.semaphores.lock().get(&semaphore).ok_or(MAGMA_STATUS_INVALID_ARGS)?.clone())
    }

    fn import_semaphore2(
        &self,
        current_task: &CurrentTask,
        control: &virtio_magma_connection_import_semaphore2_ctrl_t,
        response: &mut virtio_magma_connection_import_semaphore2_resp_t,
    ) {
        let mut status: i32 = MAGMA_STATUS_OK;

        let fd = FdNumber::from_raw(control.semaphore_handle as i32);
        let mut result_semaphore_id = 0;

        if let (Ok(connection), Ok(file)) =
            (self.get_connection(control.connection), current_task.files().get(fd))
        {
            let mut handles: Vec<magma_semaphore_t> = vec![];
            let mut ids: Vec<magma_semaphore_id_t> = vec![];

            if let Some(sync_file) = file.downcast_file::<SyncFile>() {
                for sync_point in &sync_file.fence.sync_points {
                    if let Ok(counter) =
                        sync_point.counter.duplicate_handle(zx::Rights::SAME_RIGHTS)
                    {
                        if control.flags & MAGMA_IMPORT_SEMAPHORE_ONE_SHOT == 0 {
                            // For most non-test cases, one shot should be specified.
                            log_warn!(
                                "Importing magma semaphore without MAGMA_IMPORT_SEMAPHORE_ONE_SHOT"
                            );
                        }
                        let semaphore;
                        let semaphore_id;
                        (status, semaphore, semaphore_id) =
                            import_semaphore2(&connection, counter, control.flags);
                        if status != MAGMA_STATUS_OK {
                            break;
                        }
                        handles.push(semaphore as magma_semaphore_t);
                        ids.push(semaphore_id as magma_semaphore_id_t);
                    } else {
                        status = MAGMA_STATUS_MEMORY_ERROR;
                        break;
                    }
                }
            } else {
                status = MAGMA_STATUS_INVALID_ARGS;
            }

            if status == MAGMA_STATUS_OK {
                result_semaphore_id = self.semaphore_id_generator.next();

                self.semaphores.lock().insert(
                    result_semaphore_id,
                    Arc::new(MagmaSemaphore { connection, handles, ids }),
                );
            }
        } else {
            status = MAGMA_STATUS_INVALID_ARGS;
        }

        // Import is expected to close the file that was imported.
        let _ = current_task.files().close(fd);

        response.result_return = status as u64;
        response.semaphore_out = result_semaphore_id;
        response.id_out = result_semaphore_id;
        response.hdr.type_ =
            virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_IMPORT_SEMAPHORE2 as u32;
    }
}

impl FileOps for MagmaFile {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        _request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let user_addr = UserAddress::from(arg);
        let (command, command_type) = read_magma_command_and_type(current_task, user_addr)?;
        let response_address = UserAddress::from(command.response_address);

        match command_type {
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_DEVICE_IMPORT => {
                let (control, mut response): (
                    virtio_magma_device_import_ctrl_t,
                    virtio_magma_device_import_resp_t,
                ) = read_control_and_response(current_task, &command)?;

                let device = device_import(&self.supported_vendors, control)?;

                {
                    let mut device_id = 0u64;
                    let mut retry_count = 0u64;
                    let mut device_map = self.devices.lock();

                    while device_id == 0 || device_map.contains_key(&device_id) {
                        starnix_crypto::cprng_draw(device_id.as_mut_bytes());
                        retry_count += 1;
                        if retry_count % 10 == 0 {
                            log_warn!("Too many retries generating device id: {}", retry_count);
                        }
                    }

                    device_map.insert(device_id, Arc::new(device));
                    response.device_out = device_id;
                }

                response.hdr.type_ = virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_DEVICE_IMPORT as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_DEVICE_CREATE_CONNECTION => {
                let (control, mut response): (
                    virtio_magma_device_create_connection_ctrl,
                    virtio_magma_device_create_connection_resp_t,
                ) = read_control_and_response(current_task, &command)?;

                let device = self.get_device(control.device)?;

                if let Ok(connection) = create_connection(device.handle) {
                    let mut connection_id = 0u64;
                    let mut retry_count = 0u64;
                    let mut connection_map = self.connections.lock();

                    while connection_id == 0 || connection_map.contains_key(&connection_id) {
                        starnix_crypto::cprng_draw(connection_id.as_mut_bytes());
                        retry_count += 1;
                        if retry_count % 10 == 0 {
                            log_warn!("Too many retries generating connection id: {}", retry_count);
                        }
                    }

                    connection_map.insert(connection_id, ConnectionInfo::new(Arc::new(connection)));
                    response.connection_out = connection_id;
                    response.result_return = MAGMA_STATUS_OK as u64;
                } else {
                    response.result_return = MAGMA_STATUS_MEMORY_ERROR as u64;
                }
                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_DEVICE_CREATE_CONNECTION as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_RELEASE => {
                let (control, mut response): (
                    virtio_magma_connection_release_ctrl_t,
                    virtio_magma_connection_release_resp_t,
                ) = read_control_and_response(current_task, &command)?;

                let connection_id = control.connection;
                // Dropping the MagmaConnection will call magma_connection_release via FFI.
                self.connections.lock().remove(&connection_id);

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_RELEASE as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_DEVICE_RELEASE => {
                let (control, mut response): (
                    virtio_magma_device_release_ctrl_t,
                    virtio_magma_device_release_resp_t,
                ) = read_control_and_response(current_task, &command)?;

                let device_id = control.device;
                // Dropping the MagmaDevice will call magma_device_release via FFI.
                self.devices.lock().remove(&device_id);
                response.hdr.type_ = virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_DEVICE_RELEASE as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_FLUSH => {
                let (control, mut response): (
                    virtio_magma_connection_flush_ctrl_t,
                    virtio_magma_connection_flush_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;

                flush(control, &mut response, &connection);

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_READ_NOTIFICATION_CHANNEL => {
                let (control, mut response): (
                    virtio_magma_connection_read_notification_channel_ctrl_t,
                    virtio_magma_connection_read_notification_channel_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;

                read_notification_channel(current_task, control, &mut response, &connection)?;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_GET_HANDLE => {
                let (control, mut response): (
                    virtio_magma_buffer_get_handle_ctrl_t,
                    virtio_magma_buffer_get_handle_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let buffer = self.get_buffer(control.buffer)?;

                get_buffer_handle(current_task, control, &mut response, &buffer)?;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_RELEASE_BUFFER => {
                let (control, mut response): (
                    virtio_magma_connection_release_buffer_ctrl_t,
                    virtio_magma_connection_release_buffer_resp_t,
                ) = read_control_and_response(current_task, &command)?;

                if let Some(buffers) = self.connections.lock().get_mut(&{ control.connection }) {
                    match buffers.buffer_map.remove(&{ control.buffer }) {
                        Some(_) => (),
                        _ => {
                            log_error!("Calling magma_release_buffer with an invalid buffer.");
                        }
                    };
                }
                self.buffers.lock().remove(&{ control.buffer });

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_RELEASE_BUFFER as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_EXPORT => {
                let (control, mut response): (
                    virtio_magma_buffer_export_ctrl_t,
                    virtio_magma_buffer_export_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let buffer = self.get_buffer(control.buffer)?;

                export_buffer(
                    current_task,
                    control,
                    &mut response,
                    &buffer,
                    &self.connections.lock(),
                )?;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_IMPORT_BUFFER => {
                let (control, mut response): (
                    virtio_magma_connection_import_buffer_ctrl_t,
                    virtio_magma_connection_import_buffer_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection_id = control.connection;
                let connection = self.get_connection(connection_id)?;

                let buffer_fd = FdNumber::from_raw(control.buffer_handle as i32);
                let (memory, buffer) =
                    MagmaFile::get_memory_and_magma_buffer(current_task, buffer_fd)?;
                let vmo = memory.into_vmo().ok_or_else(|| errno!(EINVAL))?;

                let mut buffer_out = magma_buffer_t::default();
                let mut size_out = 0u64;
                let mut id_out = magma_buffer_id_t::default();
                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_connection_import_buffer(
                            connection.handle,
                            vmo.into_raw(),
                            &mut size_out,
                            &mut buffer_out,
                            &mut id_out,
                        ) as u64
                    }
                };

                // Store the information for the newly imported buffer.
                self.add_buffer_info(connection_id, connection, buffer_out, id_out, buffer);
                // Import is expected to close the file that was imported.
                let _ = current_task.files().close(buffer_fd);

                response.buffer_out = id_out;
                response.id_out = id_out;
                response.size_out = size_out;
                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_IMPORT_BUFFER as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_GET_NOTIFICATION_CHANNEL_HANDLE => {
                let (control, mut response): (
                    virtio_magma_connection_get_notification_channel_handle_ctrl_t,
                    virtio_magma_connection_get_notification_channel_handle_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;

                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_connection_get_notification_channel_handle(connection.handle)
                    }
                };

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_GET_NOTIFICATION_CHANNEL_HANDLE as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_CREATE_CONTEXT => {
                let (control, mut response): (
                    virtio_magma_connection_create_context_ctrl_t,
                    virtio_magma_connection_create_context_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;

                let mut context_id_out = 0;
                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_connection_create_context(connection.handle, &mut context_id_out)
                            as u64
                    }
                };
                response.context_id_out = context_id_out as u64;

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_CREATE_CONTEXT as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_CREATE_CONTEXT2 => {
                let (control, mut response): (
                    virtio_magma_connection_create_context2_ctrl_t,
                    virtio_magma_connection_create_context2_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;

                // TODO(b/402461734) - remove this workaround when trusted clients are supported.
                let priority = if control.priority > MAGMA_PRIORITY_MEDIUM {
                    MAGMA_PRIORITY_MEDIUM
                } else {
                    control.priority
                };

                let mut context_id_out = 0;
                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_connection_create_context2(
                            connection.handle,
                            priority,
                            &mut context_id_out,
                        ) as u64
                    }
                };
                response.context_id_out = context_id_out as u64;

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_CREATE_CONTEXT2 as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_RELEASE_CONTEXT => {
                let (control, mut response): (
                    virtio_magma_connection_release_context_ctrl_t,
                    virtio_magma_connection_release_context_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;

                #[allow(clippy::undocumented_unsafe_blocks)]
                unsafe {
                    magma_connection_release_context(connection.handle, control.context_id);
                }

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_RELEASE_CONTEXT as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_CREATE_BUFFER => {
                let (control, mut response): (
                    virtio_magma_connection_create_buffer_ctrl_t,
                    virtio_magma_connection_create_buffer_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection_id = control.connection;
                let connection = self.get_connection(connection_id)?;

                let mut size_out = 0;
                let mut buffer_out = 0;
                let mut id_out = 0;
                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_connection_create_buffer(
                            connection.handle,
                            control.size,
                            &mut size_out,
                            &mut buffer_out,
                            &mut id_out,
                        ) as u64
                    }
                };
                response.size_out = size_out;
                response.buffer_out = id_out;
                response.id_out = id_out;

                self.add_buffer_info(
                    connection_id,
                    connection,
                    buffer_out,
                    id_out,
                    BufferInfo::Default,
                );

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_CREATE_BUFFER as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_CREATE_SEMAPHORE => {
                let (control, mut response): (
                    virtio_magma_connection_create_semaphore_ctrl_t,
                    virtio_magma_connection_create_semaphore_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;
                let status: i32;
                let mut result_semaphore_id = 0;

                // Use counter semaphores for compatibility with sync files, which need timestamps.
                let counter = zx::Counter::create();
                let flags: u64 = 0;
                let semaphore;
                let semaphore_id;
                (status, semaphore, semaphore_id) = import_semaphore2(&connection, counter, flags);
                if status == MAGMA_STATUS_OK {
                    result_semaphore_id = self.semaphore_id_generator.next();

                    self.semaphores.lock().insert(
                        result_semaphore_id,
                        Arc::new(MagmaSemaphore {
                            connection,
                            handles: vec![semaphore; 1],
                            ids: vec![semaphore_id; 1],
                        }),
                    );
                }
                response.result_return = status as u64;
                response.semaphore_out = result_semaphore_id;
                response.id_out = result_semaphore_id;
                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_CREATE_SEMAPHORE as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_GET_ERROR => {
                let (control, mut response): (
                    virtio_magma_connection_get_error_ctrl_t,
                    virtio_magma_connection_get_error_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;

                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_connection_get_error(connection.handle) as u64
                    }
                };

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_GET_ERROR as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_IMPORT_SEMAPHORE2 => {
                let (control, mut response): (
                    virtio_magma_connection_import_semaphore2_ctrl_t,
                    virtio_magma_connection_import_semaphore2_resp_t,
                ) = read_control_and_response(current_task, &command)?;

                self.import_semaphore2(current_task, &control, &mut response);

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_RELEASE_SEMAPHORE => {
                let (control, mut response): (
                    virtio_magma_connection_release_semaphore_ctrl_t,
                    virtio_magma_connection_release_semaphore_resp_t,
                ) = read_control_and_response(current_task, &command)?;

                self.semaphores.lock().remove(&{ control.semaphore });

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_RELEASE_SEMAPHORE as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_SEMAPHORE_EXPORT => {
                let (control, mut response): (
                    virtio_magma_semaphore_export_ctrl_t,
                    virtio_magma_semaphore_export_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let mut status: i32 = MAGMA_STATUS_OK;

                let mut sync_points: Vec<SyncPoint> = vec![];
                let mut sync_file_fd: i32 = -1;

                match self.get_semaphore(control.semaphore) {
                    Ok(semaphore) => {
                        for handle in &semaphore.handles {
                            let mut raw_handle = 0;
                            status = {
                                #[allow(clippy::undocumented_unsafe_blocks)]
                                unsafe {
                                    magma_semaphore_export(*handle, &mut raw_handle)
                                }
                            };
                            if status != MAGMA_STATUS_OK {
                                break;
                            }
                            let handle = {
                                #[allow(clippy::undocumented_unsafe_blocks)]
                                unsafe {
                                    zx::NullableHandle::from_raw(raw_handle)
                                }
                            };

                            sync_points.push(SyncPoint::new(Timeline::Magma, handle.into()));
                        }
                    }
                    Err(s) => status = s,
                }

                if status == MAGMA_STATUS_OK {
                    let sync_file_name: &[u8; 32] =
                        b"magma semaphore\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
                    let file = SyncFile::new_file(
                        current_task,
                        *sync_file_name,
                        SyncFence { sync_points },
                    )?;

                    let fd = current_task.add_file(file, FdFlags::empty())?;
                    sync_file_fd = fd.raw();
                }

                response.result_return = status as u64;
                response.semaphore_handle_out = sync_file_fd as u64;
                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_SEMAPHORE_EXPORT as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_SEMAPHORE_RESET => {
                let (control, mut response): (
                    virtio_magma_semaphore_reset_ctrl_t,
                    virtio_magma_semaphore_reset_resp_t,
                ) = read_control_and_response(current_task, &command)?;

                if let Ok(semaphore) = self.get_semaphore(control.semaphore) {
                    for handle in &semaphore.handles {
                        #[allow(clippy::undocumented_unsafe_blocks)]
                        unsafe {
                            magma_semaphore_reset(*handle);
                        }
                    }
                }
                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_SEMAPHORE_RESET as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_SEMAPHORE_SIGNAL => {
                let (control, mut response): (
                    virtio_magma_semaphore_signal_ctrl_t,
                    virtio_magma_semaphore_signal_resp_t,
                ) = read_control_and_response(current_task, &command)?;

                if let Ok(semaphore) = self.get_semaphore(control.semaphore) {
                    for handle in &semaphore.handles {
                        #[allow(clippy::undocumented_unsafe_blocks)]
                        unsafe {
                            magma_semaphore_signal(*handle);
                        }
                    }
                }
                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_SEMAPHORE_SIGNAL as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_MAP_BUFFER => {
                let (control, mut response): (
                    virtio_magma_connection_map_buffer_ctrl_t,
                    virtio_magma_connection_map_buffer_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;
                let buffer = self.get_buffer(control.buffer)?;

                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_connection_map_buffer(
                            connection.handle,
                            control.hw_va,
                            buffer.handle,
                            control.offset,
                            control.length,
                            control.map_flags,
                        ) as u64
                    }
                };

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_MAP_BUFFER as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_POLL => {
                let (control, mut response): (virtio_magma_poll_ctrl_t, virtio_magma_poll_resp_t) =
                    read_control_and_response(current_task, &command)?;

                let num_items = control.count as usize / std::mem::size_of::<StarnixPollItem>();
                let items_ref = UserRef::<StarnixPollItem>::new(UserAddress::from(control.items));
                // Read the poll items as `StarnixPollItem`, since they contain a union. Also note
                // that the minimum length of the vector is 1, to always have a valid reference for
                // `magma_poll`.
                let starnix_items =
                    current_task.read_objects_to_vec(items_ref, std::cmp::max(num_items, 1))?;
                // Then convert each item "manually" into `magma_poll_item_t`.
                let mut magma_items: Vec<magma_poll_item_t> =
                    starnix_items.iter().map(|item| item.as_poll_item()).collect();

                // Expand semaphores.
                let mut status: i32 = MAGMA_STATUS_OK;
                let mut child_semaphore_items: Vec<magma_poll_item_t> = vec![];

                for i in 0..magma_items.len() {
                    magma_items[i].result = 0;

                    if magma_items[i].type_ == MAGMA_POLL_TYPE_SEMAPHORE
                        && magma_items[i].condition == MAGMA_POLL_CONDITION_SIGNALED
                    {
                        match self.get_semaphore(starnix_items[i].semaphore_or_handle) {
                            Ok(semaphore) => {
                                magma_items[i].condition = 0; // magma_poll must ignore this item

                                // Store the number of expanded semaphores to be signaled
                                let handles_ref = &semaphore.handles;
                                let child_count = handles_ref.len() as u32;
                                magma_items[i].unused = child_count;

                                for handle in handles_ref {
                                    child_semaphore_items.push(
                                        StarnixPollItem {
                                            semaphore_or_handle: *handle,
                                            type_: MAGMA_POLL_TYPE_SEMAPHORE,
                                            condition: MAGMA_POLL_CONDITION_SIGNALED,
                                            result: 0,
                                            // Points back to the parent item
                                            unused: i as u32,
                                        }
                                        .as_poll_item(),
                                    );
                                }
                            }
                            Err(s) => status = s,
                        }
                    }
                }

                if status == MAGMA_STATUS_OK {
                    magma_items.append(&mut child_semaphore_items);

                    let abs_timeout_ns = if control.timeout_ns == u64::MAX {
                        0
                    } else {
                        zx::MonotonicInstant::get().into_nanos() as u64 + control.timeout_ns
                    };

                    'outer: while status == MAGMA_STATUS_OK {
                        // Iterate from the end to process child semaphores first
                        for i in (0..magma_items.len()).rev() {
                            if i < num_items && magma_items[i].result > 0 {
                                // A handle or parent semaphore is signaled, we're done
                                break 'outer;
                            } else if magma_items[i].result > 0 {
                                // A child semaphore is signaled
                                assert_eq!(magma_items[i].condition, MAGMA_POLL_CONDITION_SIGNALED);
                                let parent_index = magma_items[i].unused as usize;
                                assert_ne!(magma_items[parent_index].unused, 0);
                                magma_items[parent_index].unused -= 1;
                                if magma_items[parent_index].unused == 0 {
                                    // All children signaled, signal the parent
                                    magma_items[parent_index].result = magma_items[i].result;
                                }
                                // Don't poll on this child again
                                magma_items[i].condition = 0;
                            }
                        }

                        let current_time_ns = zx::MonotonicInstant::get().into_nanos() as u64;
                        let rel_timeout_ns = if abs_timeout_ns == 0 {
                            u64::MAX
                        } else if abs_timeout_ns > current_time_ns {
                            abs_timeout_ns - current_time_ns
                        } else {
                            0
                        };

                        // Force EINTR every second to allow signals to interrupt the wait.
                        // TODO(https://fxbug.dev/42080364): Only interrupt the wait when needed.
                        let capped_rel_timeout_ns = std::cmp::min(rel_timeout_ns, 1_000_000_000);

                        status = {
                            #[allow(clippy::undocumented_unsafe_blocks)]
                            unsafe {
                                magma_poll(
                                    &mut magma_items[0] as *mut magma_poll_item,
                                    magma_items.len() as u32,
                                    capped_rel_timeout_ns,
                                )
                            }
                        };
                        let current_time = zx::MonotonicInstant::get().into_nanos();

                        // Check if the wait timed out before the user-requested timeout.
                        if status == MAGMA_STATUS_TIMED_OUT
                            && (control.timeout_ns == u64::MAX
                                || (current_time as u64) < abs_timeout_ns)
                        {
                            if control.timeout_ns != u64::MAX {
                                // Update relative deadline.
                                let mut control = control;
                                control.timeout_ns = abs_timeout_ns - (current_time as u64);
                                let request_address = UserAddress::from(command.request_address);
                                let _ = current_task
                                    .write_object(UserRef::new(request_address), &control);
                            }
                            return error!(EINTR);
                        }
                    }
                }

                // Walk child items to restore modified parent items
                for i in num_items..magma_items.len() {
                    assert_eq!(magma_items[i].type_, MAGMA_POLL_TYPE_SEMAPHORE);
                    let parent_index = magma_items[i].unused as usize;
                    magma_items[parent_index].condition = MAGMA_POLL_CONDITION_SIGNALED;
                    magma_items[parent_index].unused = 0;
                }

                // Remove any child semaphores
                magma_items.truncate(num_items);

                // Convert the poll items back to a serializable version after the `magma_poll`
                // call.
                let starnix_items: Vec<StarnixPollItem> =
                    magma_items.iter().map(StarnixPollItem::new).collect();
                current_task.write_objects(items_ref, &starnix_items)?;

                response.result_return = status as u64;
                response.hdr.type_ = virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_POLL as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_EXECUTE_COMMAND => {
                let (control, mut response): (
                    virtio_magma_connection_execute_command_ctrl_t,
                    virtio_magma_connection_execute_command_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;

                let status = execute_command(current_task, control, &connection, |semaphore_id| {
                    self.get_semaphore(semaphore_id)
                })?;

                response.result_return = status as u64;
                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_EXECUTE_COMMAND as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_EXECUTE_INLINE_COMMANDS => {
                let (control, mut response): (
                    virtio_magma_connection_execute_inline_commands_ctrl_t,
                    virtio_magma_connection_execute_inline_commands_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;

                let status =
                    execute_inline_commands(current_task, control, &connection, |semaphore_id| {
                        self.get_semaphore(semaphore_id)
                    })?;

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_EXECUTE_INLINE_COMMANDS
                        as u32;
                response.result_return = status as u64;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_DEVICE_QUERY => {
                let (control, mut response): (
                    virtio_magma_device_query_ctrl_t,
                    virtio_magma_device_query_resp_t,
                ) = read_control_and_response(current_task, &command)?;

                let device = self.get_device(control.device)?;

                query(current_task, device.handle, control.id, &mut response)?;

                response.hdr.type_ = virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_DEVICE_QUERY as u32;
                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_UNMAP_BUFFER => {
                let (control, mut response): (
                    virtio_magma_connection_unmap_buffer_ctrl_t,
                    virtio_magma_connection_unmap_buffer_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;
                let buffer = self.get_buffer(control.buffer)?;

                #[allow(clippy::undocumented_unsafe_blocks)]
                unsafe {
                    magma_connection_unmap_buffer(connection.handle, control.hw_va, buffer.handle)
                };

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_UNMAP_BUFFER as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_CONNECTION_PERFORM_BUFFER_OP => {
                let (control, mut response): (
                    virtio_magma_connection_perform_buffer_op_ctrl_t,
                    virtio_magma_connection_perform_buffer_op_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let connection = self.get_connection(control.connection)?;
                let buffer = self.get_buffer(control.buffer)?;

                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_connection_perform_buffer_op(
                            connection.handle,
                            buffer.handle,
                            control.options,
                            control.start_offset,
                            control.length,
                        ) as u64
                    }
                };

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_CONNECTION_PERFORM_BUFFER_OP as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_GET_INFO => {
                let (control, mut response): (
                    virtio_magma_buffer_get_info_ctrl_t,
                    virtio_magma_buffer_get_info_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let buffer = self.get_buffer(control.buffer)?;

                let mut buffer_info = magma_buffer_info_t { committed_byte_count: 0, size: 0 };

                let status = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_buffer_get_info(buffer.handle, &mut buffer_info)
                    }
                };

                if status == MAGMA_STATUS_OK {
                    current_task.write_object(
                        UserRef::<magma_buffer_info_t>::new(UserAddress::from(control.info_out)),
                        &buffer_info,
                    )?;
                }

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_GET_INFO as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_SET_CACHE_POLICY => {
                let (control, mut response): (
                    virtio_magma_buffer_set_cache_policy_ctrl_t,
                    virtio_magma_buffer_set_cache_policy_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let buffer = self.get_buffer(control.buffer)?;

                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_buffer_set_cache_policy(
                            buffer.handle,
                            control.policy as magma_cache_policy_t,
                        ) as u64
                    }
                };

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_SET_CACHE_POLICY as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_GET_CACHE_POLICY => {
                let (control, mut response): (
                    virtio_magma_buffer_get_cache_policy_ctrl_t,
                    virtio_magma_buffer_get_cache_policy_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let buffer = self.get_buffer(control.buffer)?;

                let mut policy: magma_cache_policy_t = MAGMA_CACHE_POLICY_CACHED;

                let status = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_buffer_get_cache_policy(buffer.handle, &mut policy)
                    }
                };

                if status == MAGMA_STATUS_OK {
                    response.cache_policy_out = policy as u64;
                }
                response.result_return = status as u64;
                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_GET_CACHE_POLICY as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_CLEAN_CACHE => {
                let (control, mut response): (
                    virtio_magma_buffer_clean_cache_ctrl_t,
                    virtio_magma_buffer_clean_cache_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let buffer = self.get_buffer(control.buffer)?;

                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        magma_buffer_clean_cache(
                            buffer.handle,
                            control.offset,
                            control.size,
                            control.operation as magma_cache_operation_t,
                        ) as u64
                    }
                };

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_CLEAN_CACHE as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            virtio_magma_ctrl_type_VIRTIO_MAGMA_CMD_BUFFER_SET_NAME => {
                let (control, mut response): (
                    virtio_magma_buffer_set_name_ctrl_t,
                    virtio_magma_buffer_set_name_resp_t,
                ) = read_control_and_response(current_task, &command)?;
                let buffer = self.get_buffer(control.buffer)?;

                let wrapper_ref = UserRef::<virtmagma_buffer_set_name_wrapper>::new(
                    UserAddress::from(control.name),
                );
                let wrapper = current_task.read_object(wrapper_ref)?;

                let name = current_task.read_buffer(&UserBuffer {
                    address: UserAddress::from(wrapper.name_address),
                    length: wrapper.name_size as usize, // name_size includes null terminate byte
                })?;

                response.result_return = {
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        let name_ptr = &name[0] as *const u8 as *const std::os::raw::c_char;
                        magma_buffer_set_name(buffer.handle, name_ptr) as u64
                    }
                };

                response.hdr.type_ =
                    virtio_magma_ctrl_type_VIRTIO_MAGMA_RESP_BUFFER_SET_NAME as u32;

                current_task.write_object(UserRef::new(response_address), &response)
            }
            t => {
                track_stub!(TODO("https://fxbug.dev/322874166"), "virtio magma ioctl", t);
                error!(ENOSYS)
            }
        }?;

        Ok(SUCCESS)
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);
        error!(EINVAL)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);
        error!(EINVAL)
    }
}
