// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use std::sync::Arc;
use {fuchsia_runtime, zx};

// Allocate VMARs in 1GiB blocks. Even for the reduced size of the shared asapce 1GiB is not too
// wasteful and should still sufficiently minimize the number of allocations done.
const VMAR_SIZE: usize = 1 * 1024 * 1024 * 1024;
// We do not know the exact size of the allocations that libc will perform in the VMARs. To
// compensate we way over approximate the size of the probe. In the worst case this wastes 32MiB
// of a 1GiB vmar, which is a minor percentage.
const PROBE_SIZE: usize = 32 * 1024 * 1024;

/// Small wrapper around a zx::vmar for creating and probing a compact vmar.
///
/// Use `CompactVmar::new()` to create a new instance.
///
/// # Safety
///
/// This implements `Drop` and will explicitly destroy the underlying VMAR. Clients must ensure that
/// all mappings within this VMAR are cleared/unmapped before this object is dropped. Failure to do
/// so will trigger a panic.
#[derive(Debug)]
struct CompactVmar {
    vmar: Arc<zx::Vmar>,
}

impl CompactVmar {
    fn new() -> Option<CompactVmar> {
        fuchsia_runtime::vmar_root_self()
            .allocate(
                0,
                VMAR_SIZE,
                zx::VmarFlags::CAN_MAP_READ
                    | zx::VmarFlags::CAN_MAP_WRITE
                    | zx::VmarFlags::CAN_MAP_SPECIFIC
                    | zx::VmarFlags::COMPACT,
            )
            .ok()
            .map(|(vmar, _)| CompactVmar { vmar: Arc::new(vmar) })
    }
    /// See if this VMAR can support an allocation of the requested size.
    fn probe(&self, size: usize) -> bool {
        if let Ok((child, _addr)) = self.vmar.allocate(0, size, zx::VmarFlags::CAN_MAP_READ) {
            // SAFETY: just allocated this
            unsafe {
                let _ = child.destroy();
            }
            true
        } else {
            false
        }
    }
}

impl Drop for CompactVmar {
    fn drop(&mut self) {
        // Check that all child VMARs and mappings have been removed before destroying the VMARs.
        // We pass an empty buffer to `maps` to get the count of available maps in `avail`.
        let mut buf = [];
        match self.vmar.maps(&mut buf) {
            Err(e) => panic!("Failed to retrieve vmar maps info: {e}"),
            Ok((_, _, avail)) => {
                if avail != 1 {
                    panic!(
                        "Vmar has unexpected allocations (found {avail}). Destroying is likely to \
                        cause threads to crash"
                    );
                }
            }
        }

        // SAFETY: Just validated that there are no children (i.e. mappings).
        unsafe {
            let _ = self.vmar.destroy();
        }
    }
}

/// Implements a list of VMARs that grows, but never shrinks, to support additional allocations.
///
/// This does not actually track allocations and frees, but rather allows for performing a probe
/// just prior to allocation to find a VMAR that has space. Although probing is less efficient, this
/// is useful if the exact amounts allocated, and when they are freed, cannot be easily tracked.
#[derive(Debug, Default)]
pub struct GrowableVmars {
    vmars: std::collections::VecDeque<CompactVmar>,
}

impl GrowableVmars {
    /// Returns a handle to a VMAR that has at least `PROBE_SIZE` bytes of free space.
    ///
    /// The returned handle is suitable for passing to `thrd_set_zx_create_handles` via the
    /// `thrd_zx_create_handles_t` structure.
    pub fn probe(&mut self) -> Result<Arc<zx::Vmar>, Errno> {
        // Search out existing VMARs for one with free space. The full vmars get placed back on the
        // end of the list to ensure future searches will try them last.
        if let Some(index) = self.vmars.iter().position(|v| v.probe(PROBE_SIZE)) {
            self.vmars.rotate_left(index);
        } else {
            // All the vmars were full. Allocate a new VMAR to return.
            let vmar = CompactVmar::new().ok_or_else(|| errno!(ENOMEM))?;
            self.vmars.push_front(vmar);
        }
        Ok(self.vmars.front().expect("populated above").vmar.clone())
    }
    /// Creates a new empty `GrowableVmars`.
    pub fn new() -> GrowableVmars {
        Default::default()
    }
}

