// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::interned_string::InternedString;
use core::sync::atomic::AtomicU32;

/// A binary-stable, transparent representation of `fxt::InternedCategory`.
///
/// Under the C++ ABI, `fxt::InternedCategory` consists of:
/// 1. A reference to `InternedString`, represented as a `&'static InternedString` (8 bytes).
/// 2. A mutable atomic `u32` representing the category's index (4 bytes).
/// 3. 4 bytes of padding (implied by alignment rules on 64-bit platforms).
#[repr(C)]
pub struct InternedCategory {
    label: &'static InternedString,
    index: AtomicU32,
}

// SAFETY: InternedCategory points to a static InternedString and contains an atomic index.
// It is completely thread-safe to share and access across threads.
unsafe impl Sync for InternedCategory {}
unsafe impl Send for InternedCategory {}

impl InternedCategory {
    /// The index value representing an invalid or unregistered category.
    pub const INVALID_INDEX: u32 = u32::MAX;

    /// Creates a new `InternedCategory` from a reference to an `InternedString`.
    #[inline]
    pub const fn new(label: &'static InternedString) -> Self {
        Self { label, index: AtomicU32::new(Self::INVALID_INDEX) }
    }

    /// Returns a reference to the underlying `InternedString`.
    #[inline]
    pub fn label(&self) -> &'static InternedString {
        self.label
    }

    /// Returns the static category name as a safe C string reference.
    #[inline]
    pub fn string(&self) -> &'static core::ffi::CStr {
        self.label().as_c_str()
    }

    /// Returns the index associated with the category.
    #[inline]
    pub fn index(&self) -> u32 {
        self.index.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Sets the index for the category if it has the expected previous value. This provides a
    /// simple way to prevent re-initialization if RegisterCategories() is called more than once,
    /// while also providing a way to override the value. This can be removed once the index can be
    /// automatically derived from the section offset when the kernel supports extensible
    /// categories.
    #[inline]
    pub fn set_index(&self, index: u32, expected: u32) {
        let _ = self.index.compare_exchange(
            expected,
            index,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Acquire,
        );
    }
}

/// Statically declares a new `InternedCategory`.
///
/// By default, this macro allocates the category inside the special
/// `__fxt_interned_category_table` linker section.
///
/// If the `extern` parameter is provided, the macro instead references an external
/// symbol (e.g. C++ template-allocated) with the C++ mangled name for the category,
/// preventing duplicate physical allocation in the linker section.
///
/// # Examples
///
/// Local allocation:
/// ```rust
/// declare_interned_category!(MY_CATEGORY, "kernel:sched");
/// ```
///
/// External reference (references C++ symbol, avoids physical duplicates):
/// ```rust
/// declare_interned_category!(MY_CATEGORY, "kernel:meta", extern);
/// ```
#[macro_export]
macro_rules! declare_interned_category {
    ($var_name:ident, $str_lit:literal) => {
        #[allow(non_snake_case)]
        mod $var_name {
            $crate::declare_interned_string!(STRING, $str_lit);

            #[$crate::interned_category_export_name($str_lit)]
            #[unsafe(link_section = "__fxt_interned_category_table")]
            #[used]
            pub static CATEGORY: $crate::interned_category::InternedCategory =
                $crate::interned_category::InternedCategory::new(&STRING);
        }

        pub static $var_name: &$crate::interned_category::InternedCategory = &$var_name::CATEGORY;
    };

    ($var_name:ident, $str_lit:literal, extern) => {
        #[allow(non_snake_case)]
        mod $var_name {
            $crate::import_category!(CATEGORY, $str_lit);
        }

        pub static $var_name: &$crate::interned_category::InternedCategory =
            unsafe { &$var_name::CATEGORY };
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    declare_interned_category!(TEST_CAT_1, "foo");
    declare_interned_category!(TEST_CAT_2, "bar");

    #[test]
    fn test_label_and_string() {
        assert_eq!(TEST_CAT_1.string(), c"foo");
        assert_eq!(TEST_CAT_2.string(), c"bar");
        assert_eq!(TEST_CAT_1.label().as_c_str(), c"foo");
    }

    #[test]
    fn test_index_management() {
        assert_eq!(TEST_CAT_1.index(), InternedCategory::INVALID_INDEX);

        // Set index from INVALID_INDEX to 42.
        TEST_CAT_1.set_index(42, InternedCategory::INVALID_INDEX);
        assert_eq!(TEST_CAT_1.index(), 42);

        // Trying to set it again with a wrong expected value should fail/do nothing.
        TEST_CAT_1.set_index(100, 0);
        assert_eq!(TEST_CAT_1.index(), 42);
    }

    // This linker section bound test is only valid on ELF targets (Fuchsia/Linux)
    // where the linker generates __start and __stop symbols for orphan sections.
    #[cfg(any(target_os = "fuchsia", target_os = "linux"))]
    #[test]
    fn test_linker_section_allocation() {
        unsafe extern "C" {
            #[link_name = "__start___fxt_interned_category_table"]
            static START: InternedCategory;
            #[link_name = "__stop___fxt_interned_category_table"]
            static STOP: InternedCategory;
        }

        let start_ptr = unsafe { &START as *const InternedCategory };
        let stop_ptr = unsafe { &STOP as *const InternedCategory };

        // Ensure the boundary is valid and holds our entries
        assert!(start_ptr <= stop_ptr);
        let diff =
            (stop_ptr as usize - start_ptr as usize) / core::mem::size_of::<InternedCategory>();
        assert!(diff >= 2, "Expected at least 2 entries in the table, found {diff}");

        // Verify our static variables reside strictly within the linker bounds
        let p1 = TEST_CAT_1 as *const InternedCategory;
        let p2 = TEST_CAT_2 as *const InternedCategory;

        assert!(
            p1 >= start_ptr && p1 < stop_ptr,
            "TEST_CAT_1 pointer {p1:p} is outside bounds [{start_ptr:p}, {stop_ptr:p})"
        );
        assert!(
            p2 >= start_ptr && p2 < stop_ptr,
            "TEST_CAT_2 pointer {p2:p} is outside bounds [{start_ptr:p}, {stop_ptr:p})"
        );
    }

    #[test]
    fn test_symbol_name_matching() {
        // "foo" should mangle to the exact C++ symbol name:
        // _ZN3fxt8internal23InternedCategoryStorageIJLc102ELc111ELc111EEE17interned_categoryE
        unsafe extern "C" {
            #[link_name = "_ZN3fxt8internal23InternedCategoryStorageIJLc102ELc111ELc111EEE17interned_categoryE"]
            static EXPECTED_SYMBOL: InternedCategory;
        }

        let p_expected = unsafe { &EXPECTED_SYMBOL as *const InternedCategory };
        let p_actual = TEST_CAT_1 as *const InternedCategory;
        assert_eq!(p_actual, p_expected);
    }
}
