// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::{IOVecPtr, MemoryAccessorExt, PAGE_SIZE};
use crate::signals::{SignalInfo, send_standard_signal};
use crate::task::CurrentTask;
use crate::vfs::buffers::{VecInputBuffer, VecOutputBuffer};
use crate::vfs::pipe::{Pipe, PipeFileObject, PipeOperands};
use crate::vfs::syscalls::OffsetPtr;
use crate::vfs::{FdNumber, FileHandle};
use starnix_logging::track_stub;
use starnix_types::user_buffer::MAX_RW_COUNT;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::resource_limits::Resource;
use starnix_uapi::signals::SIGXFSZ;
use starnix_uapi::user_value::UserValue;
use starnix_uapi::{errno, error, off_t, uapi};
use std::sync::Arc;

fn maybe_read_offset(
    current_task: &CurrentTask,
    user_offset: OffsetPtr,
) -> Result<Option<usize>, Errno> {
    if user_offset.is_null() {
        return Ok(None);
    }
    let offset = current_task.read_multi_arch_object(user_offset)?;
    if offset < 0 {
        return error!(EINVAL);
    }
    Ok(Some(offset as usize))
}

#[derive(Debug)]
struct CopyOperand {
    file: FileHandle,
    user_offset: OffsetPtr,
    maybe_offset: Option<usize>,
}

impl CopyOperand {
    fn new(
        current_task: &CurrentTask,
        fd: FdNumber,
        user_offset: OffsetPtr,
    ) -> Result<Self, Errno> {
        let file = current_task.files().get(fd)?;
        let maybe_offset = maybe_read_offset(current_task, user_offset)?;
        Ok(Self { file, user_offset, maybe_offset })
    }

    fn maybe_as_pipe(&self) -> Option<&PipeFileObject> {
        Some(&*self.file.downcast_file::<PipeFileObject>()?)
    }

    fn maybe_write_result_offset(
        &self,
        current_task: &CurrentTask,
        progress: usize,
    ) -> Result<(), Errno> {
        if let Some(offset) = self.maybe_offset {
            let new_offset = (offset + progress) as off_t;
            current_task.write_multi_arch_object(self.user_offset, new_offset)?;
        }
        Ok(())
    }
}

fn copy_data(
    current_task: &CurrentTask,
    operand_in: CopyOperand,
    operand_out: CopyOperand,
    length: usize,
) -> Result<usize, Errno> {
    let mut remaining = length;
    let mut transferred = 0;

    let read = |progress: usize, data: &mut VecOutputBuffer| {
        if let Some(offset) = operand_in.maybe_offset {
            operand_in.file.read_at(current_task, offset + progress, data)
        } else {
            operand_in.file.read(current_task, data)
        }
    };

    let write = |progress: usize, data: &mut VecInputBuffer| {
        if let Some(offset) = operand_out.maybe_offset {
            operand_out.file.write_at(current_task, offset + progress, data)
        } else {
            operand_out.file.write(current_task, data)
        }
    };

    let mut copy = || -> Result<(), Errno> {
        while remaining > 0 {
            // The max chunk size is fairly arbitrary. Consider using a larger chunk size or
            // perhaps using asynchronous IO for better performance.
            let chunk_length = std::cmp::min(*PAGE_SIZE as usize, remaining);
            let mut buffer = VecOutputBuffer::new(chunk_length);
            let bytes_read = read(transferred, &mut buffer)?;
            operand_in.maybe_write_result_offset(current_task, transferred + bytes_read)?;
            if bytes_read == 0 {
                break;
            }
            let mut buffer = Vec::from(buffer);
            buffer.truncate(bytes_read);
            let bytes_written = write(transferred, &mut VecInputBuffer::from(buffer))?;
            operand_out.maybe_write_result_offset(current_task, transferred + bytes_written)?;
            if bytes_written == 0 {
                break;
            }
            transferred += bytes_written;
            remaining -= bytes_written;
        }
        Ok(())
    };

    match copy() {
        Ok(()) => Ok(transferred),
        Err(e) => {
            if transferred > 0 {
                Ok(transferred)
            } else {
                Err(e)
            }
        }
    }
}

