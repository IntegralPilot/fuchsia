// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::canonicalize_ioctl_request;
use crate::dma_heap::{Alloc, dma_heap_device_register};
use bitfield::bitfield;
use bstr::ByteSlice;
use fidl_fuchsia_hardware_qualcomm_fastrpc as frpc;
use starnix_core::device::DeviceOps;
use starnix_core::mm::memory::MemoryObject;
use starnix_core::mm::{MemoryAccessor, MemoryAccessorExt, ProtectionFlags};
use starnix_core::task::{CurrentTask, Kernel, Pid, ThreadLockupDetector};
use starnix_core::vfs::{
    Anon, FdFlags, FdNumber, FileObject, FileObjectState, FileOps, NamespaceNode,
    call_fidl_and_await_close,
};
use starnix_core::{
    fileops_impl_dataless, fileops_impl_memory, fileops_impl_noop_sync, fileops_impl_seekless,
};
use starnix_logging::{log_debug, log_error, log_warn};
use starnix_sync::{FastrpcInnerState, LockDepMutex};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_types::user_buffer::UserBuffer;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::{Errno, ErrnoCode};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{MultiArchUserRef, UserCString, UserRef};
use starnix_uapi::{errno, error};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};

type IoctlInvokeFdPtr = MultiArchUserRef<
    linux_uapi::fastrpc_ioctl_invoke_fd,
    linux_uapi::arch32::fastrpc_ioctl_invoke_fd,
>;

type IoctlInvoke2Ptr =
    MultiArchUserRef<linux_uapi::fastrpc_ioctl_invoke2, linux_uapi::arch32::fastrpc_ioctl_invoke2>;

type IoctlInitPtr =
    MultiArchUserRef<linux_uapi::fastrpc_ioctl_init, linux_uapi::arch32::fastrpc_ioctl_init>;

type IoctlInvokePtr =
    MultiArchUserRef<linux_uapi::fastrpc_ioctl_invoke, linux_uapi::arch32::fastrpc_ioctl_invoke>;

type RemoteBufPtr = MultiArchUserRef<linux_uapi::remote_buf, linux_uapi::arch32::remote_buf>;

const FASTRPC_MAX_DSP_ATTRIBUTES: usize = 256;
const FASTRPC_MAX_ATTRIBUTES: usize = 260;

// Performance data capability not supported.
const PERF_CAPABILITY_SUPPORT: u32 = 0;

// Newer error version.
const KERNEL_ERROR_CODE_V1_SUPPORT: u32 = 0;

// Userspace allocation supported through dma-heap.
const USERSPACE_ALLOCATION_SUPPORT: u32 = 1;

// No signaling support.
const DSPSIGNAL_SUPPORT: u32 = 0;

const KERNEL_CAPABILITIES: [u32; FASTRPC_MAX_ATTRIBUTES - FASTRPC_MAX_DSP_ATTRIBUTES] = [
    PERF_CAPABILITY_SUPPORT,
    KERNEL_ERROR_CODE_V1_SUPPORT,
    USERSPACE_ALLOCATION_SUPPORT,
    DSPSIGNAL_SUPPORT,
];

const ASYNC_FASTRPC_CAP: usize = 9;
const DMA_HANDLE_REVERSE_RPC_CAP: usize = 129;

const INVOKE2_MAX: u32 = 4;

const FASTRPC_INIT_ATTACH: u32 = 0;
const FASTRPC_INIT_CREATE_STATIC: u32 = 2;

const INIT_FILELEN_MAX: u32 = 2 * 1024 * 1024;
const INIT_MEMLEN_MAX: u32 = 8 * 1024 * 1024;

// Scalars:
// These are how we designate the number of various elements inside an rpc method.
// It comes in a u32 with the bit format:
//
// aaam mmmm    iiii iiii    oooo oooo    xxxx yyyy
//
// a = attribute (3 bits)
// m = method (5 bits)
// i = inbuf (8 bits)
// o = outbuf (8 bits)
// x = in handle (4 bits)
// y = out handle (4 bits)
//
// Currently we only support buffers and not handles in this implementation.
bitfield! {
    pub struct Scalar(u32);
    impl Debug;

    pub method_id, _: 28, 24;
    pub inbuffs, _: 23, 16;
    pub outbuffs, _: 15, 8;
    pub inhandles, _: 7, 4;
    pub outhandles, _: 3, 0;
}

impl Scalar {
    fn len(&self) -> u32 {
        self.inbuffs() as u32
            + self.outbuffs() as u32
            + self.inhandles() as u32
            + self.outhandles() as u32
    }
}

// All fidl transport errors should be considered as error, and converted to IO error.
fn fidl_error_to_errno(info: &str, error: fidl::Error) -> starnix_uapi::errors::Errno {
    if !error.is_closed() {
        log_error!("{}: {:?}", info, error);
        return errno!(EIO);
    }

    // Log at most once every 5 seconds for PEER_CLOSED errors which can spam if the driver
    // has crashed.
    static LAST_LOG_TIME: AtomicI64 = AtomicI64::new(0);
    let now = zx::MonotonicInstant::get().into_nanos();
    let last = LAST_LOG_TIME.load(Ordering::Relaxed);
    if now - last > 5_000_000_000 {
        LAST_LOG_TIME.store(now, Ordering::Relaxed);
        log_error!("{}: {:?}", info, error);
    }
    errno!(EIO)
}

// zx.Status errors from fidl domain errors can be converted into fdio-like errnos.
fn zx_i32_to_errno(info: &str, error: i32) -> starnix_uapi::errors::Errno {
    starnix_uapi::from_status_like_fdio!(zx::Status::err_from_raw(error), info)
}

// zx.Status errors from syscalls can be converted into fdio-like errnos.
fn zx_status_to_errno(info: &str, error: zx::Status) -> starnix_uapi::errors::Errno {
    starnix_uapi::from_status_like_fdio!(error, info)
}

// Directly passthrough retval errors from the driver to the user.
fn retval_i32_to_errno(info: &str, error: i32) -> starnix_uapi::errors::Errno {
    let code = ErrnoCode::from_return_value(error as u64);
    log_debug!("{}: {:?}", info, code);
    Errno::with_context(code, info)
}

fn fastrpc_align(size: u64) -> Result<u64, Errno> {
    // 128 is the memory alignment within the fastrpc framework.
    size.checked_next_multiple_of(128).ok_or_else(|| errno!(EOVERFLOW))
}

struct DmaBufFile {
    memory: Arc<MemoryObject>,
}

impl DmaBufFile {
    fn new(memory: Arc<MemoryObject>) -> Box<Self> {
        Box::new(Self { memory })
    }
}

