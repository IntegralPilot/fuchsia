// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon vmar objects.

use crate::clock::Clock;
use crate::iob::Iob;
use crate::{Koid, Name, NullableHandle, ObjectQuery, Status, Timeline, Topic, Vmo, ok, sys};
use bitflags::bitflags;
use std::mem::MaybeUninit;
use zerocopy::{FromBytes, Immutable};
use zx_sys::PadByte;

/// An object representing a Zircon
/// [virtual memory address region](https://fuchsia.dev/fuchsia-src/concepts/objects/vm_address_region.md).
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Vmar(NullableHandle);
impl_handle_based!(Vmar);

sys::zx_info_vmar_t!(VmarInfo);

impl From<sys::zx_info_vmar_t> for VmarInfo {
    fn from(sys::zx_info_vmar_t { base, len }: sys::zx_info_vmar_t) -> VmarInfo {
        VmarInfo { base, len }
    }
}

// VmarInfo is able to be safely replaced with a byte representation and is a PoD type.
unsafe impl ObjectQuery for VmarInfo {
    const TOPIC: Topic = Topic::VMAR;
    type InfoTy = VmarInfo;
}

struct VmarMapsInfo;
unsafe impl ObjectQuery for VmarMapsInfo {
    const TOPIC: Topic = Topic::VMAR_MAPS;
    type InfoTy = MapInfo;
}

static_assert_align!(
    /// Ergonomic wrapper around `zx_info_maps_t`.
    #[repr(C)]
    #[derive(Copy, Clone, FromBytes, Immutable)]
    <sys::zx_info_maps_t> pub struct MapInfo {
        pub name <name>: Name,
        pub base <base>: usize,
        pub size <size>: usize,
        pub depth <depth>: usize,
        r#type <r#type>: sys::zx_info_maps_type_t,
        u <u>: sys::InfoMapsTypeUnion,
    }
);

impl MapInfo {
    pub fn new(
        name: Name,
        base: usize,
        size: usize,
        depth: usize,
        details: MapDetails<'_>,
    ) -> Result<MapInfo, Status> {
        let (map_type, map_details_union) = match details {
            MapDetails::None => (
                sys::ZX_INFO_MAPS_TYPE_NONE,
                sys::InfoMapsTypeUnion { mapping: Default::default() },
            ),
            MapDetails::AddressSpace => (
                sys::ZX_INFO_MAPS_TYPE_ASPACE,
                sys::InfoMapsTypeUnion { mapping: Default::default() },
            ),
            MapDetails::Vmar => (
                sys::ZX_INFO_MAPS_TYPE_VMAR,
                sys::InfoMapsTypeUnion { mapping: Default::default() },
            ),
            MapDetails::Mapping(mapping_details) => (
                sys::ZX_INFO_MAPS_TYPE_MAPPING,
                sys::InfoMapsTypeUnion {
                    mapping: {
                        let mut mapping: sys::zx_info_maps_mapping_t = Default::default();
                        mapping.mmu_flags = mapping_details.mmu_flags.bits();
                        mapping.vmo_koid = mapping_details.vmo_koid.raw_koid();
                        mapping.vmo_offset = mapping_details.vmo_offset;
                        mapping.committed_bytes = mapping_details.committed_bytes;
                        mapping.populated_bytes = mapping_details.populated_bytes;
                        mapping.committed_private_bytes = mapping_details.committed_private_bytes;
                        mapping.populated_private_bytes = mapping_details.populated_private_bytes;
                        mapping.committed_scaled_bytes = mapping_details.committed_scaled_bytes;
                        mapping.populated_scaled_bytes = mapping_details.populated_scaled_bytes;
                        mapping.committed_fractional_scaled_bytes =
                            mapping_details.committed_fractional_scaled_bytes;
                        mapping.populated_fractional_scaled_bytes =
                            mapping_details.populated_fractional_scaled_bytes;
                        mapping
                    },
                },
            ),
            MapDetails::Unknown => {
                return Err(Status::INVALID_ARGS);
            }
        };
        Ok(MapInfo { name, base, size, depth, r#type: map_type, u: map_details_union })
    }

    pub fn details<'a>(&'a self) -> MapDetails<'a> {
        match self.r#type {
            sys::ZX_INFO_MAPS_TYPE_NONE => MapDetails::None,
            sys::ZX_INFO_MAPS_TYPE_ASPACE => MapDetails::AddressSpace,
            sys::ZX_INFO_MAPS_TYPE_VMAR => MapDetails::Vmar,
            sys::ZX_INFO_MAPS_TYPE_MAPPING => {
                // SAFETY: these values are produced by the kernel or `new()` which guarantees the
                // discriminant matches the union contents.
                let raw_mapping = unsafe { &self.u.mapping };
                let mapping_details = zerocopy::transmute_ref!(raw_mapping);
                MapDetails::Mapping(mapping_details)
            }
            _ => MapDetails::Unknown,
        }
    }
}

