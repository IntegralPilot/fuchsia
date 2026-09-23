// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use linux_uapi::{
    ASHMEM_GET_NAME, ASHMEM_GET_PIN_STATUS, ASHMEM_GET_PROT_MASK, ASHMEM_GET_SIZE,
    ASHMEM_IS_PINNED, ASHMEM_IS_UNPINNED, ASHMEM_NOT_PURGED, ASHMEM_PIN, ASHMEM_PURGE_ALL_CACHES,
    ASHMEM_SET_NAME, ASHMEM_SET_PROT_MASK, ASHMEM_SET_SIZE, ASHMEM_UNPIN, ASHMEM_WAS_PURGED,
};
use once_cell::sync::OnceCell;
use range_map::RangeMap;
use starnix_core::device::DeviceOps;
use starnix_core::mm::memory::MemoryObject;
use starnix_core::mm::{
    DesiredAddress, MappingName, MappingOptions, MemoryAccessor, MemoryAccessorExt, PAGE_SIZE,
    ProtectionFlags,
};
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::{
    FileObject, FileOps, FsString, InputBuffer, NamespaceNode, OutputBuffer, SeekTarget,
    default_seek, fileops_impl_noop_sync,
};
use starnix_lifecycle::AtomicCounter;
use starnix_sync::{AshmemStateLock, LockDepMutex};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::errors::Errno;
use starnix_uapi::math::round_up_to_increment;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{UserAddress, UserCString, UserRef};
use starnix_uapi::{ASHMEM_NAME_LEN, ashmem_pin, device_id, errno, error, off_t, uapi};
use std::sync::Arc;

/// Initializes the ashmem device.
pub fn ashmem_device_init(kernel: &Kernel) {
    let registry = &kernel.device_registry;

    registry
        .register_misc_device(kernel, "ashmem".into(), AshmemDevice::new())
        .expect("can register ashmem");
}

#[derive(Clone)]
pub struct AshmemDevice {
    pub next_id: Arc<AtomicCounter<u32>>,
}

pub struct Ashmem {
    memory: OnceCell<Arc<MemoryObject>>,
    state: LockDepMutex<AshmemState, AshmemStateLock>,
}

struct AshmemState {
    size: usize,
    name: FsString,
    prot_flags: ProtectionFlags,
    unpinned: RangeMap<u32, bool>,
    id: u32,
}

impl AshmemDevice {
    pub fn new() -> AshmemDevice {
        AshmemDevice { next_id: Arc::new(AtomicCounter::new(1)) }
    }
}

impl DeviceOps for AshmemDevice {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _id: device_id::DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let ashmem = Ashmem::new(self.next_id.next());
        Ok(Box::new(ashmem))
    }
}

impl Ashmem {
    fn new(id: u32) -> Ashmem {
        let state = AshmemState {
            size: 0,
            name: b"dev/ashmem\0".into(),
            prot_flags: ProtectionFlags::ACCESS_FLAGS,
            unpinned: RangeMap::<u32, bool>::default(),
            id: id,
        };

        Ashmem { memory: OnceCell::new(), state: state.into() }
    }

    fn memory(&self) -> Result<&Arc<MemoryObject>, Errno> {
        self.memory.get().ok_or_else(|| errno!(EINVAL))
    }

    fn is_mapped(&self) -> bool {
        self.memory.get().is_some()
    }
}

impl FileOps for Ashmem {
    fileops_impl_noop_sync!();

    fn is_seekable(&self) -> bool {
        true
    }

    fn seek(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        current_offset: off_t,
        target: SeekTarget,
    ) -> Result<off_t, Errno> {
        if !self.is_mapped() {
            return error!(EBADF);
        }
        let eof_offset = self.state.lock().size;
        default_seek(current_offset, target, || Ok(eof_offset.try_into().unwrap()))
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        let memory = self.memory().map_err(|_| errno!(EBADF))?;
        let file_length = self.state.lock().size;
        let actual = {
            let want_read = data.available();
            if offset < file_length {
                let to_read =
                    if file_length < offset + want_read { file_length - offset } else { want_read };
                let buf =
                    memory.read_to_vec(offset as u64, to_read as u64).map_err(|_| errno!(EIO))?;
                data.write_all(&buf[..])?;
                to_read
            } else {
                0
            }
        };
        Ok(actual)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(EINVAL)
    }

