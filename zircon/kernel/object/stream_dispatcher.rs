// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use bitrs::layout;
use fbl::{Canary, RefPtr};
use ksync::{KMutex, RawCriticalMutex, RawMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_OBJ_TYPE_STREAM, ZX_RIGHT_DUPLICATE, ZX_RIGHT_GET_PROPERTY, ZX_RIGHT_INSPECT, ZX_RIGHT_READ,
    ZX_RIGHT_SET_PROPERTY, ZX_RIGHT_SIGNAL, ZX_RIGHT_TRANSFER, ZX_RIGHT_WAIT, ZX_RIGHT_WRITE,
    zx_info_stream_t, zx_off_t, zx_rights_t, zx_stream_seek_origin_t,
};

use crate::user_copy::{UserInIovec, UserOutIovec};
use crate::vm::stream_size_manager::{Operation as StreamSizeManagerOperation, StreamSizeManager};
use crate::vm::vm_object_paged::VmObjectPaged;

use object_constants_rs as object_constants;

use super::KernelHandle;
use super::stream_dispatcher_ffi::cpp_stream_dispatcher_create;
use super::vm_object_dispatcher::VmObjectDispatcher;

layout!({
    /// Options and modes for a stream dispatcher.
    #[repr(transparent)]
    pub struct StreamOptions(u32);
    {
        /// Read mode is enabled.
        let read @ 0;
        /// Write mode is enabled.
        let write @ 1;
        /// Append mode is enabled.
        let append @ 2;
        /// Resizing the VMO is permitted.
        let can_resize_vmo @ 3;
    }
});

const _: () = {
    assert!(StreamOptions::READ_MASK == zx_types::ZX_STREAM_MODE_READ);
    assert!(StreamOptions::WRITE_MASK == zx_types::ZX_STREAM_MODE_WRITE);
    assert!(StreamOptions::APPEND_MASK == zx_types::ZX_STREAM_MODE_APPEND);
};

impl StreamOptions {
    /// Mask of valid stream mode bits for stream creation (`READ`, `WRITE`, `APPEND`).
    pub const MODE_MASK: u32 = Self::READ_MASK | Self::WRITE_MASK | Self::APPEND_MASK;

    /// Converts the stream mode flags into raw `zx_types::ZX_STREAM_MODE_*` bits.
    pub const fn to_stream_modes(self) -> u32 {
        self.bits() & Self::MODE_MASK
    }
}

const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_TRANSFER
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_GET_PROPERTY
    | ZX_RIGHT_SET_PROPERTY
    | ZX_RIGHT_SIGNAL;

zr::static_assert_size_and_align!(
    StreamDispatcherState,
    object_constants::kStreamDispatcherStateSize,
    object_constants::kStreamDispatcherStateAlign,
);

define_kcounter!(DISPATCHER_STREAM_CREATE_COUNT, "dispatcher.stream.create", Sum);
define_kcounter!(DISPATCHER_STREAM_DESTROY_COUNT, "dispatcher.stream.destroy", Sum);

/// Internal state of a `StreamDispatcher`.
///
/// Manages synchronization, stream options, seek position, underlying VMO, and the
/// `StreamSizeManager` instance coordinating stream size mutations.
#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct StreamDispatcherState {
    canary: Canary<{ fbl::magic(b"STRE") }>,

    #[guarded_by(lock)]
    options: StreamOptions,

    #[mutex]
    lock: KMutex<RawCriticalMutex>,

    #[guarded_by(seek_lock)]
    seek: u64,

    #[mutex(flags = lockdep::LOCK_FLAGS_ACTIVE_LIST_DISABLED)]
    seek_lock: KMutex<RawMutex>,

    vmo: RefPtr<VmObjectPaged>,

    stream_size_mgr: RefPtr<StreamSizeManager>,
}

