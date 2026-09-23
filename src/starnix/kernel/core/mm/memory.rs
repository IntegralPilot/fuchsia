// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::{
    MappingOptions, MemoryManager, PAGE_SIZE, VMEX_RESOURCE, ZX_VM_SPECIFIC_OVERWRITE,
};
use fuchsia_rcu::RcuDroppable;
use fuchsia_runtime::UtcClock;
use mapped_clock::{CLOCK_SIZE, MappedClock};
use starnix_logging::{CATEGORY_STARNIX_MM, impossible_error, set_zx_name};
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use std::mem::MaybeUninit;
use std::sync::{Arc, OnceLock};
use zerocopy::FromBytes;
use zx::Koid;

// This tracks a VMO handle along with basic information about the handle.
#[derive(Debug, RcuDroppable)]
pub struct VmoAndBasicInfo {
    vmo: zx::Vmo,
    info: OnceLock<(Koid, zx::Rights)>,
}

impl PartialEq for VmoAndBasicInfo {
    fn eq(&self, other: &Self) -> bool {
        self.vmo == other.vmo
    }
}

impl Eq for VmoAndBasicInfo {}

impl From<zx::Vmo> for VmoAndBasicInfo {
    fn from(vmo: zx::Vmo) -> Self {
        Self { vmo, info: OnceLock::new() }
    }
}

impl VmoAndBasicInfo {
    fn get_info(&self) -> &(Koid, zx::Rights) {
        self.info.get_or_init(|| {
            let info = self.vmo.basic_info().map_err(impossible_error).unwrap();
            (info.koid, info.rights)
        })
    }

    pub fn get_koid(&self) -> Koid {
        self.get_info().0
    }

    pub fn get_rights(&self) -> zx::Rights {
        self.get_info().1
    }
}

impl Drop for VmoAndBasicInfo {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        {
            if let Some((koid, rights)) = self.info.get() {
                if let Ok(info) = self.vmo.basic_info() {
                    debug_assert_eq!(*koid, info.koid, "Cached KOID mismatch");
                    debug_assert_eq!(*rights, info.rights, "Cached rights mismatch");
                }
            }
        }
    }
}

#[derive(Debug, RcuDroppable)]
pub enum MemoryObject {
    Vmo(VmoAndBasicInfo),
    /// The memory object is a bpf ring buffer. The layout it represents is:
    /// |Page1 - Page2 - Page3 .. PageN - Page3 .. PageN| where the vmo is
    /// |Page1 - Page2 - Page3 .. PageN|
    RingBuf(VmoAndBasicInfo),
    /// A memory mapped clock is backed by kernel memory, not by a VMO. So
    /// it is handled specially.  The lifecycle of this clock is:
    /// - starts off as an empty unmapped thing.
    /// - a MappedClock is created on `map_in_vmar`.
    /// - a name is added on `set_zx_name`.
    /// - most clone/resize operations return errors.
    /// - unmapped at the end of the process lifetime.
    MemoryMappedClock {
        // Koid of the `utc_clock`, cached for performance.
        koid: Koid,
        // The UTC clock handle to map to memory. Do not use it for clock reads, use
        // the public functions in `//src/starnix/kernel/core/time/utc.rs` instead
        utc_clock: UtcClock,
    },
}

impl std::cmp::Eq for MemoryObject {}

// Implemented manually as `MemoryMappedClock`'s mutex is not transparent to
// `PartialEq`.
impl std::cmp::PartialEq for MemoryObject {
    fn eq(&self, other: &MemoryObject) -> bool {
        match (self, other) {
            (MemoryObject::Vmo(info1), MemoryObject::Vmo(info2)) => info1.vmo == info2.vmo,
            (MemoryObject::RingBuf(info1), MemoryObject::RingBuf(info2)) => info1.vmo == info2.vmo,
            (MemoryObject::MemoryMappedClock { .. }, MemoryObject::MemoryMappedClock { .. }) => {
                self.get_koid() == other.get_koid()
            }
            (_, _) => false,
        }
    }
}

impl From<zx::Vmo> for MemoryObject {
    fn from(vmo: zx::Vmo) -> Self {
        Self::Vmo(VmoAndBasicInfo::from(vmo))
    }
}

impl From<UtcClock> for MemoryObject {
    fn from(utc_clock: UtcClock) -> MemoryObject {
        let koid = utc_clock.koid().expect("koid should always be readable");
        MemoryObject::MemoryMappedClock { koid, utc_clock }
    }
}