impl std::cmp::PartialEq for MapInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.base == other.base
            && self.size == other.size
            && self.depth == other.depth
            && self.details() == other.details()
    }
}
impl std::cmp::Eq for MapInfo {}

impl std::fmt::Debug for MapInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapInfo")
            .field("name", &self.name)
            .field("base", &format_args!("{:#x}", self.base))
            .field("size", &self.size)
            .field("depth", &self.depth)
            .field("details", &self.details())
            .finish()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MapDetails<'a> {
    /// The underlying value returned by the kernel is unknown.
    Unknown,
    None,
    AddressSpace,
    Vmar,
    // Mapping returns a reference to avoid copying data.
    Mapping(&'a MappingDetails),
}

impl<'a> MapDetails<'a> {
    pub fn as_mapping(&'a self) -> Option<&'a MappingDetails> {
        match self {
            Self::Mapping(d) => Some(*d),
            _ => None,
        }
    }
}

static_assert_align!(
    #[repr(C)]
    #[derive(Copy, Clone, Debug, Eq, FromBytes, Immutable, PartialEq)]
    <sys::zx_info_maps_mapping_t> pub struct MappingDetails {
        pub mmu_flags <mmu_flags>: VmarFlagsExtended,
        padding1: [PadByte; 4],
        pub vmo_koid <vmo_koid>: Koid,
        pub vmo_offset <vmo_offset>: u64,
        pub committed_bytes <committed_bytes>: usize,
        pub populated_bytes <populated_bytes>: usize,
        pub committed_private_bytes <committed_private_bytes>: usize,
        pub populated_private_bytes <populated_private_bytes>: usize,
        pub committed_scaled_bytes <committed_scaled_bytes>: usize,
        pub populated_scaled_bytes <populated_scaled_bytes>: usize,
        pub committed_fractional_scaled_bytes <committed_fractional_scaled_bytes>: u64,
        pub populated_fractional_scaled_bytes <populated_fractional_scaled_bytes>: u64,
    }
);

impl Default for MappingDetails {
    fn default() -> MappingDetails {
        Self::from(sys::zx_info_maps_mapping_t::default())
    }
}

impl From<sys::zx_info_maps_mapping_t> for MappingDetails {
    fn from(info: sys::zx_info_maps_mapping_t) -> MappingDetails {
        zerocopy::transmute!(info)
    }
}

impl Vmar {
    pub fn allocate(
        &self,
        offset: usize,
        size: usize,
        flags: VmarFlags,
    ) -> Result<(Vmar, usize), Status> {
        let mut mapped = 0;
        let mut handle = 0;
        let status = unsafe {
            sys::zx_vmar_allocate(
                self.raw_handle(),
                flags.bits(),
                offset,
                size,
                &mut handle,
                &mut mapped,
            )
        };
        ok(status)?;
        unsafe { Ok((Vmar::from(NullableHandle::from_raw(handle)), mapped)) }
    }

    pub fn map(
        &self,
        vmar_offset: usize,
        vmo: &Vmo,
        vmo_offset: u64,
        len: usize,
        flags: VmarFlags,
    ) -> Result<usize, Status> {
        let flags = VmarFlagsExtended::from_bits_truncate(flags.bits());
        unsafe { self.map_unsafe(vmar_offset, vmo, vmo_offset, len, flags) }
    }