impl StreamDispatcherState {
    /// Initializes a new `StreamDispatcherState` with the given mode options, initial seek
    /// position, underlying VMO, and `StreamSizeManager`.
    pub fn init(
        options: StreamOptions,
        seek: zx_off_t,
        vmo: RefPtr<VmObjectPaged>,
        stream_size_mgr: RefPtr<StreamSizeManager>,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        DISPATCHER_STREAM_CREATE_COUNT.add(1);
        pin_init!(Self {
            canary: Canary::new(),
            options: options.into(),
            lock <- KMutex::init(),
            seek: seek.into(),
            seek_lock <- KMutex::init(),
            vmo,
            stream_size_mgr,
        })
    }
}

#[pinned_drop]
impl PinnedDrop for StreamDispatcherState {
    fn drop(self: core::pin::Pin<&mut Self>) {
        self.canary.assert();
        DISPATCHER_STREAM_DESTROY_COUNT.add(1);
    }
}

crate::object::dispatcher::impl_dispatcher_facade_with_state!(
    pub struct StreamDispatcher,
    StreamDispatcherState,
    ZX_OBJ_TYPE_STREAM,
    object_constants::kStreamDispatcherStateOffset
);

extern "C" fn write_progress_cb(cookie: *mut core::ffi::c_void, write_offset: u64, len: usize) {
    let op = unsafe { &*cookie.cast::<StreamSizeManagerOperation>() };
    op.update_stream_size_from_progress(write_offset + len as u64);
}

impl StreamDispatcher {
    /// Parses and validates syscall flag options for stream creation.
    pub fn parse_create_syscall_flags(
        options: u32,
    ) -> Result<(StreamOptions, zx_rights_t), Status> {
        if (options & !StreamOptions::MODE_MASK) != 0 {
            return Err(Status::INVALID_ARGS);
        }

        let stream_options = StreamOptions::from(options);
        let mut required_vmo_rights = 0;
        if stream_options.read() {
            required_vmo_rights |= ZX_RIGHT_READ;
        }
        if stream_options.write() || stream_options.append() {
            required_vmo_rights |= ZX_RIGHT_WRITE;
        }
        Ok((stream_options, required_vmo_rights))
    }

    /// Creates a new StreamDispatcher via C++ and returns its kernel handle and rights.
    pub fn create(
        options: StreamOptions,
        vmo_dispatcher: &VmObjectDispatcher,
        seek: zx_off_t,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        let mut rights = DEFAULT_RIGHTS;
        if options.read() {
            rights |= ZX_RIGHT_READ;
        }
        if options.write() {
            rights |= ZX_RIGHT_WRITE;
        }
        // SAFETY: `vmo_dispatcher` is a valid `VmObjectDispatcher`, and
        // `cpp_stream_dispatcher_create` initializes `handle` on success.
        let handle = unsafe {
            KernelHandle::create(|out| {
                cpp_stream_dispatcher_create(options.bits(), vmo_dispatcher as *const _, seek, out)
            })
        }?;
        Ok((handle, rights))
    }

    fn create_write_op(
        &self,
        total_capacity: usize,
        offset: zx_off_t,
        op: &StreamSizeManagerOperation,
    ) -> Result<(u64, Option<u64>), Status> {
        let (length, prev_stream_size) = {
            ksync::lock!(let mut stream_size_guard = ksync::aliased_lock(self.state().stream_size_mgr.lock(), op.lock()));

            let requested_stream_size =
                offset.checked_add(total_capacity as u64).ok_or(Status::FILE_BIG)?;

            let prev_stream_size = self.state().stream_size_mgr.begin_write_locked(
                &mut stream_size_guard.as_mut().inner_guard(),
                requested_stream_size,
                op,
            );

            let (_ssm_token, op_token) = stream_size_guard.as_mut().tokens_mut();

            let vmo_size = self.state().vmo.size();
            if vmo_size <= offset {
                // We can't even perform a partial write
                op.cancel_locked(op_token);
                return Err(Status::OUT_OF_RANGE);
            }

            // Allow writing up to the minimum of the VMO size and requested stream size, since we
            // want to write at most the requested size but don't want to write beyond the VMO size.
            let target_stream_size = core::cmp::min(vmo_size, requested_stream_size);
            let length = target_stream_size - offset;

            if target_stream_size != requested_stream_size {
                op.shrink_size_locked(op_token, target_stream_size);
            }

            (length, prev_stream_size)
        };

        // Zero content between the previous stream size and the start of the write.
        if let Some(prev) = prev_stream_size
            && prev < offset
        {
            let status = self.state().vmo.zero_range(prev, offset - prev);
            if let Err(err) = status {
                ksync::lock!(let mut stream_size_guard = op.lock().lock());
                op.cancel_locked(stream_size_guard.as_mut().token_mut());
                return Err(err);
            }
        }

        Ok((length, prev_stream_size))
    }