impl FileOps for DmaBufFile {
    fileops_impl_memory!(self, &self.memory);
    fileops_impl_noop_sync!();

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        match canonicalize_ioctl_request(current_task, request) {
            linux_uapi::DMA_BUF_SET_NAME_B => {
                let name = current_task.read_c_string_to_vec(
                    UserCString::new(current_task, arg),
                    linux_uapi::DMA_BUF_NAME_LEN as usize,
                )?;
                log_debug!(
                    "dma buf file with koid {:?} got ioctl set name: {}",
                    self.memory.get_koid(),
                    name
                );
                self.memory.set_zx_name(&name);
                Ok(SUCCESS)
            }
            _ => error!(ENOTTY),
        }
    }
}

struct SystemHeap {
    device: Arc<frpc::SecureFastRpcSynchronousProxy>,
}

impl Alloc for SystemHeap {
    fn alloc(
        &self,
        current_task: &CurrentTask,
        size: u64,
        fd_flags: FdFlags,
    ) -> Result<FdNumber, Errno> {
        let vmo = self
            .device
            .allocate(size, zx::MonotonicInstant::INFINITE)
            .map_err(|e| fidl_error_to_errno("allocate call", e))?
            .map_err(|e| zx_i32_to_errno("allocate", e))?;

        log_debug!("allocated vmo with koid {:?}", vmo.koid());

        let memory = Arc::new(MemoryObject::from(vmo));

        let file = Anon::new_private_file(
            current_task,
            DmaBufFile::new(memory),
            OpenFlags::RDWR,
            "[fastrpc:buffer]",
        );

        current_task.add_file(file, fd_flags)
    }
}

#[derive(Default)]
struct FastRPCFileState {
    session: Option<Arc<frpc::RemoteDomainSynchronousProxy>>,
    payload_vmos: VecDeque<frpc::SharedPayloadBuffer>,
    cid: Option<i32>,
    pid: Option<Pid>,
}

struct ParsedInvoke {
    invoke: linux_uapi::fastrpc_ioctl_invoke,
    scalar: Scalar,
    fd_vmos: Option<Vec<Option<zx::Vmo>>>,
}

#[derive(PartialEq)]
struct BufferWithMergeInfo {
    start: u64,
    end: u64,
    buffer_index: usize,
    merge_contribution: u64,
    merge_offset: u64,
}

impl std::fmt::Debug for BufferWithMergeInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferWithMergeInfo")
            .field("start", &format_args!("{:#x}", self.start))
            .field("end", &format_args!("{:#x}", self.end))
            .field("buffer_index", &self.buffer_index)
            .field("merge_contribution", &format_args!("{:#x}", self.merge_contribution))
            .field("merge_offset", &self.merge_offset)
            .finish()
    }
}

struct OutputArgumentInfo {
    mapped: bool,
    offset: u64,
    length: u64,
}

struct PayloadInformation {
    payload_buffer: Option<frpc::SharedPayloadBuffer>,
    input_args: Vec<frpc::ArgumentEntry>,
    output_args: Vec<frpc::ArgumentEntry>,
    output_info: Vec<OutputArgumentInfo>,
}

struct FastRPCFile {
    pid_open: Pid,
    device: Arc<frpc::SecureFastRpcSynchronousProxy>,
    cached_capabilities: Arc<OnceLock<[u32; FASTRPC_MAX_DSP_ATTRIBUTES]>>,
    inner_state: LockDepMutex<FastRPCFileState, FastrpcInnerState>,
}

impl FastRPCFile {
    fn new(
        pid_open: Pid,
        device: Arc<frpc::SecureFastRpcSynchronousProxy>,
        cached_capabilities: Arc<OnceLock<[u32; FASTRPC_MAX_DSP_ATTRIBUTES]>>,
    ) -> Self {
        Self {
            pid_open,
            device,
            cached_capabilities,
            inner_state: LockDepMutex::new(FastRPCFileState::default()),
        }
    }

    fn invoke(
        &self,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let parsed_invoke = Self::parse_invoke_request(current_task, request, arg)?;
        let ParsedInvoke { invoke: info, scalar, mut fd_vmos } = parsed_invoke;

        log_debug!(
            "FastRPC ioctl invoke, scalar {} ({}, {}, {}), handle {}",
            info.sc,
            scalar.method_id(),
            scalar.inbuffs(),
            scalar.outbuffs(),
            info.handle
        );

        let length = scalar.len();
        let inbufs = scalar.inbuffs() as u32;

        if scalar.inhandles() != 0 || scalar.outhandles() != 0 {
            log_error!("handles in scalar not supported.");
            return error!(ENOSYS);
        }

        let remote_bufs = current_task.read_multi_arch_objects_to_vec(
            RemoteBufPtr::new(current_task, info.pra),
            length as usize,
        )?;
        let merged_buffers = Self::merge_buffers(&fd_vmos, &remote_bufs)?;
        let payload = Self::get_payload_info(
            current_task,
            &self.inner_state,
            &merged_buffers,
            &remote_bufs,
            &mut fd_vmos,
            inbufs,
        )?;

        let payload_buffer_id = match &payload.payload_buffer {
            Some(buffer) => buffer.id,
            None => 0,
        };

        let session = self.get_session()?;
        let invoke_res = {
            let _waiting_guard = ThreadLockupDetector::pause_tracking();
            session.invoke(
                current_task.get_tid(),
                info.handle,
                scalar.method_id() as u32,
                payload_buffer_id,
                payload.input_args,
                payload.output_args,
                zx::MonotonicInstant::INFINITE,
            )
        };

        let buffer_after_invoke = |success: bool| -> Result<(), Errno> {
            if success {
                if let Some(buffer) = &payload.payload_buffer {
                    self.process_out_bufs(
                        current_task,
                        &remote_bufs,
                        &buffer.vmo,
                        &payload.output_info,
                        inbufs,
                    )?;
                }
            }

            if let Some(buffer) = payload.payload_buffer {
                log_debug!("returning payload buffer {}", buffer.id);
                self.inner_state.lock().payload_vmos.push_back(buffer);
            };

            Ok(())
        };

        match invoke_res {
            Ok(Ok(())) => {
                buffer_after_invoke(true)?;
                Ok(SUCCESS)
            }
            Ok(Err(e)) => {
                buffer_after_invoke(false)?;
                Err(retval_i32_to_errno("invoke", e))
            }
            Err(e) => {
                buffer_after_invoke(false)?;
                Err(fidl_error_to_errno("invoke call", e))
            }
        }
    }

    fn get_session(&self) -> Result<Arc<frpc::RemoteDomainSynchronousProxy>, Errno> {
        let inner = self.inner_state.lock();
        Ok(inner.session.as_ref().ok_or_else(|| errno!(ENOENT))?.clone())
    }

