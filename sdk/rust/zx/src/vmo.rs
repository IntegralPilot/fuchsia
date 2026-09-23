// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon vmo objects.

use crate::{
    Bti, Koid, Name, NullableHandle, ObjectQuery, Resource, Rights, Status, Topic, ok, sys,
};
use bitflags::bitflags;
use std::mem::MaybeUninit;
use std::ptr;
use zerocopy::{FromBytes, Immutable};
use zx_sys::PadByte;

/// An object representing a Zircon
/// [virtual memory object](https://fuchsia.dev/fuchsia-src/concepts/objects/vm_object.md).
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Vmo(NullableHandle);
impl_handle_based!(Vmo);

static_assert_align!(
    #[doc="Ergonomic equivalent of [sys::zx_info_vmo_t]. Must be ABI-compatible with it."]
    #[repr(C)]
    #[derive(Debug, Copy, Clone, Eq, PartialEq, FromBytes, Immutable)]
    <sys::zx_info_vmo_t> pub struct VmoInfo {
        pub koid <koid>: Koid,
        pub name <name>: Name,
        pub size_bytes <size_bytes>: u64,
        pub parent_koid <parent_koid>: Koid,
        pub num_children <num_children>: usize,
        pub num_mappings <num_mappings>: usize,
        pub share_count <share_count>: usize,
        pub flags <flags>: VmoInfoFlags,
        padding1: [PadByte; 4],
        pub committed_bytes <committed_bytes>: u64,
        pub handle_rights <handle_rights>: Rights,
        cache_policy <cache_policy>: u32,
        pub metadata_bytes <metadata_bytes>: u64,
        pub committed_change_events <committed_change_events>: u64,
        pub populated_bytes <populated_bytes>: u64,
        pub committed_private_bytes <committed_private_bytes>: u64,
        pub populated_private_bytes <populated_private_bytes>: u64,
        pub committed_scaled_bytes <committed_scaled_bytes>: u64,
        pub populated_scaled_bytes <populated_scaled_bytes>: u64,
        pub committed_fractional_scaled_bytes <committed_fractional_scaled_bytes>: u64,
        pub populated_fractional_scaled_bytes <populated_fractional_scaled_bytes>: u64,
    }
);

impl VmoInfo {
    pub fn cache_policy(&self) -> CachePolicy {
        CachePolicy::from(self.cache_policy)
    }
}

impl Default for VmoInfo {
    fn default() -> VmoInfo {
        Self::from(sys::zx_info_vmo_t::default())
    }
}

impl From<sys::zx_info_vmo_t> for VmoInfo {
    fn from(info: sys::zx_info_vmo_t) -> VmoInfo {
        zerocopy::transmute!(info)
    }
}

struct VmoInfoQuery;
unsafe impl ObjectQuery for VmoInfoQuery {
    const TOPIC: Topic = Topic::VMO;
    type InfoTy = sys::zx_info_vmo_t;
}

impl Vmo {
    /// Create a virtual memory object.
    ///
    /// Wraps the
    /// `zx_vmo_create`
    /// syscall. See the
    /// [Shared Memory: Virtual Memory Objects (VMOs)](https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts#shared_memory_virtual_memory_objects_vmos)
    /// for more information.
    pub fn create(size: u64) -> Result<Vmo, Status> {
        Vmo::create_with_opts(VmoOptions::from_bits_truncate(0), size)
    }

    /// Create a virtual memory object with options.
    ///
    /// Wraps the
    /// `zx_vmo_create`
    /// syscall, allowing options to be passed.
    pub fn create_with_opts(opts: VmoOptions, size: u64) -> Result<Vmo, Status> {
        let mut handle = 0;
        let status = unsafe { sys::zx_vmo_create(size, opts.bits(), &mut handle) };
        ok(status)?;
        unsafe { Ok(Vmo::from(NullableHandle::from_raw(handle))) }
    }

    /// Create a physically contiguous virtual memory object.
    ///
    /// Wraps the
    /// [`zx_vmo_create_contiguous`](https://fuchsia.dev/fuchsia-src/reference/syscalls/vmo_create_contiguous) syscall.
    pub fn create_contiguous(bti: &Bti, size: usize, alignment_log2: u32) -> Result<Vmo, Status> {
        let mut vmo_handle = sys::zx_handle_t::default();
        let status = unsafe {
            // SAFETY: regular system call with no unsafe parameters.
            sys::zx_vmo_create_contiguous(bti.raw_handle(), size, alignment_log2, &mut vmo_handle)
        };
        ok(status)?;
        unsafe {
            // SAFETY: The syscall docs claim that upon success, vmo_handle will be a valid
            // handle to a virtual memory object.
            Ok(Vmo::from(NullableHandle::from_raw(vmo_handle)))
        }
    }

