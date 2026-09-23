// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::ffi::{CStr, FromBytesWithNulError};
use core::ops::{Deref, DerefMut};

/// A fixed-size buffer for assembling a string.
///
/// `StringBuffer` is designed to resemble `std::string` except that it
/// does not allocate heap storage.
///
/// # Note on Generic Parameter `N`
///
/// In C++, `StringBuffer<M>` has a capacity of `M` characters and stores `M + 1` bytes
/// (including the null terminator).
/// In Rust, to avoid unstable features (`generic_const_exprs`), the generic parameter `N`
/// represents the **total size** of the backing array.
/// Therefore, a C++ `StringBuffer<M>` corresponds to a Rust `StringBuffer<{M + 1}>`.
/// The Rust `StringBuffer<N>` can hold up to `N - 1` characters.
#[repr(C)]
pub struct StringBuffer<const N: usize> {
    /// The number of active characters in the buffer.
    ///
    /// - `length < N` to leave room for the null terminator.
    length: usize,

    /// The backing storage for the string.
    ///
    /// - `data[length]` is always `0` (null terminator).
    /// - Elements from `0` to `length - 1` are part of the string.
    data: [u8; N],
}

// On 64-bit: usize is 8 bytes. [u8; 8] is 8 bytes. Total 16 bytes. No padding.
zr::static_assert!(core::mem::size_of::<StringBuffer<8>>() == 16);
zr::static_assert!(core::mem::align_of::<StringBuffer<8>>() == core::mem::align_of::<usize>());

impl<const N: usize> StringBuffer<N> {
    const ASSERT_N_POSITIVE: () = assert!(N > 0);

    /// Creates an empty string buffer.
    pub const fn new() -> Self {
        let _ = Self::ASSERT_N_POSITIVE;

        let data = [0; N];
        Self { length: 0, data }
    }

    /// Creates a string buffer containing exactly one character and a null
    /// terminator.
    pub const fn with_char(c: u8) -> Self {
        assert!(N >= 2, "N must be at least 2 to hold a char and null terminator");
        let mut data = [0; N];
        data[0] = c;
        Self { length: 1, data }
    }

    /// Returns the capacity of the buffer (max characters it can hold).
    ///
    /// The capacity is `N - 1` because we need 1 byte for the null terminator.
    pub const fn capacity(&self) -> usize {
        N - 1
    }

    /// Returns a reference to the contents as a CStr.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer contains interior null bytes.
    pub fn as_cstr(&self) -> Result<&CStr, FromBytesWithNulError> {
        CStr::from_bytes_with_nul(&self.data[..=self.length])
    }

    /// Clears the string buffer.
    pub fn clear(&mut self) {
        self.length = 0;
        self.data[0] = 0;
    }

    /// Clears existing data from the buffer and sets the buffer to the new value, plus a null
    /// terminator.
    ///
    /// The `data` does not need to be null terminated. A terminating `0` will always be appended
    /// to the resulting string.
    ///
    /// # Panics
    ///
    /// Panics if `data.len() >= N`.
    pub fn set(&mut self, data: &[u8]) {
        let len = data.len();
        assert!(len < N, "The data and a null terminator must fit within the array.");
        self.data[..len].copy_from_slice(data);
        self.length = len;
        self.data[self.length] = 0;
    }

    /// Resizes the string buffer.
    ///
    /// If the current length is less than `count`, additional characters are appended
    /// with the value `ch`.
    ///
    /// If the current length is greater than `count`, the string is truncated.
    ///
    /// # Panics
    ///
    /// Panics if `count >= N`.
    pub fn resize(&mut self, count: usize, ch: u8) {
        assert!(count < N, "Must have room for count bytes an a null terminator within the array.");
        if self.length < count {
            self.data[self.length..count].fill(ch);
        }
        self.length = count;
        self.data[self.length] = 0;
    }

    /// Remove the first `count` characters from the string buffer.
    ///
    /// # Panics
    ///
    /// Panics if `count > self.len()`.
    pub fn remove_prefix(&mut self, count: usize) {
        assert!(count <= self.length, "Cannot remove more than current length");
        self.length -= count;
        self.data.copy_within(count..count + self.length, 0);
        self.data[self.length] = 0;
    }

    /// Appends a single character.
    ///
    /// The result is truncated if the appended content does not fit completely.
    pub fn append_char(&mut self, ch: u8) -> &mut Self {
        if self.length < self.capacity() {
            self.data[self.length] = ch;
            self.length += 1;
            self.data[self.length] = 0;
        }
        self
    }