    fn get_capabilities_from_device(
        &self,
        _domain: u32,
    ) -> Result<[u32; FASTRPC_MAX_DSP_ATTRIBUTES], Errno> {
        let capabilities = self
            .device
            .get_capabilities(zx::MonotonicInstant::INFINITE)
            .map_err(|e| fidl_error_to_errno("get_capabilities call", e))?
            .map_err(|e| retval_i32_to_errno("get_capabilities", e))?;

        let mut res: [u32; FASTRPC_MAX_DSP_ATTRIBUTES] = [0; FASTRPC_MAX_DSP_ATTRIBUTES];
        let attribute_buffer_length = FASTRPC_MAX_DSP_ATTRIBUTES - 1;
        // 0th capability is not filled by the driver.
        res[0] = 0;
        res[1..(attribute_buffer_length + 1)]
            .copy_from_slice(&capabilities[..attribute_buffer_length]);

        log_debug!("ASYNC_FASTRPC_CAP: {}", res[ASYNC_FASTRPC_CAP]);
        log_debug!("DMA_HANDLE_REVERSE_RPC_CAP: {}", res[DMA_HANDLE_REVERSE_RPC_CAP]);
        Ok(res)
    }

    fn get_capabilities(&self, domain: u32, attr: usize) -> Result<u32, Errno> {
        if attr >= FASTRPC_MAX_ATTRIBUTES {
            return error!(EOVERFLOW);
        }

        if attr >= FASTRPC_MAX_DSP_ATTRIBUTES {
            return Ok(KERNEL_CAPABILITIES[(attr) - FASTRPC_MAX_DSP_ATTRIBUTES]);
        }

        // OnceLock's get_or_try_init is a nightly feature so we end up with this which might call
        // get_capabilities_from_device unnecessarily.
        let caps = self.cached_capabilities.get();
        match caps {
            Some(caps) => Ok(caps[attr]),
            None => {
                let from_device = self.get_capabilities_from_device(domain)?;
                let caps = self.cached_capabilities.get_or_init(|| from_device);
                Ok(caps[attr])
            }
        }
    }