    /// Read from a virtual memory object.
    ///
    /// Wraps the `zx_vmo_read` syscall.
    pub fn read(&self, data: &mut [u8], offset: u64) -> Result<(), Status> {
        unsafe {
            let status = sys::zx_vmo_read(self.raw_handle(), data.as_mut_ptr(), offset, data.len());
            ok(status)
        }
    }

    /// Provides a very thin wrapper over `zx_vmo_read`.
    ///
    /// # Safety
    ///
    /// Callers must guarantee that the buffer is valid to write to.
    pub unsafe fn read_raw<T: FromBytes>(
        &self,
        buffer: *mut T,
        buffer_length: usize,
        offset: u64,
    ) -> Result<(), Status> {
        let status = unsafe {
            sys::zx_vmo_read(
                self.raw_handle(),
                buffer.cast::<u8>(),
                offset,
                buffer_length * std::mem::size_of::<T>(),
            )
        };
        ok(status)
    }

    /// Same as read, but reads into memory that might not be initialized, returning an initialized
    /// slice of bytes on success.
    ///
    /// `Copy` is required to ensure that there are no custom `Drop` implementations. It is
    /// difficult to correctly run custom drop code after initializing a `MaybeUninit`.
    pub fn read_uninit<'a, T: Copy + FromBytes>(
        &self,
        data: &'a mut [MaybeUninit<T>],
        offset: u64,
    ) -> Result<&'a mut [T], Status> {
        // SAFETY: This system call requires that the pointer and length we pass are valid to write
        // to, which we guarantee here by getting the pointer and length from a valid slice.
        unsafe {
            self.read_raw(
                // TODO(https://fxbug.dev/42079723) use MaybeUninit::slice_as_mut_ptr when stable
                data.as_mut_ptr().cast::<T>(),
                data.len(),
                offset,
            )?
        }
        // TODO(https://fxbug.dev/42079723) use MaybeUninit::slice_assume_init_mut when stable
        Ok(
            // SAFETY: We're converting &mut [MaybeUninit<u8>] back to &mut [u8], which is only
            // valid to do if all elements of `data` have actually been initialized. Here we
            // have to trust that the kernel didn't lie when it said it wrote to the entire
            // buffer, but as long as that assumption is valid them it's safe to assume this
            // slice is init.
            unsafe { std::slice::from_raw_parts_mut(data.as_mut_ptr().cast::<T>(), data.len()) },
        )
    }

    /// Same as read, but returns a Vec.
    pub fn read_to_vec<T: Copy + FromBytes>(
        &self,
        offset: u64,
        length: u64,
    ) -> Result<Vec<T>, Status> {
        let len = length.try_into().map_err(|_| Status::INVALID_ARGS)?;
        let mut buffer = Vec::with_capacity(len);
        self.read_uninit(buffer.spare_capacity_mut(), offset)?;
        unsafe {
            // SAFETY: since read_uninit succeeded we know that we can consider the buffer
            // initialized.
            buffer.set_len(len);
        }
        Ok(buffer)
    }

    /// Same as read, but returns an array.
    pub fn read_to_array<T: Copy + FromBytes, const N: usize>(
        &self,
        offset: u64,
    ) -> Result<[T; N], Status> {
        // TODO(https://fxbug.dev/42079731): replace with MaybeUninit::uninit_array.
        let array: MaybeUninit<[MaybeUninit<T>; N]> = MaybeUninit::uninit();
        // SAFETY: We are converting from an uninitialized array to an array
        // of uninitialized elements which is the same. See
        // https://doc.rust-lang.org/std/mem/union.MaybeUninit.html#initializing-an-array-element-by-element.
        let mut array = unsafe { array.assume_init() };

        // SAFETY: T is FromBytes, which means that any bit pattern is valid. Interpreting
        // [MaybeUninit<T>] as [MaybeUninit<u8>] is safe because T's alignment requirements
        // are larger than u8.
        //
        // TODO(https://fxbug.dev/42079727): Use MaybeUninit::slice_as_bytes_mut once stable.
        let buffer = unsafe {
            std::slice::from_raw_parts_mut(
                array.as_mut_ptr().cast::<MaybeUninit<u8>>(),
                N * std::mem::size_of::<T>(),
            )
        };

        self.read_uninit(buffer, offset)?;
        // SAFETY: This is safe because we have initialized all the elements in
        // the array (since `read_uninit` returned successfully).
        //
        // TODO(https://fxbug.dev/42079725): replace with MaybeUninit::array_assume_init.
        let buffer = array.map(|a| unsafe { a.assume_init() });
        Ok(buffer)
    }

    /// Same as read, but returns a `T`.
    pub fn read_to_object<T: Copy + FromBytes>(&self, offset: u64) -> Result<T, Status> {
        let mut object = MaybeUninit::<T>::uninit();
        // SAFETY: T is FromBytes, which means that any bit pattern is valid. Interpreting
        // MaybeUninit<T> as [MaybeUninit<u8>] is safe because T's alignment requirements
        // are larger than, or equal to, u8's.
        //
        // TODO(https://fxbug.dev/42079727): Use MaybeUninit::as_bytes_mut once stable.
        let buffer = unsafe {
            std::slice::from_raw_parts_mut(
                object.as_mut_ptr().cast::<MaybeUninit<u8>>(),
                std::mem::size_of::<T>(),
            )
        };
        self.read_uninit(buffer, offset)?;

        // SAFETY: The call to `read_uninit` succeeded so we know that `object`
        // has been initialized.
        let object = unsafe { object.assume_init() };
        Ok(object)
    }

    /// Write to a virtual memory object.
    ///
    /// Wraps the `zx_vmo_write` syscall.
    pub fn write(&self, data: &[u8], offset: u64) -> Result<(), Status> {
        unsafe {
            let status = sys::zx_vmo_write(self.raw_handle(), data.as_ptr(), offset, data.len());
            ok(status)
        }
    }

    /// Efficiently transfers data from one VMO to another.
    pub fn transfer_data(
        &self,
        options: TransferDataOptions,
        offset: u64,
        length: u64,
        src_vmo: &Vmo,
        src_offset: u64,
    ) -> Result<(), Status> {
        let status = unsafe {
            sys::zx_vmo_transfer_data(
                self.raw_handle(),
                options.bits(),
                offset,
                length,
                src_vmo.raw_handle(),
                src_offset,
            )
        };
        ok(status)
    }

    /// Get the size of a virtual memory object.
    ///
    /// Wraps the `zx_vmo_get_size` syscall.
    pub fn get_size(&self) -> Result<u64, Status> {
        let mut size = 0;
        let status = unsafe { sys::zx_vmo_get_size(self.raw_handle(), &mut size) };
        ok(status).map(|()| size)
    }

    /// Attempt to change the size of a virtual memory object.
    ///
    /// Wraps the `zx_vmo_set_size` syscall.
    pub fn set_size(&self, size: u64) -> Result<(), Status> {
        let status = unsafe { sys::zx_vmo_set_size(self.raw_handle(), size) };
        ok(status)
    }

    /// Get the stream size of a virtual memory object.
    ///
    /// Wraps the `zx_vmo_get_stream_size` syscall.
    pub fn get_stream_size(&self) -> Result<u64, Status> {
        let mut size = 0;
        let status = unsafe { sys::zx_vmo_get_stream_size(self.raw_handle(), &mut size) };
        ok(status).map(|()| size)
    }

    /// Attempt to set the stream size of a virtual memory object.
    ///
    /// Wraps the `zx_vmo_set_stream_size` syscall.
    pub fn set_stream_size(&self, size: u64) -> Result<(), Status> {
        let status = unsafe { sys::zx_vmo_set_stream_size(self.raw_handle(), size) };
        ok(status)
    }

    /// Attempt to change the cache policy of a virtual memory object.
    ///
    /// Wraps the `zx_vmo_set_cache_policy` syscall.
    pub fn set_cache_policy(&self, cache_policy: CachePolicy) -> Result<(), Status> {
        let status =
            unsafe { sys::zx_vmo_set_cache_policy(self.raw_handle(), cache_policy as u32) };
        ok(status)
    }

    /// Perform an operation on a range of a virtual memory object.
    ///
    /// Wraps the
    /// [zx_vmo_op_range](https://fuchsia.dev/fuchsia-src/reference/syscalls/vmo_op_range.md)
    /// syscall.
    pub fn op_range(&self, op: VmoOp, offset: u64, size: u64) -> Result<(), Status> {
        let status = unsafe {
            sys::zx_vmo_op_range(self.raw_handle(), op.into_raw(), offset, size, ptr::null_mut(), 0)
        };
        ok(status)
    }

    /// Wraps the [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_VMO topic.
    pub fn info(&self) -> Result<VmoInfo, Status> {
        Ok(VmoInfo::from(self.0.get_info_single::<VmoInfoQuery>()?))
    }

    /// Create a new virtual memory object that clones a range of this one.
    ///
    /// Wraps the
    /// [zx_vmo_create_child](https://fuchsia.dev/fuchsia-src/reference/syscalls/vmo_create_child.md)
    /// syscall.
    pub fn create_child(
        &self,
        opts: VmoChildOptions,
        offset: u64,
        size: u64,
    ) -> Result<Vmo, Status> {
        let mut out = 0;
        let status = unsafe {
            sys::zx_vmo_create_child(self.raw_handle(), opts.bits(), offset, size, &mut out)
        };
        ok(status)?;
        unsafe { Ok(Vmo::from(NullableHandle::from_raw(out))) }
    }

    /// Replace a VMO, adding execute rights.
    ///
    /// Wraps the
    /// [zx_vmo_replace_as_executable](https://fuchsia.dev/fuchsia-src/reference/syscalls/vmo_replace_as_executable.md)
    /// syscall.
    pub fn replace_as_executable(self, vmex: &Resource) -> Result<Vmo, Status> {
        let mut out = 0;
        let status = unsafe {
            sys::zx_vmo_replace_as_executable(self.raw_handle(), vmex.raw_handle(), &mut out)
        };
        // zx_vmo_replace_as_executable always invalidates the passed in handle
        // so we need to forget 'self' without executing its drop which will attempt
        // to close the now-invalid handle value.
        std::mem::forget(self);
        ok(status)?;
        unsafe { Ok(Vmo::from(NullableHandle::from_raw(out))) }
    }
}

