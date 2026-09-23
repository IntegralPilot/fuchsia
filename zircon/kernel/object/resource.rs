// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// When validating MMIO resource ranges against a non-root parent MMIO resource, we need to verify
// that the requested range is completely contained by its parent's range. There are two different
// ways that we can do this: a strict validation or a non-strict validation.
//
// A strict validation means any time we validate a range against an MMIO resource object, the MMIO
// resource's range *must* completely contain *all* of the requested range, without exception.
//
// A non-strict validation means that we check to make sure that the requested range is completely
// contained by the parent's _page aligned_ range. Because of some legacy behavior, when creating a
// physical VMO using an MMIO resource as a token, user-mode drivers (PCIe in particular) depend
// on the kernel allowing the operation provided that the MMIO resource's range simply touches all
// of the pages covered by the MMIO resource. IOW - if an MMIO resource covers the range
// `[X + 0x80, X + 0x100)`, user-mode expects this resource to allow it to create a physical VMO
// covering `[X, X + page::SIZE)`.
//
// By default, we use non-strict validation. However, when we attempt to create an MMIO resource as
// a child of another MMIO resource, we demand strict validation. Failure to do this means that
// someone could create an MMIO resource whose range is larger than that of its parents, which is
// more than a little bit confusing.
//
// TODO(b/506251014): Remove this when we get to the point where we can expect user-mode to always
// page-align all of its MMIO resource ranges.

use range_check::get_intersect_offset_len;
use zx_status::Status;
use zx_types::{ZX_RSRC_KIND_MMIO, ZX_RSRC_KIND_SYSTEM, zx_rsrc_kind_t, zx_rsrc_system_base_t};

use super::Dispatcher;
use super::handle::HandleValue;
use super::resource_dispatcher::ResourceDispatcher;
use crate::root_resource_filter::root_resource_filter_can_access_region;

// TODO(https://fxbug.dev/42107339): Take another look at validation and consider returning
// dispatchers or move validation into the parent dispatcher itself.

/// Strictness policy for validating MMIO resource ranges against parents.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum StrictValidation {
    No,
    Yes,
}

/// Check if the resource referenced by `handle` is of kind `kind` AND base `base`.
///
/// # Errors
///
/// - `ZX_ERR_BAD_HANDLE` if `handle` is not a valid handle.
/// - `ZX_ERR_WRONG_TYPE` if `handle` is not the right `kind` or `base`.
pub fn validate_resource_kind_base(
    handle: HandleValue,
    kind: zx_rsrc_kind_t,
    base: zx_rsrc_system_base_t,
) -> Result<(), Status> {
    let resource = Dispatcher::get::<ResourceDispatcher>(handle)?;
    if resource.get_kind() == kind && resource.get_base() == base {
        Ok(())
    } else {
        Err(Status::WRONG_TYPE)
    }
}

