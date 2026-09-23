// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::marker::{PhantomData, PhantomPinned};
use pin_init::{PinInit, pin_data};
use pmm_checker_bindings as bindings;
use zr::Opaque;

use crate::vm::page::VmPagePtr;

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CheckFailAction {
    Oops = 0,
    Panic = 1,
}

/// `PmmChecker` is used to detect memory corruption.  It is logically part of `PmmNode`.
///
/// Usage is as follows:
///
/// ```
/// stack_pin_init!(let checker = PmmChecker::init());
/// let mut checker = unsafe { checker.get_unchecked_mut() };
///
/// // Check only the first 16 bytes of each page.
/// checker.set_fill_size(16);
///
/// // For all free pages...
/// for page in free_pages {
///     checker.fill_pattern(page);
/// }
///
/// // Now that all free pages have been filled with a pattern, we can arm the checker.
/// checker.arm();
/// ...
/// checker.assert_pattern(page);
/// ```
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct PmmChecker {
    raw: Opaque<bindings::PmmChecker>,
    phantom: PhantomData<PhantomPinned>,
}

zr::unsafe_pinned_drop_ffi!(PmmChecker, bindings::cpp_pmm_checker_destroy);

impl PmmChecker {
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        zr::pin_init_ffi!(bindings::cpp_pmm_checker_init)
    }
    /// Domain-specific conversion: returns raw pointer for `PmmChecker`.
    pub fn as_raw(&self) -> *mut bindings::PmmChecker {
        self.raw.get()
    }

    /// Returns true if `fill_size` is a valid value.  Valid values are multiples of 8 between 8 and
    /// `page::SIZE`, inclusive.
    pub fn is_valid_fill_size(fill_size: usize) -> bool {
        // SAFETY: No preconditions.
        unsafe { bindings::cpp_pmm_checker_is_valid_fill_size(fill_size) }
    }

    /// Sets the size of the pattern to be written / validated.
    ///
    /// It is an error to call this method with an invalid fill size
    /// (see [`Self::is_valid_fill_size`]).
    ///
    /// It is an error to call this method if the checker [`Self::is_armed`].  After changing the
    /// fill size, be sure to re-fill any free pages to ensure that a future call to
    /// [`Self::validate_pattern`] or [`Self::assert_pattern`] won't spuriously report corruption.
    pub fn set_fill_size(&mut self, fill_size: usize) {
        // SAFETY: `self.as_raw()` returns a valid `PmmChecker` pointer.
        unsafe { bindings::cpp_pmm_checker_set_fill_size(self.as_raw(), fill_size) }
    }

    /// Returns the fill size.
    pub fn get_fill_size(&self) -> usize {
        // SAFETY: `self.as_raw()` returns a valid `PmmChecker` pointer.
        unsafe { bindings::cpp_pmm_checker_get_fill_size(self.as_raw()) }
    }

    /// Sets the check fail action.
    pub fn set_action(&mut self, action: CheckFailAction) {
        // SAFETY: `self.as_raw()` returns a valid `PmmChecker` pointer.
        unsafe { bindings::cpp_pmm_checker_set_action(self.as_raw(), action as u8) }
    }

    /// Returns the check fail action.
    pub fn get_action(&self) -> CheckFailAction {
        // SAFETY: `self.as_raw()` returns a valid `PmmChecker` pointer.
        let raw = unsafe { bindings::cpp_pmm_checker_get_action(self.as_raw()) };
        match raw {
            0 => CheckFailAction::Oops,
            _ => CheckFailAction::Panic,
        }
    }

    /// Returns true if armed.
    pub fn is_armed(&self) -> bool {
        // SAFETY: `self.as_raw()` returns a valid `PmmChecker` pointer.
        unsafe { bindings::cpp_pmm_checker_is_armed(self.as_raw()) }
    }

    /// Arms the checker.
    pub fn arm(&mut self) {
        // SAFETY: `self.as_raw()` returns a valid `PmmChecker` pointer.
        unsafe { bindings::cpp_pmm_checker_arm(self.as_raw()) }
    }

    /// Fills `page` with a pattern.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is valid and may be written to.
    pub unsafe fn fill_pattern(&self, page: VmPagePtr) {
        // SAFETY: `self.as_raw()` and `page.as_ffi()` return valid pointers.
        unsafe { bindings::cpp_pmm_checker_fill_pattern(self.as_raw(), page.as_ffi()) }
    }

    /// Returns true if `page` contains the expected fill pattern or `is_armed` is false.
    ///
    /// Otherwise, returns false.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is valid and may be read to.
    pub unsafe fn validate_pattern(&self, page: VmPagePtr) -> bool {
        // SAFETY: `self.as_raw()` and `page.as_ffi()` return valid pointers.
        unsafe { bindings::cpp_pmm_checker_validate_pattern(self.as_raw(), page.as_ffi()) }
    }

    /// Panics the kernel if `page` does not contain the expected fill pattern and `is_armed` is
    /// true.
    ///
    /// Otherwise, does nothing.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is valid and may be read to.
    pub unsafe fn assert_pattern(&self, page: VmPagePtr) {
        // SAFETY: `self.as_raw()` and `page.as_ffi()` return valid pointers.
        unsafe { bindings::cpp_pmm_checker_assert_pattern(self.as_raw(), page.as_ffi()) }
    }

    /// Prints the status of the checker to standard output.
    pub fn print_status_stdout(&self) {
        // SAFETY: `self.as_raw()` returns a valid `PmmChecker` pointer.
        unsafe { bindings::cpp_pmm_checker_print_status_stdout(self.as_raw()) }
    }
}