bitflags! {
    /// Options that may be used when creating a `Vmo`.
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct VmoOptions: u32 {
        const RESIZABLE = sys::ZX_VMO_RESIZABLE;
        const TRAP_DIRTY = sys::ZX_VMO_TRAP_DIRTY;
        const UNBOUNDED = sys::ZX_VMO_UNBOUNDED;
    }
}

/// Flags that may be set when receiving info on a `Vmo`.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, Immutable)]
pub struct VmoInfoFlags(u32);

bitflags! {
    impl VmoInfoFlags : u32 {
        const PAGED = sys::ZX_INFO_VMO_TYPE_PAGED;
        const RESIZABLE = sys::ZX_INFO_VMO_RESIZABLE;
        const IS_COW_CLONE = sys::ZX_INFO_VMO_IS_COW_CLONE;
        const VIA_HANDLE = sys::ZX_INFO_VMO_VIA_HANDLE;
        const VIA_MAPPING = sys::ZX_INFO_VMO_VIA_MAPPING;
        const PAGER_BACKED = sys::ZX_INFO_VMO_PAGER_BACKED;
        const CONTIGUOUS = sys::ZX_INFO_VMO_CONTIGUOUS;
        const DISCARDABLE = sys::ZX_INFO_VMO_DISCARDABLE;
        const IMMUTABLE = sys::ZX_INFO_VMO_IMMUTABLE;
        const VIA_IOB_HANDLE = sys::ZX_INFO_VMO_VIA_IOB_HANDLE;
    }
}