    /// Appends content to the string buffer from a byte slice.
    ///
    /// The result is truncated if the appended content does not fit completely.
    pub fn append(&mut self, data: &[u8]) -> &mut Self {
        let remaining = self.capacity() - self.length;
        let len = core::cmp::min(data.len(), remaining);
        self.data[self.length..self.length + len].copy_from_slice(&data[..len]);
        self.length += len;
        self.data[self.length] = 0;
        self
    }
}

impl<const N: usize> Default for StringBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> core::fmt::Write for StringBuffer<N> {
    /// Appends to the string buffer from the given string.
    ///
    /// The result is truncated if the appended content does not fit completely.
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.append(s.as_bytes());
        Ok(())
    }
}

impl<const N: usize> Deref for StringBuffer<N> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data[..self.length]
    }
}

impl<const N: usize> DerefMut for StringBuffer<N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data[..self.length]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write;

    #[test]
    fn test_empty() {
        let sb: StringBuffer<11> = StringBuffer::new(); // Capacity 10
        assert_eq!(sb.len(), 0);
        assert_eq!(sb.capacity(), 10);
        assert!(sb.is_empty());
        assert_eq!(sb.data[0], 0);
    }

    #[test]
    fn test_with_char() {
        let sb: StringBuffer<11> = StringBuffer::with_char(b'a');
        assert_eq!(sb.len(), 1);
        assert_eq!(&sb[..], b"a");
    }

    #[test]
    fn test_append() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.append(b"hello");
        assert_eq!(sb.len(), 5);
        assert_eq!(&sb[..], b"hello");

        sb.append(b" world");
        assert_eq!(sb.len(), 10);
        assert_eq!(&sb[..], b"hello worl"); // Truncated
    }

    #[test]
    fn test_append_char() {
        let mut sb: StringBuffer<6> = StringBuffer::new(); // Capacity 5
        sb.append_char(b'a').append_char(b'b');
        assert_eq!(&sb[..], b"ab");
    }

    #[test]
    fn test_clear() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.append(b"hello");
        sb.clear();
        assert_eq!(sb.len(), 0);
        assert!(sb.is_empty());
    }

    #[test]
    fn test_set() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.set(b"hello");
        assert_eq!(&sb[..], b"hello");
    }

    #[test]
    fn test_resize() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.append(b"hello");
        sb.resize(3, b' ');
        assert_eq!(&sb[..], b"hel");

        sb.resize(6, b'x');
        assert_eq!(&sb[..], b"helxxx");
    }

    #[test]
    fn test_remove_prefix() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.append(b"hello");
        sb.remove_prefix(2);
        assert_eq!(&sb[..], b"llo");
    }

    #[test]
    fn test_write_macro() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        write!(sb, "test {}", 123).unwrap();
        assert_eq!(&sb[..], b"test 123");

        write!(sb, "more").unwrap();
        assert_eq!(&sb[..], b"test 123mo"); // Truncated
    }

    #[test]
    fn test_index() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.append(b"hello");
        assert_eq!(sb[0], b'h');
        assert_eq!(sb[4], b'o');
    }

    #[test]
    fn test_default() {
        let sb: StringBuffer<11> = Default::default();
        assert_eq!(sb.len(), 0);
        assert_eq!(sb.capacity(), 10);
    }

    #[test]
    fn test_constructor_zero() {
        let sb: StringBuffer<1> = StringBuffer::new(); // Capacity 0
        assert_eq!(sb.len(), 0);
        assert_eq!(sb.capacity(), 0);
        assert!(sb.is_empty());
    }

    #[test]
    fn test_modify() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.append(b"hello");
        sb[0] = b'j';
        assert_eq!(&sb[..], b"jello");
    }

    #[test]
    fn test_deref() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.append(b"hello");
        let slice: &[u8] = &sb;
        assert_eq!(slice, b"hello");

        let slice_mut: &mut [u8] = &mut sb;
        slice_mut[0] = b'H';
        assert_eq!(&sb[..], b"Hello");
    }

    #[test]
    fn test_resize_to_max() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.resize(10, b'x');
        assert_eq!(sb.len(), 10);
        assert_eq!(&sb[..], b"xxxxxxxxxx");
        // Verify null terminator is at index 10
        assert_eq!(sb.data[10], 0);
    }

    #[test]
    fn test_as_cstr_success() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.set(b"hello");
        let cstr = sb.as_cstr().unwrap();
        assert_eq!(cstr.to_bytes(), b"hello");
    }

    #[test]
    fn test_as_cstr_interior_null() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.set(b"a\0b");
        assert!(sb.as_cstr().is_err());
    }

    #[test]
    fn test_append_chaining() {
        let mut sb: StringBuffer<11> = StringBuffer::new();
        sb.append(b"hello").append(b" world");
        assert_eq!(&sb[..], b"hello worl"); // Truncated
    }
}