/// Validates that `resource` grants access to the requested range for `kind`.
///
/// If `strict_validation` is `StrictValidation::No`, MMIO resources allow operations
/// within their page-aligned bounds.
///
/// # Errors
///
/// - `ZX_ERR_ACCESS_DENIED` if access to the region is denied by the root resource filter.
/// - `ZX_ERR_WRONG_TYPE` if `resource`'s kind does not match `kind`.
/// - `ZX_ERR_OUT_OF_RANGE` if the requested range is not fully contained in `resource`'s range.
pub fn validate_ranged_resource_dispatcher(
    resource: &ResourceDispatcher,
    kind: zx_rsrc_kind_t,
    base: u64,
    size: usize,
    strict_validation: StrictValidation,
) -> Result<(), Status> {
    // Resources get access to almost everything, but there are still resource ranges they are not
    // permitted to mint. For example:
    //
    // 1) All of physical RAM is off limits (with limited platform specific exceptions). It exists
    //    on the CPU accessible physical bus (so, the domain controlled by ZX_RSRC_KIND_MMIO) and
    //    user mode program should not be able to request access to physical RAM by address, they
    //    should be forced to go through the PMM using VMO creation instead.
    // 2) Any MMIO accessible interrupt controller registers.
    // 3) Any MMIO accessible IOMMU registers.
    //
    // Enforce that policy here by disallowing resource minting for any request which touches any
    // disallowed ranges.
    if resource.is_ranged_root(kind) {
        // If we are creating an MMIO resource from one of the root resources, make sure that range
        // being requested does not share a page with any of the kernel reserved regions.
        let (check_base, check_size) = if kind == ZX_RSRC_KIND_MMIO {
            let base = base as usize;
            let aligned_base = page::round_down(base);
            (aligned_base, page::round_up(base - aligned_base + size))
        } else {
            (base as usize, size)
        };

        if !root_resource_filter_can_access_region(check_base, check_size, kind) {
            return Err(Status::ACCESS_DENIED);
        }
        return Ok(());
    }

    if resource.get_kind() != kind {
        return Err(Status::WRONG_TYPE);
    }

    let (rbase, rsize) = (resource.get_base(), resource.get_size());

    // All resources need to track their lineage back to the root resource, and the root resource is
    // specifically prohibited from producing ranges which intersect anything in the deny list.
    // Since all resource ranges need to be a subset of their parent, it should be impossible for a
    // resource object to exist with a range which intersects anything in the deny list. Check that
    // with a debug assert here.
    debug_assert!(root_resource_filter_can_access_region(rbase as usize, rsize, kind));

    // In the specific case of MMIO, everything is rounded to page::SIZE units because it's the
    // smallest unit we can operate at with the MMU.
    let (check_base, check_size) =
        if strict_validation == StrictValidation::No && resource.get_kind() == ZX_RSRC_KIND_MMIO {
            let rbase = rbase as usize;
            let aligned_base = page::round_down(rbase);
            (aligned_base as u64, page::round_up(rbase - aligned_base + rsize) as u64)
        } else {
            (rbase, rsize as u64)
        };

    let size = size as u64;

    // Check for intersection and make sure the requested base+size fits within the resource's
    // address space allocation.
    match get_intersect_offset_len(base, size, check_base, check_size) {
        Some((ibase, isize)) if ibase == base && isize == size => Ok(()),
        _ => Err(Status::OUT_OF_RANGE),
    }
}

/// Check if the resource referenced by `handle` is of kind `kind`. If `kind` matches the
/// resource's kind, then range validation between `base` and `size` will be made against the
/// resource's backing address space allocation (defaults to non-strict MMIO validation).
///
/// # Errors
///
/// - `ZX_ERR_BAD_HANDLE` if `handle` is not a valid handle.
/// - `ZX_ERR_WRONG_TYPE` if `handle` is not a valid resource handle, or `kind` is invalid for the
///   request.
/// - `ZX_ERR_OUT_OF_RANGE` if the range specified by `base` and `size` is not granted by this
///   resource.
pub fn validate_ranged_resource(
    handle: HandleValue,
    kind: zx_rsrc_kind_t,
    base: u64,
    size: usize,
) -> Result<(), Status> {
    validate_ranged_resource_with_strict(handle, kind, base, size, StrictValidation::No)
}

/// Validates a resource handle against a requested range with explicit strictness.
///
/// # Errors
///
/// - `ZX_ERR_BAD_HANDLE` if `handle` is not a valid handle.
/// - `ZX_ERR_WRONG_TYPE` if `handle` is not a valid resource handle, or `kind` is invalid for the
///   request.
/// - `ZX_ERR_OUT_OF_RANGE` if the range specified by `base` and `size` is not granted by this
///   resource.
pub fn validate_ranged_resource_with_strict(
    handle: HandleValue,
    kind: zx_rsrc_kind_t,
    base: u64,
    size: usize,
    strict_validation: StrictValidation,
) -> Result<(), Status> {
    let resource = Dispatcher::get::<ResourceDispatcher>(handle)?;
    validate_ranged_resource_dispatcher(&resource, kind, base, size, strict_validation)
}

/// Validates access to a single system resource indexed by `base`.
pub fn validate_system_resource(handle: HandleValue, base: u64) -> Result<(), Status> {
    validate_ranged_resource(handle, ZX_RSRC_KIND_SYSTEM, base, 1)
}

