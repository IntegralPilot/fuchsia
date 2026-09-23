// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use ksync::{KCell, KMutex, guarded, lock};
use pin_init::{PinInit, pin_init};

pub type NameLock = ksync::RawSpinlock;

/// A fixed-size, thread-safe name buffer with automatic truncation and null-termination.
///
/// Names include the trailing null byte as part of their `SIZE`-sized buffer.
/// Constructors and setters automatically truncate inputs to `SIZE - 1` bytes to ensure
/// the string is always null-terminated.
///
/// Corresponds to `fbl::Name<Size>` in C++.
#[guarded]
#[repr(C)]
pub struct Name<const SIZE: usize> {
    #[mutex]
    lock: KMutex<NameLock>,
    #[guarded_by(lock)]
    name: [u8; SIZE],
}

impl<const SIZE: usize> Name<SIZE> {
    /// Asserts that `SIZE >= 1` at compile time.
    const CHECK_SIZE: () = assert!(SIZE >= 1, "Names must have SIZE >= 1");

    /// Copies `name` into `dst`, truncating at the first null byte or `SIZE - 1`
    /// (whichever comes first) and zeroing the remainder of `dst`.
    fn copy_and_truncate(dst: &mut [u8; SIZE], name: &[u8]) {
        let nul_idx = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        let len = nul_idx.min(SIZE - 1);
        dst[..len].copy_from_slice(&name[..len]);
        dst[len..].fill(0);
    }

    /// Creates a `PinInit` initializer for `Name` initialized to an empty string.
    ///
    /// Initializes storage directly in-place without stack copies.
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        let () = Self::CHECK_SIZE;
        pin_init!(Self {
            lock <- KMutex::init(),
            name: KCell::new([0u8; SIZE]),
        })
    }

    /// Returns the length of the string (excluding the null terminator).
    pub fn len(&self) -> usize {
        lock!(let guard = self.lock_lock());
        let fields = guard.fields();
        fields.name.iter().position(|&b| b == 0).unwrap_or(SIZE - 1)
    }

    /// Returns true if the string is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copies the name out into `out_name`.
    ///
    /// `out_name` is zeroed first. If `out_name` is non-empty, up to
    /// `min(out_name.len() - 1, SIZE - 1)` characters are copied up to the first null byte.
    /// If `out_name` is empty, no data is written.
    pub fn get(&self, out_name: &mut [u8]) {
        out_name.fill(0);
        let Some((_nul, dst)) = out_name.split_last_mut() else {
            return;
        };
        lock!(let guard = self.lock_lock());
        let fields = guard.fields();
        let nul_idx = fields.name.iter().position(|&b| b == 0).unwrap_or(SIZE - 1);
        let copy_len = dst.len().min(nul_idx);
        dst[..copy_len].copy_from_slice(&fields.name[..copy_len]);
    }

    /// Resets the `Name` to the given data under lock.
    ///
    /// Any characters after the first null byte are ignored. If `name` is longer than
    /// `SIZE - 1`, it will be truncated to ensure null-termination. The remainder of the
    /// internal buffer is filled with null bytes.
    pub fn set(&self, name: &[u8]) {
        lock!(let guard = self.lock_lock());
        let fields = guard.fields_mut();
        Self::copy_and_truncate(fields.name, name);
    }

    /// Resets the `Name` to the given string slice under lock.
    pub fn set_str(&self, name: &str) {
        self.set(name.as_bytes())
    }

    /// Copies the internal name into a newly stack-allocated buffer of size `SIZE`.
    pub fn copy_name(&self) -> [u8; SIZE] {
        let mut buf = [0u8; SIZE];
        self.get(&mut buf);
        buf
    }

    /// Copies the contents of `other` into `self`.
    pub fn copy_from(&self, other: &Self) {
        if !core::ptr::eq(self, other) {
            let mut buf = [0u8; SIZE];
            other.get(&mut buf);
            self.set(&buf);
        }
    }
}

impl<const SIZE: usize> core::fmt::Debug for Name<SIZE> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut buf = [0u8; SIZE];
        self.get(&mut buf);
        let nul_idx = buf.iter().position(|&b| b == 0).unwrap_or(SIZE);
        let s = core::str::from_utf8(&buf[..nul_idx]).unwrap_or("<invalid utf-8>");
        f.debug_tuple("Name").field(&s).finish()
    }
}