    fn mmap(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        addr: DesiredAddress,
        memory_offset: u64,
        length: usize,
        prot_flags: ProtectionFlags,
        mapping_options: MappingOptions,
    ) -> Result<UserAddress, Errno> {
        let state = self.state.lock();
        let size_paged_aligned = round_up_to_increment(state.size, *PAGE_SIZE as usize)?;

        // Filter protections
        if !state.prot_flags.contains(prot_flags) {
            return error!(EINVAL);
        }
        // Filter size
        if size_paged_aligned < length {
            return error!(EINVAL);
        }

        let memory = self
            .memory
            .get_or_try_init(|| {
                if size_paged_aligned == 0 {
                    return error!(EINVAL);
                }
                // Round up to page boundary
                let vmo = zx::Vmo::create(size_paged_aligned as u64).map_err(|_| errno!(ENOMEM))?;
                let memory = MemoryObject::from(vmo).with_zx_name(b"starnix:ashmem");
                Ok(Arc::new(memory))
            })?
            .clone();

        let mapped_addr = current_task.mm()?.map_memory(
            addr,
            memory,
            memory_offset,
            length,
            prot_flags,
            mapping_options,
            MappingName::Ashmem(state.name.clone().into()),
        )?;

        Ok(mapped_addr)
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        match request {
            #[allow(unreachable_patterns)]
            ASHMEM_SET_SIZE | starnix_uapi::arch32::ASHMEM_SET_SIZE => {
                let mut state = self.state.lock();

                if self.is_mapped() {
                    return error!(EINVAL);
                }
                state.size = arg.into();
                Ok(SUCCESS)
            }
            ASHMEM_GET_SIZE => Ok(self.state.lock().size.into()),
            ASHMEM_SET_NAME => {
                let mut state = self.state.lock();

                if self.is_mapped() {
                    return error!(EINVAL);
                }
                let mut name = current_task.read_c_string_to_vec(
                    UserCString::new(current_task, arg),
                    ASHMEM_NAME_LEN as usize,
                )?;
                name.push(0); // Add a null terminator

                state.name = name.into();
                Ok(SUCCESS)
            }
            ASHMEM_GET_NAME => {
                let state = self.state.lock();
                let name = &state.name[..];

                current_task.write_memory(arg.into(), name)?;
                Ok(SUCCESS)
            }
            #[allow(unreachable_patterns)]
            ASHMEM_SET_PROT_MASK | starnix_uapi::arch32::ASHMEM_SET_PROT_MASK => {
                let mut state = self.state.lock();
                let prot_flags =
                    ProtectionFlags::from_access_bits(arg.into()).ok_or_else(|| errno!(EINVAL))?;

                // Do not allow protections to be increased
                if !state.prot_flags.contains(prot_flags) {
                    return error!(EINVAL);
                }

                state.prot_flags = prot_flags;
                Ok(SUCCESS)
            }
            ASHMEM_GET_PROT_MASK => Ok(self.state.lock().prot_flags.bits().into()),
            ASHMEM_PIN | ASHMEM_UNPIN | ASHMEM_GET_PIN_STATUS => {
                let mut state = self.state.lock();

                if !self.is_mapped() {
                    return error!(EINVAL);
                }

                let user_ref = UserRef::<ashmem_pin>::new(arg.into());
                let pin = current_task.read_object(user_ref)?;
                let (lo, hi) =
                    (pin.offset, pin.offset.checked_add(pin.len).ok_or_else(|| errno!(EFAULT))?);

                // Bounds check
                if (lo as usize) >= state.size || (hi as usize) > state.size {
                    return error!(EINVAL);
                }

                // Aligned to page size
                if (lo as u64) % *PAGE_SIZE != 0 || (hi as u64) % *PAGE_SIZE != 0 {
                    return error!(EINVAL);
                }

                match request {
                    ASHMEM_PIN => {
                        for is_purged in state.unpinned.remove(lo..hi).iter() {
                            if *is_purged {
                                return Ok(ASHMEM_WAS_PURGED.into());
                            }
                        }

                        return Ok(ASHMEM_NOT_PURGED.into());
                    }
                    ASHMEM_UNPIN => {
                        // This method has must_use but we don't actually need to do any explicit
                        // cleanup.
                        let _ = state.unpinned.insert(lo..hi, false);
                        return Ok(ASHMEM_IS_UNPINNED.into());
                    }
                    ASHMEM_GET_PIN_STATUS => {
                        let mut intervals = state.unpinned.range(lo..hi);
                        return match intervals.next() {
                            Some(_) => Ok(ASHMEM_IS_UNPINNED.into()),
                            None => Ok(ASHMEM_IS_PINNED.into()),
                        };
                    }
                    _ => unreachable!(),
                }
            }
            ASHMEM_PURGE_ALL_CACHES => {
                let mut state = self.state.lock();
                let memory = self.memory.get().ok_or_else(|| errno!(EINVAL))?;

                if state.unpinned.is_empty() {
                    return Ok(ASHMEM_IS_PINNED.into());
                }
                let unpinned: Vec<_> = state.unpinned.iter().map(|(k, _)| k.clone()).collect();
                for range in unpinned.into_iter() {
                    let (lo, hi) = (range.start as u64, range.end as u64);
                    memory.op_range(zx::VmoOp::ZERO, lo, hi - lo).unwrap_or(());

                    // This method has must_use but we don't actually need to do any explicit
                    // cleanup.
                    let _ = state.unpinned.insert(range, true);
                }
                return Ok(ASHMEM_IS_UNPINNED.into());
            }
            #[allow(unreachable_patterns)]
            uapi::ASHMEM_GET_FILE_ID | uapi::arch32::ASHMEM_GET_FILE_ID => {
                let state = self.state.lock();
                current_task.write_object(arg.into(), &(state.id))?;
                Ok(SUCCESS)
            }
            _ => error!(ENOTTY),
        }
    }
}
