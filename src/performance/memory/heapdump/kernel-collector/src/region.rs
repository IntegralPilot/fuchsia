// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Holds a shared memory mapping, wrapping a VMO.
pub struct Region {
    addr: usize,
    size: usize,
}

impl Region {
    pub fn new(vmo: &zx::Vmo) -> anyhow::Result<Region> {
        let size = vmo.info()?.size_bytes.try_into()?;
        Ok(Region {
            size,
            addr: fuchsia_runtime::vmar_root_self().map(
                0,
                &vmo,
                0,
                size,
                zx::VmarFlags::PERM_READ | zx::VmarFlags::REQUIRE_NON_RESIZABLE,
            )?,
        })
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// # Safety
    /// The caller must ensure that the generic type `T` meets two conditions:
    ///
    /// 1.  T has C-compatible layout, such as `#[repr(C)]`, or primitive types.
    /// 2.  Immutable values can be read directly, while mutable values must be read atomically.
    pub unsafe fn get<T>(&self, position: usize, count: usize) -> Option<&[T]> {
        // Calculate the total size needed.
        let total_size = std::mem::size_of::<T>() * count;
        // Check if enough bytes are available.
        if total_size <= self.size - position {
            let ptr = (self.addr + position) as *const T;
            // SAFETY: see std::slice::from_raw_parts documentation.
            // - ptr is non null and aligned, and contiguous.
            // - the returned memory reference is not mutated except inside an UnsafeCell.
            // - the range does not overshoot.
            assert!(ptr.align_offset(std::mem::align_of::<T>()) == 0, "Aligned access required");
            assert!(ptr != std::ptr::null_mut(), "nonnull ptr required");
            Some(unsafe { std::slice::from_raw_parts(ptr, count) })
        } else {
            None
        }
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY:
        // - The memory is not be accessed after unmapping, as it is only accessible via `Region`
        //   API.
        // - All references to the memory region are return by this Region with bound lifetime,
        //   hence borrow check guarantees that all references are dropped or forgotten.
        // - The region does not contained executable code.
        unsafe { fuchsia_runtime::vmar_root_self().unmap(self.addr, self.size) }
            .expect("unmap failed");
    }
}
