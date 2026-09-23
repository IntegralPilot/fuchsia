// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// A binary-stable, transparent representation of `fxt::InternedString`.
///
/// This structure has the exact same memory layout as the C++ `fxt::InternedString`
/// (which is a single `const char*` pointer pointing to a null-terminated string).
///
/// Placing instances of this structure in the special linker section `__fxt_interned_string_table`
/// allows them to sit contiguous alongside C++'s own interned strings in the global
/// string table segment, meaning their trace-ID calculations (`this - section_begin`)
/// are safe, exact, and valid.
#[repr(transparent)]
pub struct InternedString {
    ptr: *const u8,
}

// SAFETY: InternedString points to a static, read-only null-terminated string byte array.
// It is completely thread-safe to share and access across threads.
unsafe impl Sync for InternedString {}
unsafe impl Send for InternedString {}

impl InternedString {
    /// Creates a new `InternedString` from a raw pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the pointer points to a null-terminated string
    /// with static storage duration ('static).
    #[inline]
    pub const unsafe fn new_raw(ptr: *const u8) -> Self {
        Self { ptr }
    }

    /// Returns the raw pointer to the null-terminated string.
    #[inline]
    pub const fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Returns the static name as a safe, null-terminated C string reference.
    #[inline]
    pub fn as_c_str(&self) -> &'static core::ffi::CStr {
        // SAFETY: The safety invariants of the raw constructor guarantee that the pointer
        // points to a valid, null-terminated static byte string in read-only memory.
        unsafe { core::ffi::CStr::from_ptr(self.ptr as *const core::ffi::c_char) }
    }

    /// Returns the numeric trace ID for this interned string.
    #[inline]
    pub fn id(&self) -> u16 {
        unsafe extern "C" {
            #[link_name = "__start___fxt_interned_string_table"]
            static START: InternedString;
        }
        let self_ptr = self as *const InternedString;
        let start_ptr = unsafe { &START as *const InternedString };
        // SAFETY: Both pointers reside within the contiguous `__fxt_interned_string_table` linker
        // section.
        let diff = unsafe { self_ptr.offset_from(start_ptr) };
        (diff + 1) as u16
    }
}

/// Statically declares a new `InternedString`.
///
/// By default, this macro allocates the string inside the special
/// `__fxt_interned_string_table` linker section.
///
/// If the `extern` parameter is provided, the macro instead references an external
/// symbol (e.g. C++ template-allocated) with the C++ mangled name for the string,
/// preventing duplicate physical allocation in the linker section.
///
/// # Examples
///
/// Local allocation:
/// ```rust
/// declare_interned_string!(MY_STRING, "hello.world");
/// ```
///
/// External reference (references C++ symbol, avoids physical duplicates):
/// ```rust
/// declare_interned_string!(MY_STRING, "drop_stats", extern);
/// ```
#[macro_export]
macro_rules! declare_interned_string {
    ($var_name:ident, $str_lit:literal) => {
        #[allow(non_snake_case)]
        mod $var_name {
            #[$crate::interned_string_export_name($str_lit)]
            #[unsafe(link_section = "__fxt_interned_string_table")]
            #[used]
            pub static STRING: $crate::interned_string::InternedString = unsafe {
                // Append a null byte to the string literal at compile time to satisfy
                // the C++ const char* expectations.
                $crate::interned_string::InternedString::new_raw(concat!($str_lit, "\0").as_ptr())
            };
        }

        pub static $var_name: &$crate::interned_string::InternedString = &$var_name::STRING;
    };

    ($var_name:ident, $str_lit:literal, extern) => {
        #[allow(non_snake_case)]
        mod $var_name {
            $crate::import_string!(STRING, $str_lit);
        }

        pub static $var_name: &$crate::interned_string::InternedString =
            unsafe { &$var_name::STRING };
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    declare_interned_string!(TEST_STR_1, "hello");
    declare_interned_string!(TEST_STR_2, "world");

    #[test]
    fn test_as_ptr() {
        assert!(!TEST_STR_1.as_ptr().is_null());
        assert!(!TEST_STR_2.as_ptr().is_null());

        // Verify null termination!
        unsafe {
            let mut ptr = TEST_STR_1.as_ptr();
            while *ptr != 0 {
                ptr = ptr.add(1);
            }
            assert_eq!(*ptr, 0);
        }
    }

    #[test]
    fn test_as_c_str() {
        let c_str1 = TEST_STR_1.as_c_str();
        let c_str2 = TEST_STR_2.as_c_str();

        assert_eq!(c_str1, c"hello");
        assert_eq!(c_str2, c"world");

        // Verify conversion to standard Rust &str
        assert_eq!(c_str1.to_str().unwrap(), "hello");
        assert_eq!(c_str2.to_str().unwrap(), "world");
    }

    // This linker section bound test is only valid on ELF targets (Fuchsia/Linux)
    // where the linker generates __start and __stop symbols for orphan sections.
    #[cfg(any(target_os = "fuchsia", target_os = "linux"))]
    #[test]
    fn test_linker_section_allocation() {
        unsafe extern "C" {
            #[link_name = "__start___fxt_interned_string_table"]
            static START: InternedString;
            #[link_name = "__stop___fxt_interned_string_table"]
            static STOP: InternedString;
        }

        let start_ptr = unsafe { &START as *const InternedString };
        let stop_ptr = unsafe { &STOP as *const InternedString };

        // Ensure the boundary is valid and holds our entries
        assert!(start_ptr <= stop_ptr);
        let diff =
            (stop_ptr as usize - start_ptr as usize) / core::mem::size_of::<InternedString>();
        assert!(diff >= 2, "Expected at least 2 entries in the table, found {diff}");

        // Verify our static variables reside strictly within the linker bounds
        let p1 = TEST_STR_1 as *const InternedString;
        let p2 = TEST_STR_2 as *const InternedString;

        assert!(
            p1 >= start_ptr && p1 < stop_ptr,
            "TEST_STR_1 pointer {p1:p} is outside bounds [{start_ptr:p}, {stop_ptr:p})"
        );
        assert!(
            p2 >= start_ptr && p2 < stop_ptr,
            "TEST_STR_2 pointer {p2:p} is outside bounds [{start_ptr:p}, {stop_ptr:p})"
        );
    }

    #[test]
    fn test_id() {
        assert!(TEST_STR_1.id() > 0);
        assert!(TEST_STR_2.id() > 0);

        let p1 = TEST_STR_1 as *const InternedString;
        let p2 = TEST_STR_2 as *const InternedString;
        let expected_diff = unsafe { p2.offset_from(p1) };
        let actual_diff = (TEST_STR_2.id() as isize) - (TEST_STR_1.id() as isize);
        assert_eq!(actual_diff, expected_diff);
    }

    #[test]
    fn test_symbol_name_matching() {
        // "hello" should mangle to the exact C++ symbol name:
        // _ZN3fxt8internal21InternedStringStorageIJLc104ELc101ELc108ELc108ELc111EEE15interned_stringE
        unsafe extern "C" {
            #[link_name = "_ZN3fxt8internal21InternedStringStorageIJLc104ELc101ELc108ELc108ELc111EEE15interned_stringE"]
            static EXPECTED_SYMBOL: InternedString;
        }

        let p_expected = unsafe { &EXPECTED_SYMBOL as *const InternedString };
        let p_actual = TEST_STR_1 as *const InternedString;
        assert_eq!(p_actual, p_expected);
    }
}