impl std::fmt::Debug for VmoInfoFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        bitflags::parser::to_writer(self, f)
    }
}

bitflags! {
    /// Options that may be used when creating a `Vmo` child.
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct VmoChildOptions: u32 {
        const SNAPSHOT = sys::ZX_VMO_CHILD_SNAPSHOT;
        const SNAPSHOT_AT_LEAST_ON_WRITE = sys::ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE;
        const RESIZABLE = sys::ZX_VMO_CHILD_RESIZABLE;
        const SLICE = sys::ZX_VMO_CHILD_SLICE;
        const NO_WRITE = sys::ZX_VMO_CHILD_NO_WRITE;
        const REFERENCE = sys::ZX_VMO_CHILD_REFERENCE;
        const SNAPSHOT_MODIFIED = sys::ZX_VMO_CHILD_SNAPSHOT_MODIFIED;
    }
}

bitflags! {
    /// Options that may be used when transferring data between VMOs.
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct TransferDataOptions: u32 {
    }
}

/// VM Object opcodes
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct VmoOp(u32);
impl VmoOp {
    pub fn from_raw(raw: u32) -> VmoOp {
        VmoOp(raw)
    }
    pub fn into_raw(self) -> u32 {
        self.0
    }
}

// VM Object Cache Policies.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
pub enum CachePolicy {
    Cached = sys::ZX_CACHE_POLICY_CACHED,
    UnCached = sys::ZX_CACHE_POLICY_UNCACHED,
    UnCachedDevice = sys::ZX_CACHE_POLICY_UNCACHED_DEVICE,
    WriteCombining = sys::ZX_CACHE_POLICY_WRITE_COMBINING,
    Unknown = u32::MAX,
}

