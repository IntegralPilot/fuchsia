// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx_status::Status;

/// Trait for the backing storage of a raw bitmap.
pub trait Storage {
    /// True if this storage supports growing.
    const SUPPORTS_GROW: bool = false;

    /// Allocates at least `size` bytes of storage.
    fn allocate(&mut self, size: usize) -> Result<(), Status>;

    /// Returns a read-only slice of `usize` words to the underlying storage.
    fn get_data(&self) -> &[usize];

    /// Returns a mutable slice of `usize` words to the underlying storage.
    fn get_data_mut(&mut self) -> &mut [usize];

    /// Optionally grows the storage to at least `size` bytes.
    fn grow(&mut self, _size: usize) -> Result<(), Status> {
        Err(Status::NO_RESOURCES)
    }
}

pub struct DefaultStorage {
    storage: kalloc::Box<[usize]>,
}

impl core::fmt::Debug for DefaultStorage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DefaultStorage").field("storage", &self.storage.as_ref()).finish()
    }
}

impl Default for DefaultStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultStorage {
    /// Create a new, empty default storage.
    pub const fn new() -> Self {
        Self { storage: kalloc::Box::<[usize]>::empty_slice() }
    }
}

impl Storage for DefaultStorage {
    fn allocate(&mut self, size: usize) -> Result<(), Status> {
        let usize_size = core::mem::size_of::<usize>();
        let num_elements = size.div_ceil(usize_size);
        let new_storage = kalloc::Box::<[usize]>::try_new_zeroed_slice(num_elements)
            .map_err(|_| Status::NO_MEMORY)?;
        self.storage = new_storage;
        Ok(())
    }

    fn get_data(&self) -> &[usize] {
        &self.storage
    }

    fn get_data_mut(&mut self) -> &mut [usize] {
        &mut self.storage
    }
}

/// A fixed-size static bitmap storage.
///
/// `N` is the number of `usize` elements in the array.
#[derive(Debug)]
pub struct FixedStorage<const N: usize> {
    storage: [usize; N],
}

impl<const N: usize> Default for FixedStorage<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> FixedStorage<N> {
    /// Create a new, zero-initialized fixed storage.
    pub const fn new() -> Self {
        Self { storage: [0; N] }
    }
}

impl<const N: usize> Storage for FixedStorage<N> {
    fn allocate(&mut self, size: usize) -> Result<(), Status> {
        let usize_size = core::mem::size_of::<usize>();
        let required_elements = size.div_ceil(usize_size);
        if required_elements > N {
            return Err(Status::INVALID_ARGS);
        }
        Ok(())
    }

    fn get_data(&self) -> &[usize] {
        &self.storage
    }

    fn get_data_mut(&mut self) -> &mut [usize] {
        &mut self.storage
    }
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
mod userspace {
    use super::*;
    use fuchsia_runtime;

    // Helper to map a VMO with default read/write permissions at offset 0.
    fn map_vmo(vmar: &zx::Vmar, vmo: &zx::Vmo, size: usize) -> Result<usize, Status> {
        vmar.map(0, vmo, 0, size, zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE)
    }

    /// Userspace-only storage backed by a VMO (Virtual Memory Object).
    #[derive(Default, Debug)]
    pub struct VmoStorage {
        vmo: Option<zx::Vmo>,
        mapped_addr: usize,
        size: usize,
    }

    impl VmoStorage {
        /// Create a new, unallocated VmoStorage.
        pub const fn new() -> Self {
            Self { vmo: None, mapped_addr: 0, size: 0 }
        }

        fn release(&mut self) {
            if self.mapped_addr != 0 {
                let vmar = fuchsia_runtime::vmar_root_self();
                // SAFETY: We mapped this memory in `allocate` or `grow` and we own it.
                unsafe {
                    let _ = vmar.unmap(self.mapped_addr, self.size);
                }
            }
            self.mapped_addr = 0;
            self.size = 0;
            self.vmo = None;
        }