    /// Directly call `zx_vmar_map`.
    ///
    /// # Safety
    ///
    /// This function is unsafe because certain flags to `zx_vmar_map` may
    /// replace an existing mapping which is referenced elsewhere.
    pub unsafe fn map_unsafe(
        &self,
        vmar_offset: usize,
        vmo: &Vmo,
        vmo_offset: u64,
        len: usize,
        flags: VmarFlagsExtended,
    ) -> Result<usize, Status> {
        let mut mapped = 0;
        let status = unsafe {
            sys::zx_vmar_map(
                self.0.raw_handle(),
                flags.bits(),
                vmar_offset,
                vmo.raw_handle(),
                vmo_offset,
                len,
                &mut mapped,
            )
        };
        ok(status).map(|_| mapped)
    }

    /// Directly call `zx_vmar_unmap`.
    ///
    /// # Safety
    ///
    /// This function is unsafe because unmapping memory regions can arbitrarily
    /// cause read, write, and execution errors. Among other things, the caller
    /// must ensure that:
    ///
    /// - The region being unmapped will not be accessed after unmapping.
    /// - All references to memory in the region must be dropped or forgotten
    ///   prior to calling this method.
    /// - If the region contained executable code, then code in the region must
    ///   not be currently executing and may not be executed in the future.
    ///
    /// This is not an exhaustive list, as there are many ways to cause memory
    /// unsafety with memory mappings.
    pub unsafe fn unmap(&self, addr: usize, len: usize) -> Result<(), Status> {
        // SAFETY: The caller has guaranteed that unmapping the given region
        // will not cause undefined behavior.
        ok(unsafe { sys::zx_vmar_unmap(self.0.raw_handle(), addr, len) })
    }

    /// Perform an operation on VMOs mapped into this VMAR.
    ///
    /// Wraps the
    /// [zx_vmar_op_range](https://fuchsia.dev/fuchsia-src/reference/syscalls/vmar_op_range.md)
    /// syscall.
    pub fn op_range(&self, op: VmarOp, addr: usize, len: usize) -> Result<(), Status> {
        ok(unsafe {
            sys::zx_vmar_op_range(
                self.0.raw_handle(),
                op.into_raw(),
                addr,
                len,
                std::ptr::null_mut(),
                0,
            )
        })
    }

    /// Directly call `zx_vmar_protect`.
    ///
    /// # Safety
    ///
    /// This function is unsafe because changing the access protections for
    /// memory regions can arbitrarily cause read, write, and execution errors.
    /// Among other things, the caller must ensure that if a read, write, or
    /// execute permission is removed from a memory region, it must not read,
    /// write, or execute it respetively.
    ///
    /// This is not an exhaustive list, as there are many ways to cause memory
    /// unsafety with memory mappings.
    pub unsafe fn protect(&self, addr: usize, len: usize, flags: VmarFlags) -> Result<(), Status> {
        // SAFETY: The caller has guaranteed that protecting the given region
        // will not cause undefined behavior.
        ok(unsafe { sys::zx_vmar_protect(self.raw_handle(), flags.bits(), addr, len) })
    }