    fn parse_invoke_request(
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<ParsedInvoke, Errno> {
        match canonicalize_ioctl_request(current_task, request) {
            linux_uapi::FASTRPC_IOCTL_INVOKE_FD => {
                let info = current_task
                    .read_multi_arch_object(IoctlInvokeFdPtr::new(current_task, arg))?;
                log_debug!("FastRPC ioctl invoke_fd {:?}", info);

                let scalar = Scalar(info.inv.sc);

                let fds = current_task
                    .read_objects_to_vec::<i32>(info.fds.into(), scalar.len() as usize)?;

                // Collect the vmos for our fds, as well as a mapping to use locally to check
                // if an entry is mapped or not.
                let mut fd_vmos = vec![];
                for fd in fds {
                    // A non-postive fd signifies a non-mapped entry.
                    if fd > 0 {
                        let file = current_task.files().get(FdNumber::from_raw(fd))?;
                        let dma_buf =
                            file.downcast_file::<DmaBufFile>().ok_or_else(|| errno!(EBADF))?;

                        let fd_vmo = dma_buf
                            .memory
                            .as_vmo()
                            .ok_or_else(|| errno!(EBADF))?
                            .duplicate_handle(fidl::Rights::SAME_RIGHTS)
                            .map_err(|e| {
                                zx_status_to_errno("parse_invoke_request duplicate_handle", e)
                            })?;

                        fd_vmos.push(Some(fd_vmo));
                    } else {
                        fd_vmos.push(None);
                    }
                }

                Ok(ParsedInvoke { invoke: info.inv, scalar, fd_vmos: Some(fd_vmos) })
            }
            linux_uapi::FASTRPC_IOCTL_INVOKE => {
                let info =
                    current_task.read_multi_arch_object(IoctlInvokePtr::new(current_task, arg))?;
                let scalar = Scalar(info.sc);
                Ok(ParsedInvoke { invoke: info, scalar, fd_vmos: None })
            }
            _ => {
                error!(ENOSYS)
            }
        }
    }

    fn merge_buffers(
        fd_vmos: &Option<Vec<Option<zx::Vmo>>>,
        remote_bufs: &[linux_uapi::remote_buf],
    ) -> Result<Vec<BufferWithMergeInfo>, Errno> {
        // Get the indices for the buffers since we will be shuffling them around.
        let mut indexed_buffers = remote_bufs
            .iter()
            .enumerate()
            .map(|(index, buf_ref)| (index, buf_ref))
            .collect::<Vec<_>>();

        // Sort them by start address, if equal start address we sort by reverse of end address.
        indexed_buffers.sort_by(|(_, b1), (_, b2)| {
            let start_comparison = b1.pv.cmp(&b2.pv);
            let end_reverse_comparison = (b2.pv.addr + b2.len).cmp(&(b1.pv.addr + b1.len));
            match start_comparison {
                std::cmp::Ordering::Equal => end_reverse_comparison,
                std::cmp::Ordering::Greater | std::cmp::Ordering::Less => start_comparison,
            }
        });

        let mut results = Vec::with_capacity(remote_bufs.len());

        // This is used to track the current merge region's endpoint. We don't need to track
        // a start as we have already sorted them using the start address.
        let mut current_merge_end: u64 = 0;

        for (original_idx, buffer) in indexed_buffers.into_iter() {
            let start = buffer.pv.addr;
            let end = buffer.pv.addr.checked_add(buffer.len).ok_or_else(|| errno!(EOVERFLOW))?;

            // The merge_contribution signifies the unique memory that needs to be used to represent
            // this buffer in memory.
            let merge_contribution;

            // The merge offset is used to get the actual start of a buffer given a merge point,
            // this is a negative offset on the current_merge_end.
            let merge_offset;

            if Self::is_buffer_mapped(fd_vmos, original_idx) {
                // Ignore buffers that are mapped in our overlap calculations.
                merge_contribution = 0;
                merge_offset = 0;
            } else if start < current_merge_end && end <= current_merge_end {
                // Buffer lives entirely in the current merged region.
                merge_contribution = 0;
                merge_offset = current_merge_end - start;
            } else if start < current_merge_end {
                // Buffer lives partially in the current merged region.
                merge_contribution = end - current_merge_end;
                merge_offset = current_merge_end - start;

                // Extend the merge region.
                current_merge_end = end;
            } else {
                // Buffer does not live anywhere in the current merged region.
                merge_contribution = end - start;
                merge_offset = 0;

                // Start a new merged region.
                current_merge_end = end;
            }

            results.push(BufferWithMergeInfo {
                start,
                end,
                buffer_index: original_idx,
                merge_contribution,
                merge_offset,
            });
        }

        Ok(results)
    }

    fn get_payload_size(
        fd_vmos: &Option<Vec<Option<zx::Vmo>>>,
        merged_buffers: &Vec<BufferWithMergeInfo>,
    ) -> Result<u64, Errno> {
        let mut size: u64 = 0;
        for i in 0..merged_buffers.len() {
            let buffer_index = merged_buffers[i].buffer_index;

            // Include in payload if not mapped.
            if !Self::is_buffer_mapped(fd_vmos, buffer_index) {
                if merged_buffers[i].merge_offset == 0 {
                    // Align each new merged region.
                    size = fastrpc_align(size)?;
                }

                size = size
                    .checked_add(merged_buffers[i].merge_contribution)
                    .ok_or_else(|| errno!(EOVERFLOW))?;
            }
        }

        Ok(size)
    }

    fn is_buffer_mapped(fd_vmos: &Option<Vec<Option<zx::Vmo>>>, idx: usize) -> bool {
        match fd_vmos {
            None => false,
            Some(vmos) => vmos[idx].is_some(),
        }
    }

    fn get_mapped_memory_and_offset(
        current_task: &CurrentTask,
        buf: &linux_uapi::remote_buf,
        fd_vmos: &mut Option<Vec<Option<zx::Vmo>>>,
        idx: usize,
    ) -> Result<(u64, zx::Vmo), Errno> {
        let (mm_vmo, mm_offset) = current_task
            .mm()?
            .get_mapping_memory(buf.pv.into(), ProtectionFlags::READ | ProtectionFlags::WRITE)?;

        if let Some(fd_vmo) =
            fd_vmos.as_deref_mut().and_then(|v| v.get_mut(idx)).and_then(|o| o.take())
        {
            if mm_vmo.get_koid()
                == fd_vmo
                    .basic_info()
                    .map_err(|e| zx_status_to_errno("get_mapped_memory_and_offset basic_info", e))?
                    .koid
            {
                log_debug!(
                    "FastRPC ioctl invoke found allocated vmo for user address. koid: {:?}. User pointer: {:#x} offset in vmo: {}",
                    mm_vmo.get_koid(),
                    buf.pv.addr,
                    mm_offset
                );
                return Ok((mm_offset, fd_vmo));
            }
        }

        error!(ENOSYS)
    }

    fn get_payload_info(
        current_task: &CurrentTask,
        inner_state: &LockDepMutex<FastRPCFileState, FastrpcInnerState>,
        merged_buffers: &Vec<BufferWithMergeInfo>,
        remote_bufs: &Vec<linux_uapi::remote_buf>,
        fd_vmos: &mut Option<Vec<Option<zx::Vmo>>>,
        inbufs: u32,
    ) -> Result<PayloadInformation, Errno> {
        let payload_size = Self::get_payload_size(fd_vmos, &merged_buffers)?;
        let payload_buffer = if payload_size == 0 {
            None
        } else {
            let payload_buffer =
                inner_state.lock().payload_vmos.pop_front().ok_or_else(|| errno!(ENOBUFS))?;
            log_debug!("selected payload buffer {}", payload_buffer.id);
            Some(payload_buffer)
        };

        // Construct these with the usize buffer_index so we can sort them after.
        //
        // Output is specified twice, once for the fidl invocation, the other
        // to be used after the invocation is done since we want to copy data back
        // to the user.
        let mut input_args: Vec<(usize, frpc::ArgumentEntry)> = vec![];
        let mut output_args: Vec<(usize, frpc::ArgumentEntry)> = vec![];
        let mut output_info: Vec<(usize, OutputArgumentInfo)> = vec![];
        let mut curr_merge_point = 0;

        for merged_buffer in merged_buffers {
            let buf =
                remote_bufs.get(merged_buffer.buffer_index).expect("to have index in remote bufs");
            let is_mapped = Self::is_buffer_mapped(fd_vmos, merged_buffer.buffer_index);

            let (entry, offset) = if is_mapped {
                let (offset, vmo) = Self::get_mapped_memory_and_offset(
                    current_task,
                    buf,
                    fd_vmos,
                    merged_buffer.buffer_index,
                )?;
                (
                    frpc::ArgumentEntry::VmoArgument(frpc::VmoArgument {
                        vmo,
                        offset,
                        length: buf.len,
                    }),
                    offset,
                )
            } else {
                if merged_buffer.merge_offset == 0 {
                    curr_merge_point = fastrpc_align(curr_merge_point)?;
                }

                let offset = curr_merge_point - merged_buffer.merge_offset;
                curr_merge_point = curr_merge_point
                    .checked_add(merged_buffer.merge_contribution)
                    .ok_or_else(|| errno!(EOVERFLOW))?;
                (frpc::ArgumentEntry::Argument(frpc::Argument { offset, length: buf.len }), offset)
            };

            if merged_buffer.buffer_index < inbufs as usize {
                // Write data and flush for non-empty, non-mapped input buffers.
                if !is_mapped && buf.len > 0 {
                    let buf_data = current_task.read_buffer(&UserBuffer {
                        address: buf.pv.into(),
                        length: buf.len as usize,
                    })?;

                    let vmo = &payload_buffer.as_ref().expect("payload buffer").vmo;
                    vmo.write(buf_data.as_slice(), offset)
                        .map_err(|e| zx_status_to_errno("get_payload_info write", e))?;
                }

                input_args.push((merged_buffer.buffer_index, entry));
            } else {
                output_args.push((merged_buffer.buffer_index, entry));
                output_info.push((
                    merged_buffer.buffer_index,
                    OutputArgumentInfo { mapped: is_mapped, offset, length: buf.len },
                ));
            }
        }

        input_args.sort_by_key(|e| e.0);
        output_args.sort_by_key(|e| e.0);
        output_info.sort_by_key(|e| e.0);

        let input_args = input_args.into_iter().map(|e| e.1).collect();
        let output_args = output_args.into_iter().map(|e| e.1).collect();
        let output_info = output_info.into_iter().map(|e| e.1).collect();

        Ok(PayloadInformation { payload_buffer, input_args, output_args, output_info })
    }

    fn process_out_bufs(
        &self,
        current_task: &CurrentTask,
        remote_bufs: &Vec<linux_uapi::remote_buf>,
        payload_vmo: &zx::Vmo,
        output_infos: &Vec<OutputArgumentInfo>,
        inbufs: u32,
    ) -> Result<(), Errno> {
        let max_len = output_infos.iter().filter(|i| !i.mapped).map(|i| i.length).max();
        let Some(max_len) = max_len else {
            return Ok(());
        };

        let mut read_vec = vec![0; max_len as usize];

        for (output_index, output_info) in output_infos.iter().enumerate() {
            if output_info.mapped {
                continue;
            }
            if output_info.length == 0 {
                continue;
            }

            let buf = &remote_bufs[output_index + inbufs as usize];

            assert_eq!(buf.len, output_info.length);

            payload_vmo
                .read(&mut read_vec[0..output_info.length as usize], output_info.offset)
                .map_err(|e| zx_status_to_errno("process_out_bufs read", e))?;

            let _ = current_task
                .write_memory(buf.pv.into(), &read_vec[0..output_info.length as usize])?;
        }
        Ok(())
    }
}

impl FileOps for FastRPCFile {
    fileops_impl_noop_sync!();
    fileops_impl_seekless!();
    fileops_impl_dataless!();

