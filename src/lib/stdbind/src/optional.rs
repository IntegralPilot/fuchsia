// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// An ABI-stable adapter for `Option<T>` designed for FFI bindings that use
/// the C++ `stdbind::optional<T>`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Optional<T> {
    None,
    Some(T),
}

impl<T> From<Option<T>> for Optional<T> {
    #[inline]
    fn from(opt: Option<T>) -> Self {
        match opt {
            Some(val) => Optional::Some(val),
            None => Optional::None,
        }
    }
}

impl<T> From<Optional<T>> for Option<T> {
    #[inline]
    fn from(opt: Optional<T>) -> Self {
        match opt {
            Optional::Some(val) => Some(val),
            Optional::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversions() {
        let std_opt = Some(123u64);
        let stdbind_opt: Optional<u64> = std_opt.into();
        assert_eq!(stdbind_opt, Optional::Some(123));

        let back_to_std: Option<u64> = stdbind_opt.into();
        assert_eq!(back_to_std, Some(123));

        let none_std: Option<u64> = None;
        let none_stdbind: Optional<u64> = none_std.into();
        assert_eq!(none_stdbind, Optional::None);

        let back_to_none: Option<u64> = none_stdbind.into();
        assert_eq!(back_to_none, None);
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[test]
    fn test_layouts() {
        assert_eq!(size_of::<Optional<u8>>(), 16);
        assert_eq!(align_of::<Optional<u8>>(), 8);

        assert_eq!(size_of::<Optional<u16>>(), 16);
        assert_eq!(align_of::<Optional<u16>>(), 8);

        assert_eq!(size_of::<Optional<u32>>(), 16);
        assert_eq!(align_of::<Optional<u32>>(), 8);

        assert_eq!(size_of::<Optional<u64>>(), 16);
        assert_eq!(align_of::<Optional<u64>>(), 8);

        assert_eq!(size_of::<Optional<Point>>(), 16);
        assert_eq!(align_of::<Optional<Point>>(), 8);
    }

    #[test]
    fn test_discriminants() {
        let none: Optional<u32> = Optional::None;
        assert_eq!(unsafe { *(core::ptr::addr_of!(none) as *const u64) }, 0);

        let some: Optional<u32> = Optional::Some(0x12345678);
        assert_eq!(unsafe { *(core::ptr::addr_of!(some) as *const u64) }, 1);
        assert_eq!(
            unsafe { *((core::ptr::addr_of!(some) as *const u8).add(8) as *const u32) },
            0x12345678
        );
    }
}
