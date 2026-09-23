// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ptr::NonNull;

/// Provides a safe byte slice view of a VMO through a `Deref<Target=[u8]>` implementation.
#[derive(Debug)]
pub struct ImmutableMapping {
    inner: Inner,
}

#[derive(Debug)]
enum Inner {
    Empty,
    Vmar { addr: NonNull<u8>, size: usize },
}

impl ImmutableMapping {
    /// If `vmo.get_size()` is not zero:
    /// Creates an immutable child VMO (by setting ZX_VMO_CHILD_SNAPSHOT and ZX_VMO_CHILD_NO_WRITE)
    /// of `vmo`, then creates a non-resizeable mapping of this child VMO and uses the mapping to
    /// provide a `&[u8]` view of `vmo`'s contents at the time this function was called.
    ///
    /// * Writes to `vmo` after this function is called will not be reflected in the `&[u8]` view.
    /// * `vmo` must have ZX_RIGHT_DUPLICATE and ZX_RIGHT_READ.
    /// * If `immediately_page` is true, ZX_VM_MAP_RANGE will be set when the mapping is created.
    ///
    /// If `vmo.get_size()` is zero:
    /// No VMOs will be created or mapped.
    /// The `&[u8]` view will be backed by a static empty array.
    /// This is for convenience to callers, as zx_vmar_map does not allow zero size mappings.
    pub fn create_from_vmo(vmo: &zx::Vmo, immediately_page: bool) -> Result<Self, Error> {
        let size = vmo.get_size().map_err(Error::GetVmoSize)?;
        if size == 0 {
            return Ok(Self { inner: Inner::Empty });
        }
        // std::slice::from_raw_parts cannot be called with more than isize::MAX bytes.
        if let Err(source) = isize::try_from(size) {
            return Err(Error::VmoSizeAsIsize { source, size });
        }
        let child = vmo
            .create_child(zx::VmoChildOptions::SNAPSHOT | zx::VmoChildOptions::NO_WRITE, 0, size)
            .map_err(Error::CreateChildVmo)?;
        let size: usize =
            size.try_into().map_err(|source| Error::VmoSizeAsUsize { source, size })?;
        let addr = fuchsia_runtime::vmar_root_self()
            .map(
                0,
                &child,
                0,
                size,
                // ZX_VM_ALLOW_FAULTS is not necessary because child is not resizable, the mapping
                // does not extend past the end of child (child and the mapping have the same size),
                // child is not discardable, and child was not created with zx_pager_create_vmo.
                zx::VmarFlags::REQUIRE_NON_RESIZABLE
                    | zx::VmarFlags::PERM_READ
                    | if immediately_page {
                        zx::VmarFlags::MAP_RANGE
                    } else {
                        zx::VmarFlags::empty()
                    },
            )
            .map_err(Error::MapChildVmo)?;
        // This should never fail, zx_vmar_map always returns non-zero.
        let addr = NonNull::new(addr as *mut u8).ok_or(Error::VmarMapReturnedNull)?;
        Ok(Self { inner: Inner::Vmar { addr, size } })
    }
}

impl std::ops::Deref for ImmutableMapping {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        match &self.inner {
            Inner::Empty => &[],
            Inner::Vmar { addr, size } => {
                // Safety:
                //
                // The entire range of the slice is contained within a single allocated object, the
                // memory mapping, which has the same size as the slice. The mapping is not
                // resizable.
                //
                // zx_vmar_map, the syscall behind `fuchsia_runtime::vmar_root_self().map()`
                // guarantees that addr is non-null and page aligned, and because T is u8 there is
                // no alignment requirement. Additionally, addr is checked to be non-null when the
                // ImmutableMapping is created. If size were zero we would take the preceding Empty
                // branch.
                //
                // The slice contents are initialized to the contents of the original VMO at the
                // time that the snapshot was taken. The contents will always be valid instances
                // for T = u8.
                //
                // The memory referenced by the returned slice will never be mutated. The mapped
                // child vmo is created with ZX_VMO_CHILD_SNAPSHOT and ZX_VMO_CHILD_NO_WRITE. The
                // mapping is created with ZX_VM_REQUIRE_NON_RESIZABLE and only ZX_VM_PERM_READ.
                // Also, at this point all the handles to the child vmo have been dropped and it is
                // kept alive only by the mapping.
                //
                // Self::create_from_vmo guarantees that size <= isize::MAX and zx_vmar_map
                // guarantees that addr + size will not wrap around the address space.
                let addr = addr.as_ptr();
                unsafe { std::slice::from_raw_parts(addr, *size) }
            }
        }
    }
}