/// Unit tests for PmmChecker.
#[cfg(ktest)]
#[unittest::suite(name = "pmm_checker_rust")]
mod pmm_checker_rust {
    use super::PmmChecker;
    use crate::vm::page_state::VmPageState;
    use crate::vm::physmap::paddr_to_physmap;
    use crate::vm::pmm;
    use page::SIZE as PAGE_SIZE;
    use page_bindings::vm_page_state;
    use pin_init::stack_pin_init;
    use unittest::{expect_eq, expect_false, expect_ne, expect_true, unwrap_ok};

    macro_rules! pmm_checker_test_with_fill_size {
        ($fill_size:expr) => {{
            let fill_size = $fill_size;
            stack_pin_init!(let checker = PmmChecker::init());
            let checker = unsafe { checker.get_unchecked_mut() };

            // Starts off unarmed.
            expect_false!(checker.is_armed());

            // Borrow a real page from the PMM, ask the checker to validate it. See that because the checker
            // is not armed, |ValidatePattern| still returns true even though the page has no pattern.
            let (page, pa) = unwrap_ok!(pmm::alloc_page(0));
            unsafe { page.set_state(VmPageState(vm_page_state::FREE)) };
            let p_vaddr = paddr_to_physmap(pa);
            // SAFETY: pa is a valid physical address for this allocated page.
            let p = unsafe { core::slice::from_raw_parts_mut(p_vaddr.0 as *mut u8, PAGE_SIZE) };
            p.fill(0);
            expect_true!(unsafe{checker.validate_pattern(page)});
            unsafe {checker.assert_pattern(page)};

            // Set the fill size and see that |GetFillSize| returns the size.
            checker.set_fill_size(fill_size);
            expect_eq!(fill_size, checker.get_fill_size());

            // Arm the checker and see that |ValidatePattern| returns false.
            checker.arm();
            expect_true!(checker.is_armed());
            expect_false!(unsafe{checker.validate_pattern(page)});

            // Fill with pattern one less than the fill size and see that it does not pass validation.
            p[..fill_size - 1].fill(0);
            expect_false!(unsafe{checker.validate_pattern(page)});

            // Fill with the full pattern and see that it validates.
            unsafe {checker.fill_pattern(page)};
            for &byte in &p[..fill_size] {
                expect_ne!(0, byte);
            }
            expect_true!(unsafe{checker.validate_pattern(page)});

            // Corrupt the page after the first |fill_size| bytes and see that the corruption is not detected.
            if fill_size < PAGE_SIZE {
                p[fill_size] = 1;
                expect_true!(unsafe{checker.validate_pattern(page)});
            }

            // Corrupt the page within the first |fill_size| bytes and see that the corruption is detected.
            p[fill_size - 1] = 1;
            expect_false!(unsafe {checker.validate_pattern(page)});

            unsafe { page.set_state(VmPageState(vm_page_state::ALLOC)) };
            unsafe { pmm::free_page(page) };
        }};
    }

    /// Tests PMM checker validation and pattern filling.
    #[test]
    fn checker() {
        pmm_checker_test_with_fill_size!(8);
        pmm_checker_test_with_fill_size!(16);
        pmm_checker_test_with_fill_size!(512);
        pmm_checker_test_with_fill_size!(PAGE_SIZE);
    }

    /// Tests validation of fill sizes for PmmChecker.
    #[test]
    fn is_valid_fill_size() {
        expect_false!(PmmChecker::is_valid_fill_size(0));
        expect_false!(PmmChecker::is_valid_fill_size(7));
        expect_false!(PmmChecker::is_valid_fill_size(9));
        expect_false!(PmmChecker::is_valid_fill_size(PAGE_SIZE + 8));
        expect_false!(PmmChecker::is_valid_fill_size(PAGE_SIZE * 2));

        expect_true!(PmmChecker::is_valid_fill_size(8));
        expect_true!(PmmChecker::is_valid_fill_size(16));
        expect_true!(PmmChecker::is_valid_fill_size(24));
        expect_true!(PmmChecker::is_valid_fill_size(512));
        expect_true!(PmmChecker::is_valid_fill_size(PAGE_SIZE));
    }
}
