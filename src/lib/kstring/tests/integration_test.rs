// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use kstring::interned_category::InternedCategory;
use kstring::interned_string::InternedString;
use kstring::{declare_interned_category, declare_interned_string};

declare_interned_string!(RUST_HELLO, "hello");
declare_interned_category!(RUST_CATEGORY, "hello_category");

unsafe extern "C" {
    fn get_cpp_hello_ptr() -> *const InternedString;
    fn get_cpp_category_ptr() -> *const InternedCategory;
}

// Verifies that C++ and Rust interned strings with the same content are successfully
// deduplicated and merged by the linker into a single memory location.
//
// Under the hood:
// 1. C++ instantiates `"hello"_intern` which generates a template static variable
//    mangled under the Itanium ABI.
// 2. Rust instantiates `declare_interned_string!(RUST_HELLO, "hello")` which
//    uses a procedural macro to export the static under the exact same mangled C++
//    symbol name.
// 3. At link-time, the linker sees two symbols with the same name and merges
//    them, deduplicating the storage.
// 4. This test asserts that the C++ pointer and the Rust pointer point to the
//    exact same address.
#[test]
fn test_cpp_rust_symbol_merging() {
    unsafe {
        let cpp_ptr = get_cpp_hello_ptr();
        let rust_ptr = RUST_HELLO as *const InternedString;
        assert!(!cpp_ptr.is_null());
        assert!(!rust_ptr.is_null());
        assert_eq!(cpp_ptr, rust_ptr, "C++ and Rust pointers did not match!");
    }
}

// Verifies that C++ and Rust interned categories with the same content are successfully
// deduplicated and merged by the linker into a single memory location.
//
// Under the hood:
// 1. C++ instantiates `"hello_category"_category` which generates a template static variable
//    mangled under the Itanium ABI.
// 2. Rust instantiates `declare_interned_category!(RUST_CATEGORY, "hello_category")` which
//    uses a procedural macro to export the static under the exact same mangled C++
//    symbol name.
// 3. At link-time, the linker sees two symbols with the same name and merges
//    them, deduplicating the storage.
// 4. This test asserts that the C++ category pointer and the Rust category pointer point to the
//    exact same address, meaning their underlying string and atomic index are perfectly shared.
#[test]
fn test_cpp_rust_category_symbol_merging() {
    unsafe {
        let cpp_ptr = get_cpp_category_ptr();
        let rust_ptr = RUST_CATEGORY as *const InternedCategory;
        assert!(!cpp_ptr.is_null());
        assert!(!rust_ptr.is_null());
        assert_eq!(cpp_ptr, rust_ptr, "C++ and Rust category pointers did not match!");
    }
}