impl From<u32> for CachePolicy {
    fn from(v: u32) -> Self {
        match v {
            sys::ZX_CACHE_POLICY_CACHED => CachePolicy::Cached,
            sys::ZX_CACHE_POLICY_UNCACHED => CachePolicy::UnCached,
            sys::ZX_CACHE_POLICY_UNCACHED_DEVICE => CachePolicy::UnCachedDevice,
            sys::ZX_CACHE_POLICY_WRITE_COMBINING => CachePolicy::WriteCombining,
            _ => CachePolicy::Unknown,
        }
    }
}

impl Into<u32> for CachePolicy {
    fn into(self) -> u32 {
        match self {
            CachePolicy::Cached => sys::ZX_CACHE_POLICY_CACHED,
            CachePolicy::UnCached => sys::ZX_CACHE_POLICY_UNCACHED,
            CachePolicy::UnCachedDevice => sys::ZX_CACHE_POLICY_UNCACHED_DEVICE,
            CachePolicy::WriteCombining => sys::ZX_CACHE_POLICY_WRITE_COMBINING,
            CachePolicy::Unknown => u32::MAX,
        }
    }
}

assoc_values!(VmoOp, [
    COMMIT =           sys::ZX_VMO_OP_COMMIT;
    DECOMMIT =         sys::ZX_VMO_OP_DECOMMIT;
    LOCK =             sys::ZX_VMO_OP_LOCK;
    UNLOCK =           sys::ZX_VMO_OP_UNLOCK;
    CACHE_SYNC =       sys::ZX_VMO_OP_CACHE_SYNC;
    CACHE_INVALIDATE = sys::ZX_VMO_OP_CACHE_INVALIDATE;
    CACHE_CLEAN =      sys::ZX_VMO_OP_CACHE_CLEAN;
    CACHE_CLEAN_INVALIDATE = sys::ZX_VMO_OP_CACHE_CLEAN_INVALIDATE;
    ZERO =             sys::ZX_VMO_OP_ZERO;
    TRY_LOCK =         sys::ZX_VMO_OP_TRY_LOCK;
    DONT_NEED =        sys::ZX_VMO_OP_DONT_NEED;
    ALWAYS_NEED =      sys::ZX_VMO_OP_ALWAYS_NEED;
    PREFETCH =         sys::ZX_VMO_OP_PREFETCH;
]);