    /// Directly call `zx_vmar_destroy`.
    ///
    /// # Safety
    ///
    /// This function is unsafe because destroying a region unmaps all of the
    /// mappings within it. See [`Vmar::unmap`] for more details on how
    /// unmapping memory regions can cause memory unsafety.
    pub unsafe fn destroy(&self) -> Result<(), Status> {
        // SAFETY: The caller has guaranteed that destroying the given region
        // will not cause undefined behavior.
        ok(unsafe { sys::zx_vmar_destroy(self.raw_handle()) })
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_VMAR topic.
    pub fn info(&self) -> Result<VmarInfo, Status> {
        Ok(self.0.get_info_single::<VmarInfo>()?)
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_VMAR_MAPS topic.
    ///
    /// Returns an initialized slice of `MapInfo`s, any uninitialized trailing entries, and the
    /// total number of infos that the kernel had available.
    pub fn maps<'a>(
        &self,
        buf: &'a mut [MaybeUninit<MapInfo>],
    ) -> Result<(&'a mut [MapInfo], &'a mut [MaybeUninit<MapInfo>], usize), Status> {
        self.0.get_info::<VmarMapsInfo>(buf)
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_VMAR_MAPS topic.
    pub fn maps_vec(&self) -> Result<Vec<MapInfo>, Status> {
        self.0.get_info_vec::<VmarMapsInfo>()
    }

    /// Wraps the [zx_vmar_map_iob](https://fuchsia.dev/fuchsia-src/reference/syscalls/zx_vmar_map_iob.md)
    /// syscall.
    pub fn map_iob(
        &self,
        options: VmarFlags,
        vmar_offset: usize,
        iob: &Iob,
        region_index: u32,
        region_offset: u64,
        region_len: usize,
    ) -> Result<usize, Status> {
        let mut addr = 0;
        let status = unsafe {
            sys::zx_vmar_map_iob(
                self.raw_handle(),
                options.bits(),
                vmar_offset,
                iob.raw_handle(),
                region_index,
                region_offset,
                region_len,
                &mut addr,
            )
        };
        ok(status)?;
        Ok(addr)
    }

    /// Wraps the [zx_vmar_map_clock](https://fuchsia.dev/fuchsia-src/reference/syscalls/zx_vmar_map_clock.md)
    /// syscall.
    pub fn map_clock<I: Timeline, O: Timeline>(
        &self,
        options: VmarFlags,
        vmar_offset: usize,
        clock: &Clock<I, O>,
        length: usize,
    ) -> Result<usize, Status> {
        let mut addr = 0;
        let status = unsafe {
            sys::zx_vmar_map_clock(
                self.raw_handle(),
                options.bits(),
                vmar_offset,
                clock.raw_handle(),
                length,
                &mut addr,
            )
        };
        ok(status)?;
        Ok(addr)
    }
}

/// VM Address Range opcodes
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct VmarOp(u32);
impl VmarOp {
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn into_raw(self) -> u32 {
        self.0
    }
}

assoc_values!(VmarOp, [
    COMMIT =           sys::ZX_VMAR_OP_COMMIT;
    DECOMMIT =         sys::ZX_VMAR_OP_DECOMMIT;
    PREFETCH =         sys::ZX_VMAR_OP_PREFETCH;
    MAP_RANGE =        sys::ZX_VMAR_OP_MAP_RANGE;
    ZERO =             sys::ZX_VMAR_OP_ZERO;
    DONT_NEED =        sys::ZX_VMAR_OP_DONT_NEED;
    ALWAYS_NEED =      sys::ZX_VMAR_OP_ALWAYS_NEED;
]);

// TODO(smklein): Ideally we would have two separate sets of bitflags,
// and a union of both of them.
macro_rules! vmar_flags {
    (
        safe: [$($safe_name:ident : $safe_sys_name:ident,)*],
        extended: [$($ex_name:ident : $ex_sys_name:ident,)*],
    ) => {
        /// Flags to VMAR routines which are considered safe.
        #[repr(transparent)]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, Immutable)]
        pub struct VmarFlags(sys::zx_vm_option_t);

        bitflags! {
            impl VmarFlags: sys::zx_vm_option_t {
                $(
                    const $safe_name = sys::$safe_sys_name;
                )*
            }
        }

        impl std::fmt::Debug for VmarFlags {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                bitflags::parser::to_writer(self, f)
            }
        }

        /// Flags to all VMAR routines.
        #[repr(transparent)]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, Immutable, Default)]
        pub struct VmarFlagsExtended(sys::zx_vm_option_t);

        bitflags! {
            impl VmarFlagsExtended: sys::zx_vm_option_t {
                $(
                    const $safe_name = sys::$safe_sys_name;
                )*
                $(
                    const $ex_name = sys::$ex_sys_name;
                )*
            }
        }

        impl std::fmt::Debug for VmarFlagsExtended {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                bitflags::parser::to_writer(self, f)
            }
        }
    };
}

vmar_flags! {
    safe: [
        PERM_READ: ZX_VM_PERM_READ,
        PERM_WRITE: ZX_VM_PERM_WRITE,
        PERM_EXECUTE: ZX_VM_PERM_EXECUTE,
        COMPACT: ZX_VM_COMPACT,
        SPECIFIC: ZX_VM_SPECIFIC,
        CAN_MAP_SPECIFIC: ZX_VM_CAN_MAP_SPECIFIC,
        CAN_MAP_READ: ZX_VM_CAN_MAP_READ,
        CAN_MAP_WRITE: ZX_VM_CAN_MAP_WRITE,
        CAN_MAP_EXECUTE: ZX_VM_CAN_MAP_EXECUTE,
        MAP_RANGE: ZX_VM_MAP_RANGE,
        REQUIRE_NON_RESIZABLE: ZX_VM_REQUIRE_NON_RESIZABLE,
        ALLOW_FAULTS: ZX_VM_ALLOW_FAULTS,
        OFFSET_IS_UPPER_LIMIT: ZX_VM_OFFSET_IS_UPPER_LIMIT,
        PERM_READ_IF_XOM_UNSUPPORTED: ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED,
        FAULT_BEYOND_STREAM_SIZE: ZX_VM_FAULT_BEYOND_STREAM_SIZE,


        // Alignment options
        ALIGN_1KB: ZX_VM_ALIGN_1KB,
        ALIGN_2KB: ZX_VM_ALIGN_2KB,
        ALIGN_4KB: ZX_VM_ALIGN_4KB,
        ALIGN_8KB: ZX_VM_ALIGN_8KB,
        ALIGN_16KB: ZX_VM_ALIGN_16KB,
        ALIGN_32KB: ZX_VM_ALIGN_32KB,
        ALIGN_64KB: ZX_VM_ALIGN_64KB,
        ALIGN_128KB: ZX_VM_ALIGN_128KB,
        ALIGN_256KB: ZX_VM_ALIGN_256KB,
        ALIGN_512KB: ZX_VM_ALIGN_512KB,
        ALIGN_1MB: ZX_VM_ALIGN_1MB,
        ALIGN_2MB: ZX_VM_ALIGN_2MB,
        ALIGN_4MB: ZX_VM_ALIGN_4MB,
        ALIGN_8MB: ZX_VM_ALIGN_8MB,
        ALIGN_16MB: ZX_VM_ALIGN_16MB,
        ALIGN_32MB: ZX_VM_ALIGN_32MB,
        ALIGN_64MB: ZX_VM_ALIGN_64MB,
        ALIGN_128MB: ZX_VM_ALIGN_128MB,
        ALIGN_256MB: ZX_VM_ALIGN_256MB,
        ALIGN_512MB: ZX_VM_ALIGN_512MB,
        ALIGN_1GB: ZX_VM_ALIGN_1GB,
        ALIGN_2GB: ZX_VM_ALIGN_2GB,
        ALIGN_4GB: ZX_VM_ALIGN_4GB,
    ],
    extended: [
        SPECIFIC_OVERWRITE: ZX_VM_SPECIFIC_OVERWRITE,
    ],
}

#[cfg(test)]
mod tests {
    // The unit tests are built with a different crate name, but fuchsia_runtime returns a "real"
    // zx::Vmar that we need to use.
    use zx::{Status, VmarFlags};

    #[test]
    fn allocate_and_info() -> Result<(), Status> {
        let size = usize::pow(2, 20); // 1MiB
        let root_vmar = fuchsia_runtime::vmar_root_self();
        let (vmar, base) = root_vmar.allocate(0, size, VmarFlags::empty())?;

        let info = vmar.info()?;
        assert!(info.base == base);
        assert!(info.len == size);
        Ok(())
    }

    #[test]
    fn root_vmar_info() -> Result<(), Status> {
        let root_vmar = fuchsia_runtime::vmar_root_self();
        let info = root_vmar.info()?;
        assert!(info.base > 0);
        assert!(info.len > 0);
        Ok(())
    }

    #[test]
    fn root_vmar_maps() {
        let root_vmar = fuchsia_runtime::vmar_root_self();
        let info = root_vmar.maps_vec().unwrap();
        assert!(!info.is_empty());
    }
}
