// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// An embeddable structure guard.
///
/// To use `fbl::Canary`, choose a 4-byte guard value.
///
/// You can use the `Canary::new()` method to instantiate a `Canary` object.
/// The compiler will infer the magic value from the type definition.
///
/// ```rust
/// struct MyStruct {
///     canary: fbl::Canary<{ fbl::magic(b"guar") }>,
///     // ...
/// }
///
/// impl MyStruct {
///     fn new() -> Self {
///         MyStruct {
///             canary: fbl::Canary::new(),
///             // ...
///         }
///     }
/// }
/// ```
///
/// The canary initializes itself with the guard value during construction and
/// checks it during destruction (on `Drop`). You can also manually check the
/// value during the lifetime of your object by calling the `assert` method.
///
/// If the value is not an ASCII string, you can directly use an integer literal
/// as the const generic parameter.
///
/// ```rust
/// struct MyStruct {
///     canary: fbl::Canary<0x12345678>,
///     // ...
/// }
/// ```
#[repr(C)]
pub struct Canary<const MAGIC: u32> {
    magic: u32,
}

impl<const MAGIC: u32> Canary<MAGIC> {
    /// Create a new Canary with the specified magic value.
    pub const fn new() -> Self {
        Canary { magic: MAGIC }
    }

    /// Assert that the value of `magic` is as expected.
    ///
    /// # Panics
    ///
    /// Panics if `self.magic` is not the expected value.
    pub fn assert(&self) {
        let observed_magic = unsafe { core::ptr::read_volatile(&self.magic) };
        if observed_magic != MAGIC {
            panic!("Invalid canary (expt: {:08x}, got: {:08x})", MAGIC, observed_magic);
        }
    }

    /// Some places have special handling of bad magic values. For these
    /// cases, simply return whether the `magic` is correct, and let
    /// them respond appropriately if not.
    pub fn valid(&self) -> bool {
        let observed_magic = unsafe { core::ptr::read_volatile(&self.magic) };
        observed_magic == MAGIC
    }
}

impl<const MAGIC: u32> Default for Canary<MAGIC> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const MAGIC: u32> Drop for Canary<MAGIC> {
    fn drop(&mut self) {
        self.assert();
        unsafe {
            core::ptr::write_volatile(&mut self.magic, 0);
        }
    }
}

/// Function for generating canary magic values from strings
pub const fn magic(str: &[u8; 4]) -> u32 {
    ((str[0] as u32) << 24) | ((str[1] as u32) << 16) | ((str[2] as u32) << 8) | (str[3] as u32)
}

#[macro_export]
macro_rules! canary {
    ($str:literal) => {
        $crate::Canary::<{ $crate::magic($str) }>::new()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canary_default() {
        let canary: Canary<{ magic(b"test") }> = Default::default();
        assert!(canary.valid());
    }

    #[test]
    fn test_magic_runtime() {
        let m = magic(b"abcd");
        assert_eq!(m, 0x61626364);
    }

    #[test]
    fn test_canary() {
        let canary = canary!(b"test");
        assert!(canary.valid());
        canary.assert();
    }

    #[test]
    #[should_panic(expected = "Invalid canary")]
    fn test_canary_corruption() {
        let mut canary = canary!(b"test");
        // Corrupt the canary storage directly
        unsafe {
            core::ptr::write_volatile(&mut canary.magic, 0);
        }
        canary.assert();
    }

    unsafe extern "C" {
        fn check_rust_canary(ptr: *const core::ffi::c_void, expected_magic: u32) -> bool;
    }

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_canary_ffi() {
        const MAGIC_VAL: u32 = 0x12345678; // Must match the hardcoded value in C++ helper
        let canary = Canary::<MAGIC_VAL>::new();
        unsafe {
            assert!(check_rust_canary(&canary as *const _ as *const core::ffi::c_void, MAGIC_VAL));
        }
    }
}
