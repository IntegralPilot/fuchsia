// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! C-compatible memory-backed file buffer for FFI.

use std::ffi::CStr;
use std::marker::PhantomData;

unsafe extern "C" {
    fn fmemopen(
        buf: *mut libc::c_void,
        size: libc::size_t,
        mode: *const libc::c_char,
    ) -> *mut libc::FILE;
}

/// A wrapper around the raw `FILE*` that borrows `CFileBuffer` mutably.
/// This ensures exclusive access to the file descriptor.
pub struct CFilePtr<'a> {
    ptr: *mut libc::FILE,
    _marker: PhantomData<&'a mut CFileBuffer>,
}

impl<'a> CFilePtr<'a> {
    /// Get the raw `FILE*` pointer.
    ///
    /// # Safety Constraints
    /// The returned raw pointer must not be stored or used after this `CFilePtr`
    /// (or the parent `CFileBuffer`) is dropped. The caller must ensure that the
    /// parent `CFileBuffer` outlives any use of the raw pointer.
    pub fn as_raw(&self) -> *mut libc::FILE {
        self.ptr
    }
}

/// A C-compatible memory-backed file buffer.
///
/// This struct wraps a `FILE*` created via `fmemopen`, allowing FFI functions
/// to write to a memory buffer that can then be read from Rust.
pub struct CFileBuffer {
    file: *mut libc::FILE,
    buffer: Vec<u8>,
}

// SAFETY: CFileBuffer owns the FILE* and the buffer, and does not share them concurrently.
// It is safe to transfer ownership of CFileBuffer to another thread.
unsafe impl Send for CFileBuffer {}

impl CFileBuffer {
    /// Create a new CFileBuffer object with a buffer of `max_size`.
    pub fn new(max_size: usize) -> Result<Self, &'static str> {
        let mut buffer = vec![0u8; max_size];

        // Open the file in "w+" mode. We use "w+" instead of "w" because in "w"
        // mode, musl's fmemopen will overwrite the last byte with a null character
        // when the buffer is full, corrupting the data if we write exactly `max_size`
        // bytes. "w+" mode avoids this behavior.
        let mode = CStr::from_bytes_with_nul(b"w+\0").unwrap();

        // SAFETY: The buffer is owned by the CFileBuffer struct and will remain
        // valid for the lifetime of the returned FILE*. The mode string is null-terminated.
        let file =
            unsafe { fmemopen(buffer.as_mut_ptr() as *mut libc::c_void, max_size, mode.as_ptr()) };

        if file.is_null() {
            return Err("fmemopen failed");
        }

        Ok(Self { file, buffer })
    }

    /// Get the raw FILE pointer wrapper.
    /// This mutably borrows `self`, ensuring no other borrows (like `data()`)
    /// can exist while this wrapper is alive.
    pub fn file(&mut self) -> CFilePtr<'_> {
        CFilePtr { ptr: self.file, _marker: PhantomData }
    }

    /// Flush the stream and return a slice over the data that has been written.
    pub fn data(&self) -> &[u8] {
        // Flush to ensure all data is written to the buffer.
        // SAFETY: self.file is a valid FILE pointer created during construction.
        unsafe {
            libc::fflush(self.file);
        }

        // Find the current position in the file.
        // SAFETY: self.file is a valid FILE pointer created during construction.
        let pos = unsafe { libc::ftell(self.file) };
        if pos < 0 {
            return &[];
        }

        let len = pos as usize;
        let len = std::cmp::min(len, self.buffer.len());

        &self.buffer[..len]
    }

    /// Reset the position of the FILE* to the start of the file.
    pub fn reset(&self) -> Result<(), &'static str> {
        // SAFETY: self.file is a valid FILE pointer created during construction.
        let ret = unsafe { libc::fseek(self.file, 0, libc::SEEK_SET) };
        if ret != 0 {
            return Err("fseek failed");
        }
        Ok(())
    }
}

impl Drop for CFileBuffer {
    fn drop(&mut self) {
        // SAFETY: self.file is a valid FILE pointer that has not been closed yet.
        unsafe {
            libc::fclose(self.file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read() {
        let mut fmem = CFileBuffer::new(1024).unwrap();

        // Write some data using libc
        let data_to_write = b"hello world";
        // SAFETY: The file pointer is valid, and the data buffer is valid for the duration of the write.
        unsafe {
            let file_ptr = fmem.file();
            assert!(!file_ptr.as_raw().is_null());
            libc::fwrite(
                data_to_write.as_ptr() as *const libc::c_void,
                1,
                data_to_write.len(),
                file_ptr.as_raw(),
            );
        }

        // Read data back
        let written = fmem.data();
        assert_eq!(written, data_to_write);
    }

    #[test]
    fn test_reset() {
        let mut fmem = CFileBuffer::new(1024).unwrap();

        let data1 = b"abc";
        // SAFETY: The file pointer is valid, and the data buffer is valid for the duration of the write.
        unsafe {
            let file_ptr = fmem.file();
            libc::fwrite(data1.as_ptr() as *const libc::c_void, 1, data1.len(), file_ptr.as_raw());
        }
        assert_eq!(fmem.data(), data1);

        fmem.reset().unwrap();
        assert_eq!(fmem.data(), b"");

        let data2 = b"defgh";
        // SAFETY: The file pointer is valid, and the data buffer is valid for the duration of the write.
        unsafe {
            let file_ptr = fmem.file();
            libc::fwrite(data2.as_ptr() as *const libc::c_void, 1, data2.len(), file_ptr.as_raw());
        }
        assert_eq!(fmem.data(), data2);
    }

    #[test]
    fn test_overflow() {
        let mut fmem = CFileBuffer::new(5).unwrap();

        let data = b"abcdefg";
        // SAFETY: The file pointer is valid, and the data buffer is valid for the duration of the write.
        unsafe {
            let file_ptr = fmem.file();
            libc::fwrite(data.as_ptr() as *const libc::c_void, 1, data.len(), file_ptr.as_raw());
        }
        // It should be capped at 5
        assert_eq!(fmem.data(), b"abcde");
    }

    #[test]
    fn test_exact_size() {
        let mut fmem = CFileBuffer::new(5).unwrap();

        let data = b"12345";
        // SAFETY: The file pointer is valid, and the data buffer is valid for the duration of the write.
        unsafe {
            let file_ptr = fmem.file();
            libc::fwrite(data.as_ptr() as *const libc::c_void, 1, data.len(), file_ptr.as_raw());
        }
        assert_eq!(fmem.data(), b"12345");
    }
}