    fn close(self: Box<Self>, _file: &FileObjectState, _current_task: &CurrentTask) {
        let inner = self.inner_state.lock();
        if let Some(ref session) = inner.session {
            call_fidl_and_await_close(frpc::RemoteDomainSynchronousProxy::close, session.as_ref());
        }
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let pid = current_task.pid.clone();
        if pid != self.pid_open {
            return error!(EPERM);
        }

        match canonicalize_ioctl_request(current_task, request) {
            linux_uapi::FASTRPC_IOCTL_INVOKE | linux_uapi::FASTRPC_IOCTL_INVOKE_FD => {
                self.invoke(current_task, request, arg)
            }
            linux_uapi::FASTRPC_IOCTL_GETINFO => {
                let user_info = UserRef::<u32>::from(arg);
                let channel_id = current_task.read_object(user_info)?;
                let device_channel_id = self
                    .device
                    .get_channel_id(zx::MonotonicInstant::INFINITE)
                    .map_err(|e| fidl_error_to_errno("get_channel_id call", e))?
                    .map_err(|e| zx_i32_to_errno("get_channel_id", e))?;

                if device_channel_id != channel_id {
                    return error!(EPERM);
                }

                let mut inner = self.inner_state.lock();
                if inner.session.is_some() {
                    return error!(EEXIST);
                }

                inner.pid = Some(pid);
                inner.cid = Some(channel_id as i32);

                log_debug!("FastRPC ioctl getinfo for channel_id {}", channel_id);

                // The reply value indicates to the user whether the smmu
                // is enabled for this session. On Fuchsia currently we enable the smmu in a
                // passthrough mode and hardcode a stream id. Eventually when we fully enable the
                // smmu we will need to allocate and use specific context banks for sessions so
                // this value will need to come from the driver.
                current_task.write_object(user_info, &(1u32))?;
                Ok(SUCCESS)
            }
            linux_uapi::FASTRPC_IOCTL_GET_DSP_INFO => {
                // UserRef note:
                // fastrpc_ioctl_capability is checked for check_arch_independent_layout.
                let user_ref = UserRef::<linux_uapi::fastrpc_ioctl_capability>::new(arg.into());
                let mut info = current_task.read_object(user_ref)?;
                log_debug!(
                    "FastRPC ioctl get dsp info domain {} attribute {}",
                    info.domain,
                    info.attribute_ID
                );
                info.capability = self.get_capabilities(info.domain, info.attribute_ID as usize)?;
                current_task.write_object(user_ref, &info)?;
                Ok(SUCCESS)
            }
            linux_uapi::FASTRPC_IOCTL_INVOKE2 => {
                let info =
                    current_task.read_multi_arch_object(IoctlInvoke2Ptr::new(current_task, arg))?;
                if info.req > INVOKE2_MAX {
                    log_debug!("FastRPC ioctl invoke2 out of bounds req number {}", info.req);
                    return error!(ENOTTY);
                }

                log_debug!("FastRPC ioctl invoke2 {:?}", info);
                error!(ENOSYS)
            }
            linux_uapi::FASTRPC_IOCTL_INIT => {
                let info =
                    current_task.read_multi_arch_object(IoctlInitPtr::new(current_task, arg))?;

                if info.filelen >= INIT_FILELEN_MAX || info.memlen >= INIT_MEMLEN_MAX {
                    return error!(EFBIG);
                }

                let mut inner = self.inner_state.lock();
                if inner.session.is_some() {
                    return error!(EEXIST);
                }

                match info.flags {
                    FASTRPC_INIT_ATTACH => {
                        log_debug!("FastRPC ioctl init FASTRPC_INIT_ATTACH {:?}", info);

                        let (client, server) =
                            fidl::endpoints::create_sync_proxy::<frpc::RemoteDomainMarker>();

                        self.device
                            .attach_root_domain(server, zx::MonotonicInstant::INFINITE)
                            .map_err(|e| fidl_error_to_errno("attach_root_domain call", e))?
                            .map_err(|e| retval_i32_to_errno("attach_root_domain", e))?;

                        inner.payload_vmos = client
                            .get_payload_buffer_set(3, zx::MonotonicInstant::INFINITE)
                            .map_err(|e| fidl_error_to_errno("get_payload_buffer_set call", e))?
                            .map_err(|e| zx_i32_to_errno("get_payload_buffer_set", e))?
                            .into();

                        inner.session = Some(Arc::new(client));
                        Ok(SUCCESS)
                    }
                    FASTRPC_INIT_CREATE_STATIC => {
                        log_debug!("FastRPC ioctl init FASTRPC_INIT_CREATE_STATIC {:?}", info);
                        let file_name = current_task.read_c_string_to_vec(
                            UserCString::new(current_task, info.file),
                            info.filelen as usize,
                        )?;

                        let (client, server) =
                            fidl::endpoints::create_sync_proxy::<frpc::RemoteDomainMarker>();

                        self.device
                            .create_static_domain(
                                file_name.to_str().map_err(|_| errno!(EINVAL))?,
                                info.memlen,
                                server,
                                zx::MonotonicInstant::INFINITE,
                            )
                            .map_err(|e| fidl_error_to_errno("create_static_domain call", e))?
                            .map_err(|e| retval_i32_to_errno("create_static_domain", e))?;

                        inner.payload_vmos = client
                            .get_payload_buffer_set(3, zx::MonotonicInstant::INFINITE)
                            .map_err(|e| fidl_error_to_errno("get_payload_buffer_set call", e))?
                            .map_err(|e| zx_i32_to_errno("get_payload_buffer_set", e))?
                            .into();

                        inner.session = Some(Arc::new(client));
                        Ok(SUCCESS)
                    }
                    _ => {
                        log_warn!("FastRPC ioctl init with unsupported flag {:?}", info);
                        error!(ENOSYS)
                    }
                }
            }
            _ => error!(ENOTTY),
        }
    }
}

#[derive(Clone)]
struct FastRPCDevice {
    device: Arc<frpc::SecureFastRpcSynchronousProxy>,
    cached_capabilities: Arc<OnceLock<[u32; FASTRPC_MAX_DSP_ATTRIBUTES]>>,
}

impl FastRPCDevice {
    fn new(device: Arc<frpc::SecureFastRpcSynchronousProxy>) -> Self {
        Self { device, cached_capabilities: Arc::new(OnceLock::new()) }
    }
}

impl DeviceOps for FastRPCDevice {
    fn open(
        &self,
        current_task: &CurrentTask,
        _id: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(FastRPCFile::new(
            current_task.pid.clone(),
            self.device.clone(),
            self.cached_capabilities.clone(),
        )))
    }
}