/// Holds VMARs suitable for creating new threads.
///
/// Provides three different VMAR allocators, one for each of the allocations performed on thread
/// creation. See `thrd_zx_create_handles_t` in `zircon/threads.h` for details.
#[derive(Debug, Default)]
pub struct ThreadCreateVmars {
    /// VMARs for the machine stack.
    pub machine_stack: GrowableVmars,
    /// VMARs for the security stack (unsafe stack on x86, shadow call stack on others).
    pub security_stack: GrowableVmars,
    /// VMARs for the thread control block and thread-local storage.
    pub thread_block: GrowableVmars,
}

impl ThreadCreateVmars {
    /// Creates a new `ThreadCreateVmars` with empty VMAR lists.
    pub fn new() -> ThreadCreateVmars {
        Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Wrapper around a zx::Vmar for testing that calls destroy on drop.
    struct VmarWrap(zx::Vmar);

    impl Drop for VmarWrap {
        fn drop(&mut self) {
            // SAFETY: No mappings were ever placed in this VMAR.
            unsafe {
                let _ = self.0.destroy();
            }
        }
    }

    fn fill_vmar(raw_handle: zx::sys::zx_handle_t) -> Vec<VmarWrap> {
        // SAFETY: We are only creating an unowned handle to inspect/allocate children.
        // The handle lifetime is managed by GrowableVmars.
        let vmar = unsafe { zx::Unowned::<zx::Vmar>::from_raw_handle(raw_handle) };
        let mut handles = Vec::new();
        loop {
            // Allocate a child VMAR to occupy space.
            // We use the same parameters as probe() to ensure we effectively block it.
            match vmar.allocate(0, PROBE_SIZE, zx::VmarFlags::CAN_MAP_READ) {
                Ok((child, _)) => handles.push(VmarWrap(child)),
                Err(_) => break,
            }
        }
        handles
    }

    #[fuchsia::test]
    fn test_thread_create_vmars_init() {
        let mut tcv = ThreadCreateVmars::new();
        // Just verify we can probe each.
        assert!(tcv.machine_stack.probe().is_ok());
        assert!(tcv.security_stack.probe().is_ok());
        assert!(tcv.thread_block.probe().is_ok());
    }

    #[fuchsia::test]
    fn test_probe_growth_and_rotation() {
        let mut vmars = GrowableVmars::new();

        // Probe for an initial VMAR.
        let h1 = vmars.probe().expect("failed to probe");
        let h1_raw = h1.raw_handle();

        // Exhaust the provided VMAR
        let allocs1 = fill_vmar(h1_raw);

        // Probing should yield a new VMAR.
        let h2 = vmars.probe().expect("failed to probe 2");
        let h2_raw = h2.raw_handle();
        assert_ne!(h1_raw, h2_raw);
        assert_eq!(vmars.vmars.len(), 2);
        // Current order should be [h2, h1] because h1 failed and h2 is new.
        assert_eq!(vmars.vmars[0].vmar.raw_handle(), h2_raw);
        assert_eq!(vmars.vmars[1].vmar.raw_handle(), h1_raw);

        // Now exhaust this VMAR.
        let _allocs2 = fill_vmar(h2_raw);

        // Once again, probing should yield another VMAR.
        let h3 = vmars.probe().expect("failed to probe 3");
        let h3_raw = h3.raw_handle();
        assert_ne!(h3_raw, h1_raw);
        assert_ne!(h3_raw, h2_raw);
        assert_eq!(vmars.vmars.len(), 3);
        assert_eq!(vmars.vmars[0].vmar.raw_handle(), h3_raw);

        // Exhaust this VMAR.
        let _allocs3 = fill_vmar(h3_raw);

        // Free our allocations from the original VMAR.
        drop(allocs1);

        // Probing should now find h1 again, and not allocate a new VMAR. A consequence of searching
        // should therefore leave the vmars in the order [h1, h3, h2]
        let h_rotated = vmars.probe().expect("failed to probe rotated");
        assert_eq!(h_rotated.raw_handle(), h1_raw);
        assert_eq!(vmars.vmars[0].vmar.raw_handle(), h1_raw);
        assert_eq!(vmars.vmars[1].vmar.raw_handle(), h3_raw);
        assert_eq!(vmars.vmars[2].vmar.raw_handle(), h2_raw);
    }
}