/// Kernel unit tests for resource validation functions.
#[cfg(ktest)]
#[unittest::suite(name = "resource_validation_tests")]
mod tests {
    use crate::object::resource_dispatcher::{ResourceDispatcher, ResourceStorage};
    use pin_init::stack_pin_init;
    use zx_status::Status;
    use zx_types::{ZX_RSRC_KIND_MMIO, ZX_RSRC_KIND_SYSTEM};

    use super::{StrictValidation, validate_ranged_resource_dispatcher};

    /// Tests validate_ranged_resource_dispatcher with ranged root and child MMIO resources.
    #[test]
    fn test_validate_ranged_resource_dispatcher() {
        stack_pin_init!(let storage = ResourceStorage::init());

        // Find an MMIO range that is not denied by the root resource filter (i.e. not in RAM).
        let step = 0x1000_0000u64;
        let mut test_base = 0u64;
        let found = loop {
            if root_resource_filter_can_access_region(
                test_base as usize,
                0x10_0000,
                ZX_RSRC_KIND_MMIO,
            ) {
                break true;
            }
            if test_base >= u64::MAX - step {
                break false;
            }
            test_base += step;
        };
        assert!(found, "could not find non-denied MMIO region");

        // Initialize MMIO allocator for tests
        ResourceDispatcher::initialize_allocator_with_storage(
            ZX_RSRC_KIND_MMIO,
            test_base,
            0x10_0000,
            &storage,
        )
        .expect("initialize MMIO allocator failed");

        // Create a ranged root resource for MMIO
        let (ranged_root_handle, _) = ResourceDispatcher::create_ranged_root_with_storage(
            ZX_RSRC_KIND_MMIO,
            b"mmio-root",
            &storage,
        )
        .expect("create ranged root failed");

        unittest::expect_ok!(validate_ranged_resource_dispatcher(
            ranged_root_handle.dispatcher(),
            ZX_RSRC_KIND_MMIO,
            test_base + 0x2000,
            0x1000,
            StrictValidation::Yes,
        ));
        unittest::expect_err!(
            validate_ranged_resource_dispatcher(
                ranged_root_handle.dispatcher(),
                ZX_RSRC_KIND_SYSTEM,
                1,
                1,
                StrictValidation::No,
            ),
            Status::WRONG_TYPE
        );

        // Create a concrete MMIO resource covering [test_base + 0x10080, test_base + 0x10100)
        // (unaligned).
        let (child_handle, _) = ResourceDispatcher::create_with_storage(
            ZX_RSRC_KIND_MMIO,
            test_base + 0x10080,
            0x80,
            0,
            b"mmio-child",
            &storage,
        )
        .expect("create child MMIO failed");

        // Exactly contained in [test_base + 0x10080, test_base + 0x10100)
        unittest::expect_ok!(validate_ranged_resource_dispatcher(
            child_handle.dispatcher(),
            ZX_RSRC_KIND_MMIO,
            test_base + 0x10080,
            0x80,
            StrictValidation::Yes,
        ));

        // In page-aligned range [test_base + 0x10000, test_base + 0x11000), strict=false passes,
        // strict=true fails.
        unittest::expect_ok!(validate_ranged_resource_dispatcher(
            child_handle.dispatcher(),
            ZX_RSRC_KIND_MMIO,
            test_base + 0x10000,
            page::SIZE,
            StrictValidation::No,
        ));
        unittest::expect_err!(
            validate_ranged_resource_dispatcher(
                child_handle.dispatcher(),
                ZX_RSRC_KIND_MMIO,
                test_base + 0x10000,
                page::SIZE,
                StrictValidation::Yes,
            ),
            Status::OUT_OF_RANGE
        );

        // Out of range entirely
        unittest::expect_err!(
            validate_ranged_resource_dispatcher(
                child_handle.dispatcher(),
                ZX_RSRC_KIND_MMIO,
                test_base + 0x20000,
                0x1000,
                StrictValidation::No,
            ),
            Status::OUT_OF_RANGE
        );
    }
}