        /// Access the underlying VMO if allocated.
        pub fn get_vmo(&self) -> Option<&zx::Vmo> {
            self.vmo.as_ref()
        }
    }

    impl Drop for VmoStorage {
        fn drop(&mut self) {
            self.release();
        }
    }

    impl Storage for VmoStorage {
        const SUPPORTS_GROW: bool = true;

        fn allocate(&mut self, size: usize) -> Result<(), Status> {
            self.release();
            let page_size = zx::system_get_page_size() as usize;
            let rounded_size = (size + page_size - 1) & !(page_size - 1);
            let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, rounded_size as u64)?;
            let _ = vmo.set_name(&zx::Name::new_lossy("vmo-backed-bitmap"));

            let vmar = fuchsia_runtime::vmar_root_self();
            let mapped_addr = map_vmo(&vmar, &vmo, rounded_size)?;

            self.vmo = Some(vmo);
            self.mapped_addr = mapped_addr;
            self.size = rounded_size;
            Ok(())
        }

        fn get_data(&self) -> &[usize] {
            if self.mapped_addr == 0 {
                &[]
            } else {
                // SAFETY: `mapped_addr` is a valid address mapped from a VMO with `self.size` bytes.
                // VMO mappings are page-aligned (and thus aligned to `usize`). The size of the
                // slice in words is `self.size / size_of::<usize>()`.
                // We use `with_exposed_provenance` to construct a pointer with exposed provenance,
                // and then `from_raw_parts` to construct a slice of `usize`.
                unsafe {
                    let ptr = core::ptr::with_exposed_provenance::<usize>(self.mapped_addr);
                    core::slice::from_raw_parts(ptr, self.size / core::mem::size_of::<usize>())
                }
            }
        }

        fn get_data_mut(&mut self) -> &mut [usize] {
            if self.mapped_addr == 0 {
                &mut []
            } else {
                // SAFETY: `mapped_addr` is a valid address mapped from a VMO with `self.size` bytes.
                // VMO mappings are page-aligned (and thus aligned to `usize`). The size of the
                // slice in words is `self.size / size_of::<usize>()`.
                // We use `with_exposed_provenance_mut` to construct a mutable pointer with exposed provenance,
                // and then `from_raw_parts_mut` to construct a mutable slice of `usize`.
                unsafe {
                    let ptr = core::ptr::with_exposed_provenance_mut::<usize>(self.mapped_addr);
                    core::slice::from_raw_parts_mut(ptr, self.size / core::mem::size_of::<usize>())
                }
            }
        }

        fn grow(&mut self, size: usize) -> Result<(), Status> {
            if size <= self.size {
                return Ok(());
            }
            let page_size = zx::system_get_page_size() as usize;
            let rounded_size = (size + page_size - 1) & !(page_size - 1);
            let vmo = self.vmo.as_ref().ok_or(Status::BAD_STATE)?;
            vmo.set_size(rounded_size as u64)?;

            let vmar = fuchsia_runtime::vmar_root_self();
            let vmar_info = vmar.info()?;
            let extend_offset = self.mapped_addr + self.size - vmar_info.base;
            let extend_size = rounded_size - self.size;
            let map_result = vmar.map(
                extend_offset,
                vmo,
                self.size as u64,
                extend_size,
                zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE | zx::VmarFlags::SPECIFIC,
            );
            if map_result.is_ok() {
                self.size = rounded_size;
                return Ok(());
            }

            let new_addr = map_vmo(&vmar, vmo, rounded_size)?;
            // SAFETY: We mapped the old memory at `mapped_addr` and we are unmapping it.
            unsafe {
                let _ = vmar.unmap(self.mapped_addr, self.size);
            }
            self.mapped_addr = new_addr;
            self.size = rounded_size;
            Ok(())
        }
    }
}

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
pub use userspace::VmoStorage;