    /// Reads data from the stream using an output iovec array.
    pub fn read_vector(&self, user_data: UserOutIovec) -> Result<usize, Status> {
        self.state().canary.assert();

        let total_capacity = user_data.get_total_capacity()?;
        if total_capacity == 0 {
            return Ok(0);
        }

        pin_init::stack_pin_init!(let op = StreamSizeManagerOperation::init(&self.state().stream_size_mgr));

        ksync::lock!(let mut seek_guard = self.state().lock_seek_lock());
        let (offset, length) = {
            ksync::lock!(let mut stream_size_guard = ksync::aliased_lock(self.state().stream_size_mgr.lock(), op.lock()));

            let seek = *seek_guard.fields().seek;
            let size_limit = self.state().stream_size_mgr.begin_read_locked(
                stream_size_guard.as_mut().token_mut(),
                seek.saturating_add(total_capacity as u64),
                &op,
            );
            if size_limit <= seek {
                // Return |ZX_OK| since there is nothing to be read.
                let (_, op_token) = stream_size_guard.as_mut().tokens_mut();
                op.cancel_locked(op_token);
                return Ok(0);
            }

            (seek, (size_limit - seek) as usize)
        };

        let result = self.state().vmo.read_user_vector(user_data, offset, length);
        let read_bytes = result.as_ref().copied().unwrap_or(0);
        *seek_guard.as_mut().fields_mut().seek += read_bytes as u64;

        // Reacquire the lock to commit the operation.
        ksync::lock!(let mut stream_size_guard = op.lock().lock());
        op.commit_locked(stream_size_guard.as_mut().token_mut());

        if read_bytes > 0 { Ok(read_bytes) } else { result }
    }

    /// Reads data from the stream at a specific offset.
    pub fn read_vector_at(
        &self,
        user_data: UserOutIovec,
        offset: zx_off_t,
    ) -> Result<usize, Status> {
        self.state().canary.assert();

        let total_capacity = user_data.get_total_capacity()?;
        if total_capacity == 0 {
            return Ok(0);
        }

        pin_init::stack_pin_init!(let op = StreamSizeManagerOperation::init(&self.state().stream_size_mgr));

        let length = {
            ksync::lock!(let mut stream_size_guard = ksync::aliased_lock(self.state().stream_size_mgr.lock(), op.lock()));

            let size_limit = self.state().stream_size_mgr.begin_read_locked(
                stream_size_guard.as_mut().token_mut(),
                offset.saturating_add(total_capacity as u64),
                &op,
            );
            if size_limit <= offset {
                // Return |ZX_OK| since there is nothing to be read.
                let (_, op_token) = stream_size_guard.as_mut().tokens_mut();
                op.cancel_locked(op_token);
                return Ok(0);
            }

            (size_limit - offset) as usize
        };

        let result = self.state().vmo.read_user_vector(user_data, offset, length);
        let read_bytes = result.as_ref().copied().unwrap_or(0);

        // Reacquire the lock to commit the operation.
        ksync::lock!(let mut stream_size_guard = op.lock().lock());
        op.commit_locked(stream_size_guard.as_mut().token_mut());

        if read_bytes > 0 { Ok(read_bytes) } else { result }
    }