pub fn sendfile(
    current_task: &CurrentTask,
    fd_out: FdNumber,
    fd_in: FdNumber,
    user_offset_in: OffsetPtr,
    count: i32,
) -> Result<usize, Errno> {
    let operand_out = CopyOperand::new(current_task, fd_out, OffsetPtr::null(current_task))?;
    let operand_in = CopyOperand::new(current_task, fd_in, user_offset_in)?;

    if count < 0 {
        return error!(EINVAL);
    }

    if !operand_in.file.flags().can_read() || !operand_out.file.flags().can_write() {
        return error!(EBADF);
    }

    if operand_in.file.node().is_dir() || operand_out.file.node().is_dir() {
        return error!(EINVAL);
    }

    // We need the in file to be seekable because we use read_at below, but this is also a proxy for
    // checking that the file supports mmap-like operations.
    if !operand_in.file.is_seekable() {
        return error!(EINVAL);
    }

    // fd_out has the O_APPEND flag set.  This is not currently supported by sendfile().
    // See https://man7.org/linux/man-pages/man2/sendfile.2.html#ERRORS
    if operand_out.file.flags().contains(OpenFlags::APPEND) {
        return error!(EINVAL);
    }

    let length = std::cmp::min(count as usize, *MAX_RW_COUNT);
    copy_data(current_task, operand_in, operand_out, length)
}

pub fn splice(
    current_task: &CurrentTask,
    fd_in: FdNumber,
    off_in: OffsetPtr,
    fd_out: FdNumber,
    off_out: OffsetPtr,
    mut len: usize,
    flags: u32,
) -> Result<usize, Errno> {
    const KNOWN_FLAGS: u32 =
        uapi::SPLICE_F_MOVE | uapi::SPLICE_F_NONBLOCK | uapi::SPLICE_F_MORE | uapi::SPLICE_F_GIFT;
    if flags & !KNOWN_FLAGS != 0 {
        track_stub!(TODO("https://fxbug.dev/322875389"), "splice flags", flags & !KNOWN_FLAGS);
        return error!(EINVAL);
    }

    if len == 0 {
        return Ok(0);
    }

    let non_blocking = flags & uapi::SPLICE_F_NONBLOCK != 0;

    let operand_in = CopyOperand::new(current_task, fd_in, off_in)?;
    let operand_out = CopyOperand::new(current_task, fd_out, off_out)?;

    if operand_in.maybe_offset.is_some() && !operand_in.file.is_seekable() {
        return error!(ESPIPE);
    }

    if operand_out.maybe_offset.is_some() && !operand_out.file.is_seekable() {
        return error!(ESPIPE);
    }

    if operand_out.maybe_offset.is_some() && operand_out.file.offset.read() < 0 {
        return error!(EINVAL);
    }

    // fd_out has the O_APPEND flag set. This is not supported by splice().
    if operand_out.file.flags().contains(OpenFlags::APPEND) {
        return error!(EINVAL);
    }

    // Enforce RLIMITs for the running process, if applicable.
    if operand_out.file.node().is_reg() {
        let fsize_limit = current_task.thread_group().get_rlimit(Resource::FSIZE);
        if fsize_limit != uapi::RLIM_INFINITY as u64 {
            let out_offset = match operand_out.maybe_offset {
                Some(off) => off as u64,
                None => operand_out.file.offset.read() as u64,
            };

            // Signal an error if the given offset directly exceeds the RLIMIT.
            if out_offset >= fsize_limit {
                send_standard_signal(current_task, SignalInfo::kernel(SIGXFSZ));
                return error!(EFBIG);
            }

            // Otherwise, ensure the buffer size conforms to the RLIMIT_FSIZE.
            if out_offset + (len as u64) > fsize_limit {
                send_standard_signal(current_task, SignalInfo::kernel(SIGXFSZ));
                len = (fsize_limit - out_offset) as usize;
            }
        }
    }

    let spliced = match (operand_in.maybe_as_pipe(), operand_out.maybe_as_pipe()) {
        // Splice can only be used when one of the files is a pipe.
        (None, None) => return error!(EINVAL),
        // If both ends are pipes, use the symmetric `Pipe::splice` function.
        (Some(_), Some(_)) => {
            let PipeOperands { mut read, mut write } = PipeFileObject::lock_pipes(
                current_task,
                &operand_in.file,
                &operand_out.file,
                len,
                non_blocking,
            )?;
            Pipe::splice(&mut read, &mut write, len)?
        }
        (None, Some(pipe_out)) => {
            let spliced = pipe_out.splice_from(
                current_task,
                &operand_out.file,
                &operand_in.file,
                operand_in.maybe_offset,
                len,
                non_blocking,
            )?;
            operand_in.maybe_write_result_offset(current_task, spliced)?;
            spliced
        }
        (Some(pipe_in), None) => {
            let spliced = pipe_in.splice_to(
                current_task,
                &operand_in.file,
                &operand_out.file,
                operand_out.maybe_offset,
                len,
                non_blocking,
            )?;
            operand_out.maybe_write_result_offset(current_task, spliced)?;
            spliced
        }
    };

    Ok(spliced)
}