impl Drop for ImmutableMapping {
    fn drop(&mut self) {
        match self.inner {
            Inner::Empty => (),
            Inner::Vmar { addr, size } => {
                // Safety:
                //
                // The memory pointed to by addr, aka the memory of the mapping we are about to
                // unmap, is only accessible via the Deref impl on this type. This fn, drop, takes
                // a mutable reference to self, so at this point there can not be any outstanding
                // references obtained from <Self as Deref>::deref(&self).
                //
                // The child vmo and mapping were both created with only the read permission, so
                // there cannot be any executable code relying on it.
                let addr = addr.as_ptr() as usize;
                unsafe {
                    let _: Result<(), zx::Status> =
                        fuchsia_runtime::vmar_root_self().unmap(addr, size);
                }
            }
        }
    }
}

/// Error type for `ImmutableMapping::create_from_vmo`.
#[allow(missing_docs)]
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to get VMO size")]
    GetVmoSize(#[source] zx::Status),

    #[error("failed to convert VMO size {size} to isize")]
    VmoSizeAsIsize { source: std::num::TryFromIntError, size: u64 },

    #[error("failed to create child VMO")]
    CreateChildVmo(#[source] zx::Status),

    #[error("failed to convert VMO size {size} to usize")]
    VmoSizeAsUsize { source: std::num::TryFromIntError, size: u64 },

    #[error("failed to map child VMO")]
    MapChildVmo(#[source] zx::Status),

    #[error("zx_vmar_map returned a base address of zero")]
    VmarMapReturnedNull,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use test_case::test_case;

    #[test_case(true; "immediately-page")]
    #[test_case(false; "do-not-immediately-page")]
    fn read(immediately_page: bool) {
        let vmo = zx::Vmo::create(1).unwrap();
        let () = vmo.write(b"content", 0).unwrap();

        let mapping = ImmutableMapping::create_from_vmo(&vmo, immediately_page).unwrap();

        let mut expected = vec![0u8; zx::system_get_page_size().try_into().unwrap()];
        assert_eq!(expected.as_mut_slice().write(b"content").unwrap(), 7);

        assert_eq!(&mapping[..], expected.as_slice());
    }

    #[test_case(true; "immediately-page")]
    #[test_case(false; "do-not-immediately-page")]
    fn drop_cleans_up(immediately_page: bool) {
        let vmo = zx::Vmo::create(7).unwrap();
        assert!(
            vmo.wait_one(zx::Signals::VMO_ZERO_CHILDREN, zx::MonotonicInstant::INFINITE_PAST)
                .is_ok()
        );

        let mapping = ImmutableMapping::create_from_vmo(&vmo, immediately_page).unwrap();
        assert!(
            vmo.wait_one(zx::Signals::VMO_ZERO_CHILDREN, zx::MonotonicInstant::INFINITE_PAST)
                .is_err()
        );

        drop(mapping);
        assert!(
            vmo.wait_one(zx::Signals::VMO_ZERO_CHILDREN, zx::MonotonicInstant::INFINITE_PAST)
                .is_ok()
        );
    }

    #[test_case(true; "immediately-page")]
    #[test_case(false; "do-not-immediately-page")]
    fn empty(immediately_page: bool) {
        let vmo = zx::Vmo::create(0).unwrap();

        let mapping = ImmutableMapping::create_from_vmo(&vmo, immediately_page).unwrap();

        assert_eq!(*mapping, []);
    }
}