    /// Writes data to the stream using an input iovec array.
    pub fn write_vector(&self, user_data: UserInIovec) -> Result<usize, Status> {
        self.state().canary.assert();

        if self.is_in_append_mode() {
            return self.append_vector(user_data);
        }

        let total_capacity = user_data.get_total_capacity()?;
        // Return early if writing zero bytes since there's nothing to do.
        if total_capacity == 0 {
            return Ok(0);
        }

        pin_init::stack_pin_init!(let op = StreamSizeManagerOperation::init(&self.state().stream_size_mgr));

        ksync::lock!(let mut seek_guard = self.state().lock_seek_lock());
        let seek = *seek_guard.fields().seek;

        let (length, prev_stream_size) = self.create_write_op(total_capacity, seek, &op)?;

        let result = if let Some(prev) = prev_stream_size {
            self.state().vmo.write_user_vector_progress(
                user_data,
                seek,
                length as usize,
                prev,
                write_progress_cb,
                core::ptr::from_ref(&*op).cast_mut().cast(),
            )
        } else {
            self.state().vmo.write_user_vector(user_data, seek, length as usize)
        };
        let written = result.as_ref().copied().unwrap_or(0);

        // Reacquire the lock to potentially shrink and commit the operation.
        ksync::lock!(let mut stream_size_guard = op.lock().lock());

        // Update the stream size operation if operation was partially successful.
        if (written as u64) < length {
            if written == 0 {
                // Do not commit the operation if nothing was written.
                op.cancel_locked(stream_size_guard.as_mut().token_mut());
                return result;
            } else {
                op.shrink_size_locked(
                    stream_size_guard.as_mut().token_mut(),
                    seek + written as u64,
                );
            }
        }

        *seek_guard.as_mut().fields_mut().seek += written as u64;

        op.commit_locked(stream_size_guard.as_mut().token_mut());
        if written > 0 { Ok(written) } else { result }
    }

    /// Writes data to the stream at a specific offset.
    pub fn write_vector_at(
        &self,
        user_data: UserInIovec,
        offset: zx_off_t,
    ) -> Result<usize, Status> {
        self.state().canary.assert();

        let total_capacity = user_data.get_total_capacity()?;
        // Return early if writing zero bytes
        if total_capacity == 0 {
            return Ok(0);
        }

        pin_init::stack_pin_init!(let op = StreamSizeManagerOperation::init(&self.state().stream_size_mgr));

        let (length, prev_stream_size) = self.create_write_op(total_capacity, offset, &op)?;

        let result = if let Some(prev) = prev_stream_size {
            self.state().vmo.write_user_vector_progress(
                user_data,
                offset,
                length as usize,
                prev,
                write_progress_cb,
                core::ptr::from_ref(&*op).cast_mut().cast(),
            )
        } else {
            self.state().vmo.write_user_vector(user_data, offset, length as usize)
        };
        let written = result.as_ref().copied().unwrap_or(0);

        // Reacquire the lock to potentially shrink and commit the operation.
        ksync::lock!(let mut stream_size_guard = op.lock().lock());

        // Update the stream size operation if operation was partially successful.
        if (written as u64) < length {
            if written == 0 {
                // Do not commit the operation if nothing was written.
                op.cancel_locked(stream_size_guard.as_mut().token_mut());
                return result;
            } else {
                op.shrink_size_locked(
                    stream_size_guard.as_mut().token_mut(),
                    offset + written as u64,
                );
            }
        }

        op.commit_locked(stream_size_guard.as_mut().token_mut());
        if written > 0 { Ok(written) } else { result }
    }