unsafe_handle_properties!(object: Vmo,
    props: [
        {query_ty: VMO_CONTENT_SIZE, tag: VmoContentSizeTag, prop_ty: u64, get:get_content_size, set: set_content_size},
    ]
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Iommu, IommuDescStub, ObjectType};
    use fidl_fuchsia_kernel as fkernel;
    use fuchsia_component::client::connect_channel_to_protocol;
    use test_case::test_case;
    use zerocopy::KnownLayout;

    #[test]
    fn vmo_create_contiguous() {
        use zx::{Channel, MonotonicInstant};
        let (client_end, server_end) = Channel::create();
        connect_channel_to_protocol::<fkernel::IommuResourceMarker>(server_end).unwrap();
        let service = fkernel::IommuResourceSynchronousProxy::new(client_end);
        let resource =
            service.get(MonotonicInstant::INFINITE).expect("couldn't get iommu resource");
        // This test and fuchsia-zircon are different crates, so we need
        // to use from_raw to convert between the zx handle and this test handle.
        // See https://fxbug.dev/42173139 for details.
        let resource = unsafe { Resource::from(NullableHandle::from_raw(resource.into_raw())) };
        let iommu = Iommu::create_stub(&resource, IommuDescStub::default()).unwrap();
        let bti = Bti::create(&iommu, 0).unwrap();

        let vmo = Vmo::create_contiguous(&bti, 8192, 0).unwrap();
        let info = vmo.as_handle_ref().basic_info().unwrap();
        assert_eq!(info.object_type, ObjectType::VMO);

        let vmo_info = vmo.info().unwrap();
        assert!(vmo_info.flags.contains(VmoInfoFlags::CONTIGUOUS));
    }

    #[test]
    fn vmo_get_size() {
        let size = 16 * 1024 * 1024;
        let vmo = Vmo::create(size).unwrap();
        assert_eq!(size, vmo.get_size().unwrap());
    }

    #[test]
    fn vmo_set_size() {
        // Use a multiple of page size to match VMOs page aligned size
        let start_size = 4096;
        let vmo = Vmo::create_with_opts(VmoOptions::RESIZABLE, start_size).unwrap();
        assert_eq!(start_size, vmo.get_size().unwrap());

        // Change the size and make sure the new size is reported
        let new_size = 8192;
        assert!(vmo.set_size(new_size).is_ok());
        assert_eq!(new_size, vmo.get_size().unwrap());
    }

    #[test]
    fn vmo_get_info_default() {
        let size = 4096;
        let vmo = Vmo::create(size).unwrap();
        let info = vmo.info().unwrap();
        assert!(!info.flags.contains(VmoInfoFlags::PAGER_BACKED));
        assert!(info.flags.contains(VmoInfoFlags::PAGED));
    }

    #[test]
    fn vmo_get_child_info() {
        let size = 4096;
        let vmo = Vmo::create(size).unwrap();
        let info = vmo.info().unwrap();
        assert!(!info.flags.contains(VmoInfoFlags::IS_COW_CLONE));

        let child = vmo.create_child(VmoChildOptions::SNAPSHOT, 0, 512).unwrap();
        let info = child.info().unwrap();
        assert!(info.flags.contains(VmoInfoFlags::IS_COW_CLONE));

        let child = vmo.create_child(VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE, 0, 512).unwrap();
        let info = child.info().unwrap();
        assert!(info.flags.contains(VmoInfoFlags::IS_COW_CLONE));

        let child = vmo.create_child(VmoChildOptions::SLICE, 0, 512).unwrap();
        let info = child.info().unwrap();
        assert!(!info.flags.contains(VmoInfoFlags::IS_COW_CLONE));
    }

    #[test]
    fn vmo_set_size_fails_on_non_resizable() {
        let size = 4096;
        let vmo = Vmo::create(size).unwrap();
        assert_eq!(size, vmo.get_size().unwrap());

        let new_size = 8192;
        assert_eq!(Err(Status::UNAVAILABLE), vmo.set_size(new_size));
        assert_eq!(size, vmo.get_size().unwrap());
    }

    #[test_case(0)]
    #[test_case(1)]
    fn vmo_read_to_array(read_offset: usize) {
        const ACTUAL_SIZE: usize = 5;
        const ACTUAL: [u8; ACTUAL_SIZE] = [1, 2, 3, 4, 5];
        let vmo = Vmo::create(ACTUAL.len() as u64).unwrap();
        vmo.write(&ACTUAL, 0).unwrap();
        let read_len = ACTUAL_SIZE - read_offset;
        assert_eq!(
            &vmo.read_to_array::<u8, ACTUAL_SIZE>(read_offset as u64).unwrap()[..read_len],
            &ACTUAL[read_offset..]
        );
    }

    #[test_case(0)]
    #[test_case(1)]
    fn vmo_read_to_vec(read_offset: usize) {
        const ACTUAL_SIZE: usize = 4;
        const ACTUAL: [u8; ACTUAL_SIZE] = [6, 7, 8, 9];
        let vmo = Vmo::create(ACTUAL.len() as u64).unwrap();
        vmo.write(&ACTUAL, 0).unwrap();
        let read_len = ACTUAL_SIZE - read_offset;
        assert_eq!(
            &vmo.read_to_vec::<u8>(read_offset as u64, read_len as u64).unwrap(),
            &ACTUAL[read_offset..]
        );
    }

    #[test_case(0)]
    #[test_case(1)]
    fn vmo_read_to_object(read_offset: usize) {
        #[repr(C)]
        #[derive(Copy, Clone, Debug, Eq, KnownLayout, FromBytes, PartialEq)]
        struct Object {
            a: u8,
            b: u8,
        }

        const ACTUAL_SIZE: usize = std::mem::size_of::<Object>();
        const ACTUAL: [u8; ACTUAL_SIZE + 1] = [10, 11, 12];
        let vmo = Vmo::create(ACTUAL.len() as u64).unwrap();
        vmo.write(&ACTUAL, 0).unwrap();
        assert_eq!(
            vmo.read_to_object::<Object>(read_offset as u64).unwrap(),
            Object { a: ACTUAL[read_offset], b: ACTUAL[1 + read_offset] }
        );
    }

    #[test]
    fn vmo_read_write() {
        let mut vec1 = vec![0; 16];
        let vmo = Vmo::create(4096 as u64).unwrap();
        assert!(vmo.write(b"abcdef", 0).is_ok());
        assert!(vmo.read(&mut vec1, 0).is_ok());
        assert_eq!(b"abcdef", &vec1[0..6]);
        assert!(vmo.write(b"123", 2).is_ok());
        assert!(vmo.read(&mut vec1, 0).is_ok());
        assert_eq!(b"ab123f", &vec1[0..6]);

        // Read one byte into the vmo.
        assert!(vmo.read(&mut vec1, 1).is_ok());
        assert_eq!(b"b123f", &vec1[0..5]);

        assert_eq!(&vmo.read_to_vec::<u8>(0, 6).expect("read_to_vec failed"), b"ab123f");
    }

    #[test]
    fn vmo_child_snapshot() {
        let size = 4096 * 2;
        let vmo = Vmo::create(size).unwrap();

        vmo.write(&[1; 4096], 0).unwrap();
        vmo.write(&[2; 4096], 4096).unwrap();

        let child = vmo.create_child(VmoChildOptions::SNAPSHOT, 0, size).unwrap();

        child.write(&[3; 4096], 0).unwrap();

        vmo.write(&[4; 4096], 0).unwrap();
        vmo.write(&[5; 4096], 4096).unwrap();

        let mut page = [0; 4096];

        // SNAPSHOT child observes no further changes to parent VMO.
        child.read(&mut page[..], 0).unwrap();
        assert_eq!(&page[..], &[3; 4096][..]);
        child.read(&mut page[..], 4096).unwrap();
        assert_eq!(&page[..], &[2; 4096][..]);
    }

    #[test]
    fn vmo_child_snapshot_at_least_on_write() {
        let size = 4096 * 2;
        let vmo = Vmo::create(size).unwrap();

        vmo.write(&[1; 4096], 0).unwrap();
        vmo.write(&[2; 4096], 4096).unwrap();

        let child = vmo.create_child(VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE, 0, size).unwrap();

        child.write(&[3; 4096], 0).unwrap();

        vmo.write(&[4; 4096], 0).unwrap();
        vmo.write(&[5; 4096], 4096).unwrap();

        let mut page = [0; 4096];

        // SNAPSHOT_AT_LEAST_ON_WRITE child may observe changes to pages it has not yet written to,
        // but such behavior is not guaranteed.
        child.read(&mut page[..], 0).unwrap();
        assert_eq!(&page[..], &[3; 4096][..]);
        child.read(&mut page[..], 4096).unwrap();
        assert!(
            &page[..] == &[2; 4096][..] || &page[..] == &[5; 4096][..],
            "expected page of 2 or 5, got {:?}",
            &page[..]
        );
    }

    #[test]
    fn vmo_child_no_write() {
        let size = 4096;
        let vmo = Vmo::create(size).unwrap();
        vmo.write(&[1; 4096], 0).unwrap();

        let child =
            vmo.create_child(VmoChildOptions::SLICE | VmoChildOptions::NO_WRITE, 0, size).unwrap();
        assert_eq!(child.write(&[3; 4096], 0), Err(Status::ACCESS_DENIED));
    }

    #[test]
    fn vmo_op_range_unsupported() {
        let vmo = Vmo::create(12).unwrap();
        assert_eq!(vmo.op_range(VmoOp::LOCK, 0, 1), Err(Status::NOT_SUPPORTED));
        assert_eq!(vmo.op_range(VmoOp::UNLOCK, 0, 1), Err(Status::NOT_SUPPORTED));
    }

    #[test]
    fn vmo_cache() {
        let vmo = Vmo::create(12).unwrap();

        // Cache operations should all succeed.
        assert_eq!(vmo.op_range(VmoOp::CACHE_SYNC, 0, 12), Ok(()));
        assert_eq!(vmo.op_range(VmoOp::CACHE_INVALIDATE, 0, 12), Ok(()));
        assert_eq!(vmo.op_range(VmoOp::CACHE_CLEAN, 0, 12), Ok(()));
        assert_eq!(vmo.op_range(VmoOp::CACHE_CLEAN_INVALIDATE, 0, 12), Ok(()));
    }

    #[test]
    fn vmo_create_child() {
        let original = Vmo::create(16).unwrap();
        assert!(original.write(b"one", 0).is_ok());

        // Clone the VMO, and make sure it contains what we expect.
        let clone =
            original.create_child(VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE, 0, 16).unwrap();
        let mut read_buffer = vec![0; 16];
        assert!(clone.read(&mut read_buffer, 0).is_ok());
        assert_eq!(&read_buffer[0..3], b"one");

        // Writing to the original will not affect the clone.
        assert!(original.write(b"two", 0).is_ok());
        assert!(original.read(&mut read_buffer, 0).is_ok());
        assert_eq!(&read_buffer[0..3], b"two");
        assert!(clone.read(&mut read_buffer, 0).is_ok());
        assert_eq!(&read_buffer[0..3], b"one");

        // However, writing to the clone will not affect the original.
        assert!(clone.write(b"three", 0).is_ok());
        assert!(original.read(&mut read_buffer, 0).is_ok());
        assert_eq!(&read_buffer[0..3], b"two");
        assert!(clone.read(&mut read_buffer, 0).is_ok());
        assert_eq!(&read_buffer[0..5], b"three");
    }

    #[test]
    fn vmo_replace_as_executeable() {
        use zx::{Channel, MonotonicInstant};

        let vmo = Vmo::create(16).unwrap();

        let info = vmo.as_handle_ref().basic_info().unwrap();
        assert!(!info.rights.contains(Rights::EXECUTE));

        let (client_end, server_end) = Channel::create();
        connect_channel_to_protocol::<fkernel::VmexResourceMarker>(server_end).unwrap();
        let service = fkernel::VmexResourceSynchronousProxy::new(client_end);
        let resource = service.get(MonotonicInstant::INFINITE).expect("couldn't get vmex resource");
        let resource =
            unsafe { crate::Resource::from(NullableHandle::from_raw(resource.into_raw())) };

        let exec_vmo = vmo.replace_as_executable(&resource).unwrap();
        let info = exec_vmo.as_handle_ref().basic_info().unwrap();
        assert!(info.rights.contains(Rights::EXECUTE));
    }

    #[test]
    fn vmo_content_size() {
        let start_size = 1024;
        let vmo = Vmo::create_with_opts(VmoOptions::RESIZABLE, start_size).unwrap();
        assert_eq!(vmo.get_content_size().unwrap(), start_size);
        vmo.set_content_size(&0).unwrap();
        assert_eq!(vmo.get_content_size().unwrap(), 0);

        // write should not change content size.
        let content = b"abcdef";
        assert!(vmo.write(content, 0).is_ok());
        assert_eq!(vmo.get_content_size().unwrap(), 0);
    }

    #[test]
    fn vmo_zero() {
        let vmo = Vmo::create(16).unwrap();
        let content = b"0123456789abcdef";
        assert!(vmo.write(content, 0).is_ok());
        let mut buf = vec![0u8; 16];
        assert!(vmo.read(&mut buf[..], 0).is_ok());
        assert_eq!(&buf[..], content);

        assert!(vmo.op_range(VmoOp::ZERO, 0, 16).is_ok());
        assert!(vmo.read(&mut buf[..], 0).is_ok());
        assert_eq!(&buf[..], &[0u8; 16]);
    }

    #[test]
    fn vmo_stream_size() {
        let start_size = 1300;
        let vmo = Vmo::create_with_opts(VmoOptions::UNBOUNDED, start_size).unwrap();
        assert_eq!(vmo.get_stream_size().unwrap(), start_size);
        vmo.set_stream_size(0).unwrap();
        assert_eq!(vmo.get_stream_size().unwrap(), 0);

        // write should not change content size.
        let content = b"abcdef";
        assert!(vmo.write(content, 0).is_ok());
        assert_eq!(vmo.get_stream_size().unwrap(), 0);

        // stream size can also grow.
        let mut buf = vec![1; 6];
        vmo.set_stream_size(6).unwrap();
        assert!(vmo.read(&mut buf, 0).is_ok());
        // growing will zero new bytes.
        assert_eq!(buf, vec![0; 6]);
    }
}