impl MemoryObject {
    pub fn as_vmo(&self) -> Option<&zx::Vmo> {
        match self {
            Self::Vmo(info) => Some(&info.vmo),
            Self::RingBuf(_) | Self::MemoryMappedClock { .. } => None,
        }
    }

    /// Returns true if this [MemoryObject] is a memory mapped clock.
    pub fn is_clock(&self) -> bool {
        match self {
            Self::Vmo(_) | Self::RingBuf(_) => false,
            Self::MemoryMappedClock { .. } => true,
        }
    }

    pub fn into_vmo(self) -> Option<zx::Vmo> {
        match self {
            Self::Vmo(info) => Some(
                info.vmo
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("duplicate_handle failed in into_vmo"),
            ),
            Self::RingBuf(_) | Self::MemoryMappedClock { .. } => None,
        }
    }

    pub fn get_content_size(&self) -> u64 {
        match self {
            Self::Vmo(info) => info.vmo.get_stream_size().map_err(impossible_error).unwrap(),
            Self::RingBuf(_) => self.get_size(),
            Self::MemoryMappedClock { .. } => CLOCK_SIZE as u64,
        }
    }

    pub fn set_content_size(&self, size: u64) -> Result<(), zx::Status> {
        match self {
            Self::Vmo(info) => info.vmo.set_stream_size(size),
            Self::RingBuf(_) => Ok(()),
            Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    pub fn get_size(&self) -> u64 {
        match self {
            Self::Vmo(info) => info.vmo.get_size().map_err(impossible_error).unwrap(),
            Self::RingBuf(info) => {
                let base_size = info.vmo.get_size().map_err(impossible_error).unwrap();
                (base_size - *PAGE_SIZE) * 2
            }
            Self::MemoryMappedClock { .. } => CLOCK_SIZE as u64,
        }
    }

    pub fn set_size(&self, size: u64) -> Result<(), zx::Status> {
        match self {
            Self::Vmo(info) => info.vmo.set_size(size),
            Self::RingBuf(_) | Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    pub fn create_child(
        &self,
        option: zx::VmoChildOptions,
        offset: u64,
        size: u64,
    ) -> Result<Self, zx::Status> {
        match self {
            Self::Vmo(info) => info.vmo.create_child(option, offset, size).map(Self::from),
            Self::RingBuf(info) => info
                .vmo
                .create_child(option, offset, size)
                .map(|vmo| Self::RingBuf(VmoAndBasicInfo::from(vmo))),
            Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    pub fn duplicate_handle(&self, rights: zx::Rights) -> Result<Self, zx::Status> {
        match self {
            Self::Vmo(info) => info.vmo.duplicate_handle(rights).map(Self::from),
            Self::RingBuf(info) => info
                .vmo
                .duplicate_handle(rights)
                .map(|vmo| Self::RingBuf(VmoAndBasicInfo::from(vmo))),
            Self::MemoryMappedClock { utc_clock, .. } => {
                utc_clock.duplicate_handle(rights).map(|c| Self::from(c))
            }
        }
    }

    pub fn read(&self, data: &mut [u8], offset: u64) -> Result<(), zx::Status> {
        match self {
            Self::Vmo(info) => info.vmo.read(data, offset),
            Self::RingBuf(_) | Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    pub fn read_to_array<T: Copy + FromBytes, const N: usize>(
        &self,
        offset: u64,
    ) -> Result<[T; N], zx::Status> {
        match self {
            Self::Vmo(info) => info.vmo.read_to_array(offset),
            Self::RingBuf(_) => Err(zx::Status::NOT_SUPPORTED),
            // There does not seem to be an API that allows this read.
            Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    pub fn read_to_vec(&self, offset: u64, length: u64) -> Result<Vec<u8>, zx::Status> {
        match self {
            Self::Vmo(info) => info.vmo.read_to_vec(offset, length),
            Self::RingBuf(_) => Err(zx::Status::NOT_SUPPORTED),
            // See the note in `read_to_array` above.
            Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    pub fn read_uninit<'a>(
        &self,
        data: &'a mut [MaybeUninit<u8>],
        offset: u64,
    ) -> Result<&'a mut [u8], zx::Status> {
        match self {
            Self::Vmo(info) => info.vmo.read_uninit(data, offset),
            Self::RingBuf(_) => Err(zx::Status::NOT_SUPPORTED),
            // See the note in `read_to_array` above.
            Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    /// Reads from the memory.
    ///
    /// # Safety
    ///
    /// Callers must guarantee that the buffer is valid to write to.
    ///
    /// # Errors
    ///
    /// Returns `zx::Status::NOT_SUPPORTED` where unsupported.
    pub unsafe fn read_raw(
        &self,
        buffer: *mut u8,
        buffer_length: usize,
        offset: u64,
    ) -> Result<(), zx::Status> {
        match self {
            #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
            Self::Vmo(info) => unsafe { info.vmo.read_raw(buffer, buffer_length, offset) },
            Self::RingBuf(_) => Err(zx::Status::NOT_SUPPORTED),
            // See the note in `read_to_array` above.
            Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    /// Write to memory.
    ///
    /// # Errors
    ///
    /// Returns `zx::Status::NOT_SUPPORTED` for read-only memory.
    pub fn write(&self, data: &[u8], offset: u64) -> Result<(), zx::Status> {
        match self {
            Self::Vmo(info) => info.vmo.write(data, offset),
            Self::RingBuf(_) | Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    /// Returns the koid of the underlying memory-like object.
    ///
    /// Should be cheap to call frequently.
    pub fn get_koid(&self) -> Koid {
        match self {
            Self::Vmo(info) => info.get_koid(),
            Self::RingBuf(info) => info.get_koid(),
            Self::MemoryMappedClock { koid, .. } => *koid,
        }
    }

    /// Returns the rights of the underlying memory-like object.
    pub fn get_rights(&self) -> zx::Rights {
        match self {
            Self::Vmo(info) => info.get_rights(),
            Self::RingBuf(info) => info.get_rights(),
            Self::MemoryMappedClock { utc_clock, .. } => {
                utc_clock.basic_info().map_err(impossible_error).unwrap().rights
            }
        }
    }

    /// Returns `zx::VmoInfo` for a memory object that supports it.
    ///
    /// # Panics
    ///
    /// Calling `info` on a `MemoryObject` that is not represented by a VMO
    /// will panic. To avoid this in code, call `is_clock` before attempting.
    pub fn info(&self) -> Result<zx::VmoInfo, Errno> {
        match self {
            Self::Vmo(info) | Self::RingBuf(info) => info.vmo.info().map_err(|_| errno!(EIO)),
            // Use `is_clock` to avoid calling info on a clock.
            Self::MemoryMappedClock { .. } => {
                panic!("info() is not supported on a memory mapped clock")
            }
        }
    }

    pub fn set_zx_name(&self, name: &[u8]) {
        match self {
            Self::Vmo(info) | Self::RingBuf(info) => set_zx_name(&info.vmo, name),
            Self::MemoryMappedClock { .. } => {
                // The memory mapped clock is a singleton, so it does not
                // seem appropriate to give it a zx name.
            }
        }
    }

    pub fn with_zx_name(self, name: &[u8]) -> Self {
        self.set_zx_name(name);
        self
    }

    pub fn op_range(
        &self,
        op: zx::VmoOp,
        mut offset: u64,
        mut size: u64,
    ) -> Result<(), zx::Status> {
        match self {
            Self::Vmo(info) => info.vmo.op_range(op, offset, size),
            Self::RingBuf(info) => {
                let vmo_size = info.vmo.get_size().map_err(impossible_error).unwrap();
                let data_size = vmo_size - (2 * *PAGE_SIZE);
                let memory_size = vmo_size + data_size;
                if offset + size > memory_size {
                    return Err(zx::Status::OUT_OF_RANGE);
                }
                // If `offset` is greater than `vmo_size`, the operation is equivalent to the one
                // done on the first part of the memory range.
                if offset >= vmo_size {
                    offset -= data_size;
                }
                // If the operation spill over the end if the vmo, it must be done on the start of
                // the data part of the vmo.
                if offset + size > vmo_size {
                    info.vmo.op_range(op, 2 * *PAGE_SIZE, offset + size - vmo_size)?;
                    size = vmo_size - offset;
                }
                info.vmo.op_range(op, offset, size)
            }
            Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    pub fn replace_as_executable(self, vmex: &zx::Resource) -> Result<Self, zx::Status> {
        match self {
            Self::Vmo(info) => {
                let vmo = info.vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
                let exec_vmo = vmo.replace_as_executable(vmex)?;
                Ok(Self::from(exec_vmo))
            }
            Self::RingBuf(_) | Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    pub fn map_in_vmar(
        &self,
        vmar: &zx::Vmar,
        vmar_offset: usize,
        mut memory_offset: u64,
        len: usize,
        flags: zx::VmarFlags,
    ) -> Result<usize, zx::Status> {
        match self {
            Self::Vmo(info) => vmar.map(vmar_offset, &info.vmo, memory_offset, len, flags),
            Self::RingBuf(info) => {
                let vmo_size = info.vmo.get_size().map_err(impossible_error).unwrap();
                let data_size = vmo_size - (2 * *PAGE_SIZE);
                let memory_size = vmo_size + data_size;
                if memory_offset.checked_add(len as u64).ok_or(zx::Status::OUT_OF_RANGE)?
                    > memory_size
                {
                    return Err(zx::Status::OUT_OF_RANGE);
                }
                // If `memory_offset` is greater than `vmo_size`, the operation is equivalent to
                // the one done on the first part of the memory range.
                if memory_offset >= vmo_size {
                    memory_offset -= data_size;
                }
                // Map the vmo for the full length. This ensures the kernel will choose a range
                // that can accommodate the full length so that the second mapping will not erase
                // another mapping.
                let result = vmar.map(
                    vmar_offset,
                    &info.vmo,
                    memory_offset,
                    len,
                    flags | zx::VmarFlags::ALLOW_FAULTS,
                )?;
                // The maximal amount of data that can have been mapped from the vmo with the
                // previous operation.
                let max_mapped_len = (vmo_size - memory_offset) as usize;
                // If more data is needed, the data part of the vmo must be mapped again, replacing
                // the part of the previous mapping that contained no data.
                if len > max_mapped_len {
                    let vmar_info = vmar.info().map_err(|_| errno!(EIO))?;
                    let base_address = vmar_info.base;
                    // The request should map the data part of the vmo a second time
                    let second_mapping_address = vmar
                        .map(
                            result + max_mapped_len - base_address,
                            &info.vmo,
                            2 * *PAGE_SIZE,
                            len - max_mapped_len,
                            flags | ZX_VM_SPECIFIC_OVERWRITE,
                        )
                        .expect("Mapping should not fail as the space has been reserved");
                    debug_assert_eq!(second_mapping_address, result + max_mapped_len);
                }
                Ok(result)
            }
            Self::MemoryMappedClock { utc_clock, .. } => {
                // The memory mapped clock API only allows memory offset of 0, and a page-sized
                // length of the mapping. No offset or partial mappings are allowed.
                assert_eq!(0, memory_offset, "memory mapped clock must be at memory offset 0");

                // We don't need to remember this, since vmar will know how to unmap it.
                let memory_mapped_clock = MappedClock::try_new_without_unmap(
                    &utc_clock,
                    vmar,
                    flags,
                    vmar_offset as u64,
                )?;
                Ok(memory_mapped_clock.raw_addr())
            }
        }
    }

    pub fn memmove(
        &self,
        options: zx::TransferDataOptions,
        dst_offset: u64,
        src_offset: u64,
        size: u64,
    ) -> Result<(), zx::Status> {
        match self {
            Self::Vmo(info) => {
                info.vmo.transfer_data(options, dst_offset, size, &info.vmo, src_offset)
            }
            Self::RingBuf(_) | Self::MemoryMappedClock { .. } => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    pub fn clone_memory(
        self: &Arc<Self>,
        rights: zx::Rights,
        options: MappingOptions,
    ) -> Result<Arc<Self>, Errno> {
        if self.is_clock() {
            return Err(errno!(ENOTSUP, "clone_memory not supported on memory mapped clock"));
        }

        // Non-anonymous memory is pager-backed, and we can clone it if we don't need write
        // rights.
        Ok(if !options.contains(MappingOptions::ANONYMOUS) && !rights.contains(zx::Rights::WRITE) {
            self.clone()
        } else {
            fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "pager_backed_memory_snapshot");
            let mut cloned_memory = self
                .create_child(
                    zx::VmoChildOptions::SNAPSHOT_MODIFIED | zx::VmoChildOptions::RESIZABLE,
                    0,
                    self.get_size(),
                )
                .map_err(MemoryManager::get_errno_for_map_err)?;
            if rights.contains(zx::Rights::EXECUTE) {
                cloned_memory = cloned_memory
                    .replace_as_executable(&VMEX_RESOURCE)
                    .map_err(impossible_error)?;
            }

            Arc::new(cloned_memory)
        })
    }
}