pub fn vmsplice(
    current_task: &CurrentTask,
    fd: FdNumber,
    iovec_addr: IOVecPtr,
    iovec_count: UserValue<i32>,
    flags: u32,
) -> Result<usize, Errno> {
    const KNOWN_FLAGS: u32 =
        uapi::SPLICE_F_MOVE | uapi::SPLICE_F_NONBLOCK | uapi::SPLICE_F_MORE | uapi::SPLICE_F_GIFT;
    if flags & !KNOWN_FLAGS != 0 {
        track_stub!(TODO("https://fxbug.dev/322875487"), "vmsplice flags", flags & !KNOWN_FLAGS);
        return error!(EINVAL);
    }

    let non_blocking = flags & uapi::SPLICE_F_NONBLOCK != 0;

    let file = current_task.files().get(fd)?;
    let should_write = file.can_write();
    let should_read = file.can_read();

    let iovec = current_task.read_iovec(iovec_addr, iovec_count)?;
    let pipe = file.downcast_file::<PipeFileObject>().ok_or_else(|| errno!(EBADF))?;
    let mut bytes_transferred = 0;

    if should_write {
        bytes_transferred +=
            pipe.vmsplice_from(current_task, &file, iovec.clone(), non_blocking)?;
    }

    if should_read {
        bytes_transferred += pipe.vmsplice_to(current_task, &file, iovec, non_blocking)?;
    }

    Ok(bytes_transferred)
}

pub fn copy_file_range(
    current_task: &CurrentTask,
    fd_in: FdNumber,
    user_offset_in: OffsetPtr,
    fd_out: FdNumber,
    user_offset_out: OffsetPtr,
    length: usize,
    flags: u32,
) -> Result<usize, Errno> {
    if flags != 0 {
        return error!(EINVAL);
    }

    let operand_in = CopyOperand::new(current_task, fd_in, user_offset_in)?;
    let operand_out = CopyOperand::new(current_task, fd_out, user_offset_out)?;

    if !operand_in.file.flags().can_read() || !operand_out.file.flags().can_write() {
        return error!(EBADF);
    }

    // fd_out has the O_APPEND flag set. This is not supported by copy_file_range().
    if operand_out.file.flags().contains(OpenFlags::APPEND) {
        return error!(EBADF);
    }

    if operand_in.file.node().is_dir() || operand_out.file.node().is_dir() {
        return error!(EISDIR);
    }

    if !operand_in.file.node().is_reg() || !operand_out.file.node().is_reg() {
        return error!(EINVAL);
    }

    let length = std::cmp::min(length, *MAX_RW_COUNT);

    if Arc::ptr_eq(operand_in.file.node(), &operand_out.file.node()) {
        // If the input and output files are the same, we need to return EINVAL if the input and
        // output range overlap.
        if let (Some(offset_in), Some(offset_out)) =
            (operand_in.maybe_offset, operand_out.maybe_offset)
        {
            let range_in = offset_in..length;
            let range_out = offset_out..length;
            if range_in.contains(&range_out.start) || range_out.contains(&range_in.start) {
                return error!(EINVAL);
            }
        } else {
            return error!(EINVAL);
        }
    }

    copy_data(current_task, operand_in, operand_out, length)
}

pub fn tee(
    current_task: &CurrentTask,
    fd_in: FdNumber,
    fd_out: FdNumber,
    len: usize,
    flags: u32,
) -> Result<usize, Errno> {
    const KNOWN_FLAGS: u32 =
        uapi::SPLICE_F_MOVE | uapi::SPLICE_F_NONBLOCK | uapi::SPLICE_F_MORE | uapi::SPLICE_F_GIFT;
    if flags & !KNOWN_FLAGS != 0 {
        track_stub!(TODO("https://fxbug.dev/322874902"), "tee flags", flags & !KNOWN_FLAGS);
        return error!(EINVAL);
    }

    let non_blocking = flags & uapi::SPLICE_F_NONBLOCK != 0;

    let file_in = current_task.files().get(fd_in)?;
    let file_out = current_task.files().get(fd_out)?;

    // tee requires that both files are pipes.
    let PipeOperands { mut read, mut write } =
        PipeFileObject::lock_pipes(current_task, &file_in, &file_out, len, non_blocking)?;
    Pipe::tee(&mut read, &mut write, len)
}
