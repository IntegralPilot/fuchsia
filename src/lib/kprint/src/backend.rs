// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

unsafe extern "C" {
    pub fn printf(format: *const core::ffi::c_char, ...) -> core::ffi::c_int;

    pub fn snprintf(
        str: *mut core::ffi::c_char,
        size: usize,
        format: *const core::ffi::c_char,
        ...
    ) -> core::ffi::c_int;
}

/// Trait implemented by types that can be formatted as a string slice (`%.*s`).
pub trait AsKPrintStr {
    fn kprint_len(&self) -> core::ffi::c_int;
    fn kprint_ptr(&self) -> *const core::ffi::c_char;
}

impl AsKPrintStr for &str {
    #[inline(always)]
    fn kprint_len(&self) -> core::ffi::c_int {
        self.len() as core::ffi::c_int
    }
    #[inline(always)]
    fn kprint_ptr(&self) -> *const core::ffi::c_char {
        self.as_ptr() as *const core::ffi::c_char
    }
}

impl AsKPrintStr for str {
    #[inline(always)]
    fn kprint_len(&self) -> core::ffi::c_int {
        self.len() as core::ffi::c_int
    }
    #[inline(always)]
    fn kprint_ptr(&self) -> *const core::ffi::c_char {
        self.as_ptr() as *const core::ffi::c_char
    }
}

impl AsKPrintStr for &[u8] {
    #[inline(always)]
    fn kprint_len(&self) -> core::ffi::c_int {
        self.len() as core::ffi::c_int
    }
    #[inline(always)]
    fn kprint_ptr(&self) -> *const core::ffi::c_char {
        self.as_ptr() as *const core::ffi::c_char
    }
}

impl AsKPrintStr for [u8] {
    #[inline(always)]
    fn kprint_len(&self) -> core::ffi::c_int {
        self.len() as core::ffi::c_int
    }
    #[inline(always)]
    fn kprint_ptr(&self) -> *const core::ffi::c_char {
        self.as_ptr() as *const core::ffi::c_char
    }
}

impl<const N: usize> AsKPrintStr for [u8; N] {
    #[inline(always)]
    fn kprint_len(&self) -> core::ffi::c_int {
        self.len() as core::ffi::c_int
    }
    #[inline(always)]
    fn kprint_ptr(&self) -> *const core::ffi::c_char {
        self.as_ptr() as *const core::ffi::c_char
    }
}

impl<const N: usize> AsKPrintStr for &[u8; N] {
    #[inline(always)]
    fn kprint_len(&self) -> core::ffi::c_int {
        self.len() as core::ffi::c_int
    }
    #[inline(always)]
    fn kprint_ptr(&self) -> *const core::ffi::c_char {
        self.as_ptr() as *const core::ffi::c_char
    }
}

impl AsKPrintStr for &core::ffi::CStr {
    #[inline(always)]
    fn kprint_len(&self) -> core::ffi::c_int {
        self.to_bytes().len() as core::ffi::c_int
    }
    #[inline(always)]
    fn kprint_ptr(&self) -> *const core::ffi::c_char {
        self.as_ptr()
    }
}

impl AsKPrintStr for core::ffi::CStr {
    #[inline(always)]
    fn kprint_len(&self) -> core::ffi::c_int {
        self.to_bytes().len() as core::ffi::c_int
    }
    #[inline(always)]
    fn kprint_ptr(&self) -> *const core::ffi::c_char {
        self.as_ptr()
    }
}

/// Trait implemented by types that can be formatted as a null-terminated C string (`%s`).
pub trait AsKPrintCString {
    fn as_c_ptr(&self) -> *const core::ffi::c_char;
}

impl AsKPrintCString for *const i8 {
    #[inline(always)]
    fn as_c_ptr(&self) -> *const core::ffi::c_char {
        *self as *const core::ffi::c_char
    }
}

impl AsKPrintCString for *mut i8 {
    #[inline(always)]
    fn as_c_ptr(&self) -> *const core::ffi::c_char {
        *self as *const core::ffi::c_char
    }
}

impl AsKPrintCString for *const u8 {
    #[inline(always)]
    fn as_c_ptr(&self) -> *const core::ffi::c_char {
        *self as *const core::ffi::c_char
    }
}

impl AsKPrintCString for *mut u8 {
    #[inline(always)]
    fn as_c_ptr(&self) -> *const core::ffi::c_char {
        *self as *const core::ffi::c_char
    }
}

impl AsKPrintCString for &core::ffi::CStr {
    #[inline(always)]
    fn as_c_ptr(&self) -> *const core::ffi::c_char {
        self.as_ptr()
    }
}