    /// Appends data to the end of the stream.
    pub fn append_vector(&self, user_data: UserInIovec) -> Result<usize, Status> {
        self.state().canary.assert();

        let total_capacity = user_data.get_total_capacity()?;
        // Return early if writing zero bytes since there's nothing to do.
        if total_capacity == 0 {
            return Ok(0);
        }

        let length: usize;
        let offset: u64;
        pin_init::stack_pin_init!(let op = StreamSizeManagerOperation::init(&self.state().stream_size_mgr));
        ksync::lock!(let mut seek_guard = self.state().lock_seek_lock());

        // This section expands the VMO if necessary and bumps the |seek_| pointer if successful.
        {
            ksync::lock!(let mut stream_size_guard = ksync::aliased_lock(self.state().stream_size_mgr.lock(), op.lock()));

            self.state().stream_size_mgr.begin_append_locked(
                &mut stream_size_guard.as_mut().inner_guard(),
                total_capacity,
                &op,
            )?;

            let (_ssm_token, op_token) = stream_size_guard.as_mut().tokens_mut();

            let new_stream_size = op.get_size_locked(op_token);
            offset = new_stream_size - total_capacity as u64;

            let vmo_size = self.state().vmo.size();
            if vmo_size <= offset {
                // We can't even perform a partial write
                op.cancel_locked(op_token);
                return Err(Status::OUT_OF_RANGE);
            }

            if vmo_size < new_stream_size {
                // Unable to expand to requested size but able to perform a partial write.
                op.shrink_size_locked(op_token, vmo_size);
            }

            length = (core::cmp::min(vmo_size, new_stream_size) - offset) as usize;
        }

        let result = self.state().vmo.write_user_vector_progress(
            user_data,
            offset,
            length,
            offset,
            write_progress_cb,
            core::ptr::from_ref(&*op).cast_mut().cast(),
        );
        let written = result.as_ref().copied().unwrap_or(0);
        *seek_guard.as_mut().fields_mut().seek = offset + written as u64;

        // Reacquire the lock to potentially shrink and commit the operation.
        ksync::lock!(let mut stream_size_guard = ksync::aliased_lock(self.state().stream_size_mgr.lock(), op.lock()));

        let (_ssm_token, op_token) = stream_size_guard.as_mut().tokens_mut();

        // Update the stream size operation if operation was partially successful.
        if written < length {
            if written == 0 {
                // Do not commit the operation if nothing was written.
                op.cancel_locked(op_token);
                return result;
            } else {
                op.shrink_size_locked(op_token, offset + written as u64);
            }
        }

        op.commit_locked(op_token);
        if written > 0 { Ok(written) } else { result }
    }

    /// Sets the seek offset of the stream.
    pub fn seek(&self, whence: zx_stream_seek_origin_t, offset: i64) -> Result<zx_off_t, Status> {
        self.state().canary.assert();
        ksync::lock!(let mut guard = self.state().lock_seek_lock());
        let current_seek = *guard.fields().seek;
        let target = match whence {
            zx_types::ZX_STREAM_SEEK_ORIGIN_START => {
                if offset < 0 {
                    return Err(Status::INVALID_ARGS);
                }
                offset as u64
            }
            zx_types::ZX_STREAM_SEEK_ORIGIN_CURRENT => {
                let Some(value) = current_seek.checked_add_signed(offset) else {
                    return Err(Status::INVALID_ARGS);
                };
                value
            }
            zx_types::ZX_STREAM_SEEK_ORIGIN_END => {
                let stream_size = self.state().stream_size_mgr.get_stream_size();
                let Some(value) = stream_size.checked_add_signed(offset) else {
                    return Err(Status::INVALID_ARGS);
                };
                value
            }
            _ => return Err(Status::INVALID_ARGS),
        };
        *guard.as_mut().fields_mut().seek = target;
        Ok(target)
    }

    /// Sets whether the stream is in append mode.
    pub fn set_append_mode(&self, value: bool) {
        ksync::lock!(let mut guard = self.state().lock_lock());
        guard.as_mut().fields_mut().options.set_append(value);
    }

    /// Returns whether the stream is in append mode.
    pub fn is_in_append_mode(&self) -> bool {
        ksync::lock!(let guard = self.state().lock_lock());
        guard.fields().options.append()
    }

    /// Returns whether the stream can resize the underlying VMO.
    pub fn can_resize_vmo(&self) -> bool {
        ksync::lock!(let guard = self.state().lock_lock());
        guard.fields().options.can_resize_vmo()
    }

    /// Returns diagnostic information about the stream.
    pub fn get_info(&self) -> zx_info_stream_t {
        self.state().canary.assert();

        ksync::lock!(let options_guard = self.state().lock_lock());
        ksync::lock!(let seek_guard = self.state().lock_seek_lock());

        zx_info_stream_t {
            options: options_guard.fields().options.to_stream_modes(),
            padding1: [0; 4],
            seek: *seek_guard.fields().seek,
            // |content_size| is the legacy name for the stream size
            content_size: self.state().stream_size_mgr.get_stream_size(),
        }
    }
}