pub fn fastrpc_device_init(kernel: &Kernel) {
    let device = fuchsia_component::client::connect_to_protocol_sync::<frpc::SecureFastRpcMarker>()
        .expect("Failed to connect to fuchsia.hardware.qualcomm.fastrpc.SecureFastRpc");

    let device = Arc::new(device);

    // This is called the "system" dma heap, but as of now the fastrpc client is its only client.
    // Because fastrpc needs to be aware of the fds from this, we are putting the implementation
    // in this module.
    dma_heap_device_register(kernel, "system", SystemHeap { device: device.clone() });

    let device = FastRPCDevice::new(device);
    let registry = &kernel.device_registry;
    registry
        .register_dyn_device(
            kernel,
            "adsprpc-smd-secure".into(),
            registry.objects.get_or_create_class("fastrpc".into(), registry.objects.virtual_bus()),
            device,
        )
        .expect("Can register heap device");
}

#[cfg(test)]
pub mod tests {
    use crate::fastrpc::{BufferWithMergeInfo, FastRPCFile, FastRPCFileState};
    use fidl_fuchsia_hardware_qualcomm_fastrpc::{
        Argument, ArgumentEntry, SharedPayloadBuffer, VmoArgument,
    };
    use linux_uapi::{remote_buf, uaddr};
    use starnix_core::mm::ProtectionFlags;
    use starnix_core::testing::{UserMemoryWriter, map_memory, spawn_kernel_and_run};
    use starnix_sync::LockDepMutex;
    use starnix_types::PAGE_SIZE;
    use starnix_uapi::user_address::UserAddress;

    #[fuchsia::test]
    fn merge_buffers_test_empty_input() {
        let remote_bufs: Vec<remote_buf> = vec![];
        let fd_vmos = None;
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        assert!(results.is_empty());
    }

    #[fuchsia::test]
    fn merge_buffers_test_single_buffer() {
        let remote_bufs = vec![remote_buf { pv: uaddr { addr: 100 }, len: 50 }];
        let fd_vmos = None;
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        assert_eq!(
            results,
            vec![BufferWithMergeInfo {
                start: 100,
                end: 150,
                buffer_index: 0,
                merge_contribution: 50,
                merge_offset: 0,
            }]
        );
    }

    #[fuchsia::test]
    fn merge_buffers_test_disjoint_buffers_sorted_input() {
        let remote_bufs = vec![
            remote_buf { pv: uaddr { addr: 100 }, len: 50 },
            remote_buf { pv: uaddr { addr: 200 }, len: 50 },
        ];
        let fd_vmos = None;
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        assert_eq!(
            results,
            vec![
                BufferWithMergeInfo {
                    start: 100,
                    end: 150,
                    buffer_index: 0,
                    merge_contribution: 50,
                    merge_offset: 0
                },
                BufferWithMergeInfo {
                    start: 200,
                    end: 250,
                    buffer_index: 1,
                    merge_contribution: 50,
                    merge_offset: 0
                },
            ]
        );
    }

    #[fuchsia::test]
    fn merge_buffers_test_disjoint_buffers_unsorted_input() {
        let remote_bufs = vec![
            remote_buf { pv: uaddr { addr: 200 }, len: 50 }, // index 0
            remote_buf { pv: uaddr { addr: 100 }, len: 50 }, // index 1
        ];
        let fd_vmos = None;
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        assert_eq!(
            results,
            vec![
                BufferWithMergeInfo {
                    start: 100,
                    end: 150,
                    buffer_index: 1,
                    merge_contribution: 50,
                    merge_offset: 0
                },
                BufferWithMergeInfo {
                    start: 200,
                    end: 250,
                    buffer_index: 0,
                    merge_contribution: 50,
                    merge_offset: 0
                },
            ]
        );
    }

    #[fuchsia::test]
    fn merge_buffers_test_touching_buffers() {
        let remote_bufs = vec![
            remote_buf { pv: uaddr { addr: 100 }, len: 50 },
            remote_buf { pv: uaddr { addr: 150 }, len: 50 },
        ];
        let fd_vmos = None;
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        assert_eq!(
            results,
            vec![
                BufferWithMergeInfo {
                    start: 100,
                    end: 150,
                    buffer_index: 0,
                    merge_contribution: 50,
                    merge_offset: 0
                },
                BufferWithMergeInfo {
                    start: 150,
                    end: 200,
                    buffer_index: 1,
                    merge_contribution: 50,
                    merge_offset: 0
                },
            ]
        );
    }

    #[fuchsia::test]
    fn merge_buffers_test_touching_buffers_one_mapped() {
        let remote_bufs = vec![
            remote_buf { pv: uaddr { addr: 100 }, len: 50 },
            remote_buf { pv: uaddr { addr: 150 }, len: 50 },
        ];
        let fd_vmos = Some(vec![None, Some(zx::Vmo::create(1).expect("vmo"))]);
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        assert_eq!(
            results,
            vec![
                BufferWithMergeInfo {
                    start: 100,
                    end: 150,
                    buffer_index: 0,
                    merge_contribution: 50,
                    merge_offset: 0
                },
                BufferWithMergeInfo {
                    start: 150,
                    end: 200,
                    buffer_index: 1,
                    merge_contribution: 00,
                    merge_offset: 0
                },
            ]
        );
    }

    #[fuchsia::test]
    fn merge_buffers_test_partial_overlap() {
        let remote_bufs = vec![
            remote_buf { pv: uaddr { addr: 100 }, len: 100 },
            remote_buf { pv: uaddr { addr: 150 }, len: 100 },
        ];
        let fd_vmos = None;
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        assert_eq!(
            results,
            vec![
                BufferWithMergeInfo {
                    start: 100,
                    end: 200,
                    buffer_index: 0,
                    merge_contribution: 100,
                    merge_offset: 0
                },
                BufferWithMergeInfo {
                    start: 150,
                    end: 250,
                    buffer_index: 1,
                    merge_contribution: 50,
                    merge_offset: 50
                },
            ]
        );
    }

    #[fuchsia::test]
    fn merge_buffers_test_full_containment() {
        let remote_bufs = vec![
            remote_buf { pv: uaddr { addr: 100 }, len: 100 },
            remote_buf { pv: uaddr { addr: 120 }, len: 50 },
        ];
        let fd_vmos = None;
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        assert_eq!(
            results,
            vec![
                BufferWithMergeInfo {
                    start: 100,
                    end: 200,
                    buffer_index: 0,
                    merge_contribution: 100,
                    merge_offset: 0
                },
                BufferWithMergeInfo {
                    start: 120,
                    end: 170,
                    buffer_index: 1,
                    merge_contribution: 0,
                    merge_offset: 80
                },
            ]
        );
    }

    #[fuchsia::test]
    fn merge_buffers_test_same_start_address() {
        let remote_bufs = vec![
            remote_buf { pv: uaddr { addr: 100 }, len: 50 },
            remote_buf { pv: uaddr { addr: 100 }, len: 100 },
        ];
        let fd_vmos = None;
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        assert_eq!(
            results,
            vec![
                BufferWithMergeInfo {
                    start: 100,
                    end: 200,
                    buffer_index: 1,
                    merge_contribution: 100,
                    merge_offset: 0
                },
                BufferWithMergeInfo {
                    start: 100,
                    end: 150,
                    buffer_index: 0,
                    merge_contribution: 0,
                    merge_offset: 100
                },
            ]
        );
    }