impl AsKPrintCString for core::ffi::CStr {
    #[inline(always)]
    fn as_c_ptr(&self) -> *const core::ffi::c_char {
        self.as_ptr()
    }
}

/// Trait implemented by signed integer types formatted with `%lld`.
pub trait AsKPrintSignedInt {
    fn as_c_longlong(&self) -> core::ffi::c_longlong;
}

macro_rules! impl_signed_int {
    ($($t:ty),*) => {
        $(
            impl AsKPrintSignedInt for $t {
                #[inline(always)]
                fn as_c_longlong(&self) -> core::ffi::c_longlong {
                    *self as core::ffi::c_longlong
                }
            }
            impl AsKPrintSignedInt for &$t {
                #[inline(always)]
                fn as_c_longlong(&self) -> core::ffi::c_longlong {
                    **self as core::ffi::c_longlong
                }
            }
        )*
    };
}

impl_signed_int!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, bool, char);

/// Trait implemented by unsigned integer types formatted with `%llu`, `%llx`, `%llo`.
pub trait AsKPrintUnsignedInt {
    fn as_c_ulonglong(&self) -> core::ffi::c_ulonglong;
}

macro_rules! impl_unsigned_int {
    ($($t:ty),*) => {
        $(
            impl AsKPrintUnsignedInt for $t {
                #[inline(always)]
                fn as_c_ulonglong(&self) -> core::ffi::c_ulonglong {
                    *self as core::ffi::c_ulonglong
                }
            }
            impl AsKPrintUnsignedInt for &$t {
                #[inline(always)]
                fn as_c_ulonglong(&self) -> core::ffi::c_ulonglong {
                    **self as core::ffi::c_ulonglong
                }
            }
        )*
    };
}

impl_unsigned_int!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, bool);

/// Trait implemented by pointer types formatted with `%p`.
pub trait AsKPrintPointer {
    fn as_c_ptr_void(&self) -> *const core::ffi::c_void;
}

impl<T: ?Sized> AsKPrintPointer for *const T {
    #[inline(always)]
    fn as_c_ptr_void(&self) -> *const core::ffi::c_void {
        *self as *const core::ffi::c_void
    }
}

impl<T: ?Sized> AsKPrintPointer for *mut T {
    #[inline(always)]
    fn as_c_ptr_void(&self) -> *const core::ffi::c_void {
        *self as *const core::ffi::c_void
    }
}

impl<T: ?Sized> AsKPrintPointer for &T {
    #[inline(always)]
    fn as_c_ptr_void(&self) -> *const core::ffi::c_void {
        *self as *const T as *const core::ffi::c_void
    }
}

impl<T: ?Sized> AsKPrintPointer for &mut T {
    #[inline(always)]
    fn as_c_ptr_void(&self) -> *const core::ffi::c_void {
        *self as *const T as *const core::ffi::c_void
    }
}

impl AsKPrintPointer for usize {
    #[inline(always)]
    fn as_c_ptr_void(&self) -> *const core::ffi::c_void {
        *self as *const core::ffi::c_void
    }
}

/// Trait implemented by types that can be formatted as a character (`{:c}`).
pub trait AsKPrintChar {
    fn kprint_encode_char(&self, buf: &mut [u8; 4])
    -> (core::ffi::c_int, *const core::ffi::c_char);
}

impl AsKPrintChar for char {
    #[inline(always)]
    fn kprint_encode_char(
        &self,
        buf: &mut [u8; 4],
    ) -> (core::ffi::c_int, *const core::ffi::c_char) {
        let s = self.encode_utf8(buf);
        (s.len() as core::ffi::c_int, s.as_ptr() as *const core::ffi::c_char)
    }
}

impl AsKPrintChar for &char {
    #[inline(always)]
    fn kprint_encode_char(
        &self,
        buf: &mut [u8; 4],
    ) -> (core::ffi::c_int, *const core::ffi::c_char) {
        (*self).kprint_encode_char(buf)
    }
}

impl AsKPrintChar for u8 {
    #[inline(always)]
    fn kprint_encode_char(
        &self,
        buf: &mut [u8; 4],
    ) -> (core::ffi::c_int, *const core::ffi::c_char) {
        buf[0] = *self;
        (1, buf.as_ptr() as *const core::ffi::c_char)
    }
}

impl AsKPrintChar for &u8 {
    #[inline(always)]
    fn kprint_encode_char(
        &self,
        buf: &mut [u8; 4],
    ) -> (core::ffi::c_int, *const core::ffi::c_char) {
        (*self).kprint_encode_char(buf)
    }
}