    #[fuchsia::test]
    fn merge_buffers_test_zero_length_buffers() {
        let remote_bufs = vec![
            remote_buf { pv: uaddr { addr: 100 }, len: 50 },
            remote_buf { pv: uaddr { addr: 120 }, len: 0 },
            remote_buf { pv: uaddr { addr: 200 }, len: 0 },
        ];
        let fd_vmos = None;
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        assert_eq!(
            results,
            vec![
                BufferWithMergeInfo {
                    start: 100,
                    end: 150,
                    buffer_index: 0,
                    merge_contribution: 50,
                    merge_offset: 0
                },
                BufferWithMergeInfo {
                    start: 120,
                    end: 120,
                    buffer_index: 1,
                    merge_contribution: 0,
                    merge_offset: 30
                },
                BufferWithMergeInfo {
                    start: 200,
                    end: 200,
                    buffer_index: 2,
                    merge_contribution: 0,
                    merge_offset: 0
                },
            ]
        );
    }

    #[fuchsia::test]
    fn merge_buffers_test_complex() {
        let remote_bufs = vec![
            remote_buf { pv: uaddr { addr: 500 }, len: 100 }, // 500-600, index 0
            remote_buf { pv: uaddr { addr: 100 }, len: 100 }, // 100-200, index 1
            remote_buf { pv: uaddr { addr: 150 }, len: 100 }, // 150-250, index 2
            remote_buf { pv: uaddr { addr: 400 }, len: 50 },  // 400-450, index 3
            remote_buf { pv: uaddr { addr: 180 }, len: 20 },  // 180-200, index 4 (contained)
        ];
        let fd_vmos = None;
        let results = FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
        let expected = vec![
            // First merge region (100 -> 200 -> 250)
            BufferWithMergeInfo {
                start: 100,
                end: 200,
                buffer_index: 1,
                merge_contribution: 100,
                merge_offset: 0,
            },
            BufferWithMergeInfo {
                start: 150,
                end: 250,
                buffer_index: 2,
                merge_contribution: 50,
                merge_offset: 50,
            },
            BufferWithMergeInfo {
                start: 180,
                end: 200,
                buffer_index: 4,
                merge_contribution: 0,
                merge_offset: 70,
            },
            // Second merge region (400 -> 450)
            BufferWithMergeInfo {
                start: 400,
                end: 450,
                buffer_index: 3,
                merge_contribution: 50,
                merge_offset: 0,
            },
            // Third merge region (500 -> 600)
            BufferWithMergeInfo {
                start: 500,
                end: 600,
                buffer_index: 0,
                merge_contribution: 100,
                merge_offset: 0,
            },
        ];

        assert_eq!(results, expected);
    }

    #[fuchsia::test]
    async fn get_payload_info_test_complex_range_values() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::from_ptr(100 as usize), 500);

            // Use the same buffers as merge_buffers_test_complex but just offset them in the
            // memory we got mapped above.
            let remote_bufs = vec![
                remote_buf { pv: uaddr { addr: (addr + 500u64).expect("add").into() }, len: 100 },
                remote_buf { pv: uaddr { addr: (addr + 100u64).expect("add").into() }, len: 100 },
                remote_buf { pv: uaddr { addr: (addr + 150u64).expect("add").into() }, len: 100 },
                remote_buf { pv: uaddr { addr: (addr + 400u64).expect("add").into() }, len: 50 },
                remote_buf { pv: uaddr { addr: (addr + 180u64).expect("add").into() }, len: 20 },
            ];

            // This variant of the test puts range based values into the user memory.
            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[0].pv.into());
            let data = (0..remote_bufs[0].len as u8).collect::<Vec<_>>();
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[1].pv.into());
            let data = (0..remote_bufs[1].len as u8).collect::<Vec<_>>();
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[2].pv.into());
            let data = (0..remote_bufs[2].len as u8).collect::<Vec<_>>();
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[3].pv.into());
            let data = (0..remote_bufs[3].len as u8).collect::<Vec<_>>();
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[4].pv.into());
            let data = (0..remote_bufs[4].len as u8).collect::<Vec<_>>();
            writer.write(&data);

            let vmo = zx::Vmo::create(*PAGE_SIZE).expect("vmo create");
            let vmo_dup = vmo.duplicate_handle(fidl::Rights::SAME_RIGHTS).expect("dup");

            let state = LockDepMutex::new(FastRPCFileState {
                session: None,
                payload_vmos: vec![SharedPayloadBuffer { id: 1, vmo: vmo }].into(),
                cid: None,
                pid: None,
            });
            let mut fd_vmos = None;

            let merged_buffers =
                FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
            let payload_info = FastRPCFile::get_payload_info(
                &current_task,
                &state,
                &merged_buffers,
                &remote_bufs,
                &mut fd_vmos,
                3,
            )
            .expect("get_payload_info");

            assert_eq!(
                payload_info.input_args,
                vec![
                    ArgumentEntry::Argument(Argument { offset: 384, length: 100 }),
                    ArgumentEntry::Argument(Argument { offset: 0, length: 100 }),
                    ArgumentEntry::Argument(Argument { offset: 50, length: 100 })
                ]
            );

            assert_eq!(
                payload_info.output_args,
                vec![
                    ArgumentEntry::Argument(Argument { offset: 256, length: 50 }),
                    ArgumentEntry::Argument(Argument { offset: 80, length: 20 }),
                ]
            );

            // Tests that the input buffers have been correctly setup in the payload.
            //
            // Since the buffer at 256 is part of the output, it will not be copied into the vmo as
            // part of the setup. But because 80-100 is already included as part of the input buffer
            // from 0-100 and 50-150 the data appears in here just as a side effect.
            let data = vmo_dup.read_to_vec::<u8>(0, 484).expect("read");
            let expected_vmo = vec![
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
                23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43,
                44, 45, 46, 47, 48, 49, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
                17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,
                10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60,
                61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
                19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39,
                40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60,
                61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99,
            ];

            assert_eq!(expected_vmo, data);
        })
        .await;
    }

    #[fuchsia::test]
    async fn get_payload_info_test_complex_single_values() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::from_ptr(100 as usize), 500);

            // Use the same buffers as merge_buffers_test_complex but just offset them in the
            // memory we got mapped above.
            let remote_bufs = vec![
                remote_buf { pv: uaddr { addr: (addr + 500u64).expect("add").into() }, len: 100 },
                remote_buf { pv: uaddr { addr: (addr + 100u64).expect("add").into() }, len: 100 },
                remote_buf { pv: uaddr { addr: (addr + 150u64).expect("add").into() }, len: 100 },
                remote_buf { pv: uaddr { addr: (addr + 400u64).expect("add").into() }, len: 50 },
                remote_buf { pv: uaddr { addr: (addr + 180u64).expect("add").into() }, len: 20 },
            ];

            // This variant of the test puts single values based on the buffer index
            // into the user memory.
            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[0].pv.into());
            let data = vec![10; remote_bufs[0].len as usize];
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[1].pv.into());
            let data = vec![11; remote_bufs[1].len as usize];
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[2].pv.into());
            let data = vec![12; remote_bufs[2].len as usize];
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[3].pv.into());
            let data = vec![13; remote_bufs[3].len as usize];
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[4].pv.into());
            let data = vec![14; remote_bufs[4].len as usize];
            writer.write(&data);

            let vmo = zx::Vmo::create(*PAGE_SIZE).expect("vmo create");
            let vmo_dup = vmo.duplicate_handle(fidl::Rights::SAME_RIGHTS).expect("dup");

            let state = LockDepMutex::new(FastRPCFileState {
                session: None,
                payload_vmos: vec![SharedPayloadBuffer { id: 1, vmo: vmo }].into(),
                cid: None,
                pid: None,
            });
            let mut fd_vmos = None;

            let merged_buffers =
                FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
            let payload_info = FastRPCFile::get_payload_info(
                &current_task,
                &state,
                &merged_buffers,
                &remote_bufs,
                &mut fd_vmos,
                3,
            )
            .expect("get_payload_info");

            assert_eq!(
                payload_info.input_args,
                vec![
                    ArgumentEntry::Argument(Argument { offset: 384, length: 100 }),
                    ArgumentEntry::Argument(Argument { offset: 0, length: 100 }),
                    ArgumentEntry::Argument(Argument { offset: 50, length: 100 })
                ]
            );

            assert_eq!(
                payload_info.output_args,
                vec![
                    ArgumentEntry::Argument(Argument { offset: 256, length: 50 }),
                    ArgumentEntry::Argument(Argument { offset: 80, length: 20 }),
                ]
            );

            // Tests that the input buffers have been correctly setup in the payload.
            //
            // Since the buffer at 256 is part of the output, it will not be copied into the vmo as
            // part of the setup. But because 80-100 is already included as part of the input buffer
            // from 0-100 and 50-150 the data appears in here just as a side effect.
            let data = vmo_dup.read_to_vec::<u8>(0, 484).expect("read");
            let expected_vmo = vec![
                11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
                11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
                11, 11, 11, 11, 11, 11, 11, 11, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
                12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 14, 14, 14, 14,
                14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 12, 12, 12, 12, 12,
                12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
                12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
                12, 12, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
                10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
                10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
                10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
                10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
                10, 10, 10, 10, 10, 10,
            ];

            assert_eq!(expected_vmo, data);
        })
        .await;
    }

    #[fuchsia::test]
    async fn get_payload_info_test_complex_single_values_with_one_mapped() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::from_ptr(100 as usize), 400);

            let mapped_addr = starnix_core::testing::map_memory_anywhere(current_task, 100);
            let (mm_vmo, _mm_offset) = current_task
                .mm()
                .unwrap()
                .get_mapping_memory(mapped_addr, ProtectionFlags::READ | ProtectionFlags::WRITE)
                .expect("mem");

            // Use the same buffers as merge_buffers_test_complex but just offset them in the
            // memory we got mapped above.
            let remote_bufs = vec![
                remote_buf { pv: uaddr { addr: mapped_addr.into() }, len: 100 },
                remote_buf { pv: uaddr { addr: (addr + 100u64).expect("add").into() }, len: 100 },
                remote_buf { pv: uaddr { addr: (addr + 150u64).expect("add").into() }, len: 100 },
                remote_buf { pv: uaddr { addr: (addr + 400u64).expect("add").into() }, len: 50 },
                remote_buf { pv: uaddr { addr: (addr + 180u64).expect("add").into() }, len: 20 },
            ];

            // This variant of the test puts single values based on the buffer index
            // into the user memory.
            let mut writer = UserMemoryWriter::new(&current_task, mapped_addr.into());
            let data = vec![10; remote_bufs[0].len as usize];
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[1].pv.into());
            let data = vec![11; remote_bufs[1].len as usize];
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[2].pv.into());
            let data = vec![12; remote_bufs[2].len as usize];
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[3].pv.into());
            let data = vec![13; remote_bufs[3].len as usize];
            writer.write(&data);

            let mut writer = UserMemoryWriter::new(&current_task, remote_bufs[4].pv.into());
            let data = vec![14; remote_bufs[4].len as usize];
            writer.write(&data);

            let vmo = zx::Vmo::create(*PAGE_SIZE).expect("vmo create");
            let vmo_dup = vmo.duplicate_handle(fidl::Rights::SAME_RIGHTS).expect("dup");

            let state = LockDepMutex::new(FastRPCFileState {
                session: None,
                payload_vmos: vec![SharedPayloadBuffer { id: 1, vmo: vmo }].into(),
                cid: None,
                pid: None,
            });
            let mut fd_vmos = Some(vec![
                Some(
                    mm_vmo
                        .as_vmo()
                        .unwrap()
                        .duplicate_handle(fidl::Rights::SAME_RIGHTS)
                        .expect("dup"),
                ),
                None,
                None,
                None,
                None,
            ]);

            let merged_buffers =
                FastRPCFile::merge_buffers(&fd_vmos, &remote_bufs).expect("merge to succeed");
            let payload_info = FastRPCFile::get_payload_info(
                &current_task,
                &state,
                &merged_buffers,
                &remote_bufs,
                &mut fd_vmos,
                3,
            )
            .expect("get_payload_info");

            let ArgumentEntry::VmoArgument(VmoArgument { vmo: _vmo, offset: _offset, length }) =
                &payload_info.input_args[0]
            else {
                panic!("wrong type")
            };

            assert_eq!(length, &100u64);

            assert_eq!(
                payload_info.input_args[1..3],
                vec![
                    ArgumentEntry::Argument(Argument { offset: 0, length: 100 }),
                    ArgumentEntry::Argument(Argument { offset: 50, length: 100 })
                ]
            );

            assert_eq!(
                payload_info.output_args,
                vec![
                    ArgumentEntry::Argument(Argument { offset: 256, length: 50 }),
                    ArgumentEntry::Argument(Argument { offset: 80, length: 20 }),
                ]
            );

            // Tests that the input buffers have been correctly setup in the payload.
            //
            // The buffers at 500 is mapped so it should not appear here.
            //
            // Since the buffer at 256 is part of the output, it will not be copied into the vmo as
            // part of the setup. But because 80-100 is already included as part of the input buffer
            // from 0-100 and 50-150 the data appears in here just as a side effect.
            let data = vmo_dup.read_to_vec::<u8>(0, 484).expect("read");
            let expected_vmo = vec![
                11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
                11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
                11, 11, 11, 11, 11, 11, 11, 11, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
                12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 14, 14, 14, 14,
                14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 12, 12, 12, 12, 12,
                12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
                12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
                12, 12, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0,
            ];

            assert_eq!(expected_vmo, data);
        })
        .await;
    }
}
