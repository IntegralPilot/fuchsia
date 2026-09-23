// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::NonZero;
use std::os::raw::{c_char, c_int};

unsafe extern "C" {
    fn LZ4_compress_default(
        src: *const c_char,
        dst: *mut c_char,
        srcSize: c_int,
        dstCapacity: c_int,
    ) -> c_int;
    fn LZ4_decompress_safe(
        src: *const c_char,
        dst: *mut c_char,
        compressedSize: c_int,
        dstCapacity: c_int,
    ) -> c_int;
    fn LZ4_compressBound(inputSize: c_int) -> c_int;
    fn LZ4_compress_HC(
        src: *const c_char,
        dst: *mut c_char,
        srcSize: c_int,
        dstCapacity: c_int,
        compressionLevel: c_int,
    ) -> c_int;
}

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum Error {
    #[error("LZ4 only supports compressing up to 2016MiB")]
    InputTooLarge,
    #[error("LZ4 decompression failed")]
    DecompressionFailed,
}

fn compress_bound(input_size: c_int) -> Result<NonZero<c_int>, Error> {
    // SAFETY: no unsafe parameters.
    let bound = unsafe { LZ4_compressBound(input_size) };
    NonZero::<c_int>::new(bound).ok_or(Error::InputTooLarge)
}

/// Compresses the given data using LZ4.
pub fn compress(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let src_size = data.len() as c_int;
    let bound = unsafe { LZ4_compressBound(src_size) };
    let mut compressed = vec![0u8; bound as usize];
    let compressed_size = unsafe {
        LZ4_compress_default(
            data.as_ptr() as *const c_char,
            compressed.as_mut_ptr() as *mut c_char,
            src_size,
            bound,
        )
    };
    assert!(compressed_size > 0, "LZ4 compression failed");
    compressed.truncate(compressed_size as usize);
    compressed
}

/// Decompresses the given data using LZ4, expecting the given uncompressed size.
pub fn decompress(data: &[u8], uncompressed_size: usize) -> Result<Vec<u8>, i32> {
    let mut decompressed = vec![0u8; uncompressed_size];
    let bytes_written = decompress_into(data, &mut decompressed).map_err(|_| -1)?;
    if bytes_written != uncompressed_size {
        return Err(-1);
    }
    Ok(decompressed)
}

/// Decompresses the given data into `destination` using LZ4. The number of bytes written to
/// `destination` is returned.
pub fn decompress_into(data: &[u8], destination: &mut [u8]) -> Result<usize, Error> {
    if data.is_empty() {
        return Ok(0);
    }
    if destination.is_empty() {
        // The pointer of an empty byte slice in rust points to 0x1. LZ4 subtracts some constants
        // from the pointer before checking if the output size is 0. The subtraction causes an
        // overflow which is undefined behaviour and gets caught by asan.
        //
        // `data` is not empty and `destination` is empty so LZ4 would return an error because not
        // all of `data` could be decompressed into `destination`.
        return Err(Error::DecompressionFailed);
    }
    let result = unsafe {
        LZ4_decompress_safe(
            data.as_ptr() as *const c_char,
            destination.as_mut_ptr() as *mut c_char,
            data.len().try_into().map_err(|_| Error::InputTooLarge)?,
            destination.len().try_into().map_err(|_| Error::InputTooLarge)?,
        )
    };
    if result < 0 { Err(Error::DecompressionFailed) } else { Ok(result as usize) }
}

/// The compression level to use with `compress_hc`.
#[derive(Copy, Clone)]
pub struct HcCompressionLevel(i32);

impl HcCompressionLevel {
    pub const MIN: Self = Self(3);
    pub const DEFAULT: Self = Self(9);
    pub const OPT_MIN: Self = Self(10);
    pub const MAX: Self = Self(12);

    pub fn custom(level: i32) -> Self {
        Self(level)
    }
}

impl From<HcCompressionLevel> for i32 {
    fn from(level: HcCompressionLevel) -> Self {
        level.0
    }
}

/// Compresses the given data using LZ4 HC.
pub fn compress_hc(data: &[u8], compression_level: HcCompressionLevel) -> Result<Vec<u8>, Error> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let data_size = i32::try_from(data.len()).map_err(|_| Error::InputTooLarge)?;
    let bound = compress_bound(data_size)?.get();
    let mut compressed = Vec::with_capacity(bound as usize);
    // SAFETY:
    //  1. u8, and MaybeUninit<u8> have the same size and alignment as c_char which makes reading
    //     from and writing to the casted pointers safe.
    //  2. When compression succeeds, LZ4 will have initialized all of the bytes up to
    //     `compressed_size` making the `set_len` call safe.
    unsafe {
        let dst = compressed.spare_capacity_mut()[0..bound as usize].as_mut_ptr().cast::<c_char>();
        let src = data.as_ptr().cast::<c_char>();
        let compressed_size = LZ4_compress_HC(src, dst, data_size, bound, compression_level.into());
        // Compression is guaranteed to succeed when the size of the destination buffer is at least
        // `LZ4_compressBound(src_size)`.
        assert!(compressed_size > 0, "LZ4 compression failed");
        compressed.set_len(compressed_size as usize);
    }
    Ok(compressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let data = b"Hello, world! LZ4 compression test.";
        let compressed = compress(data);
        let decompressed = decompress(&compressed, data.len()).unwrap();
        assert_eq!(data.as_slice(), decompressed.as_slice());
    }

    #[test]
    fn test_empty() {
        let data = b"";
        let compressed = compress(data);
        let decompressed = decompress(&compressed, 0).unwrap();
        assert_eq!(data.as_slice(), decompressed.as_slice());
    }

    #[test]
    fn test_compress_bound() {
        assert!(compress_bound(0).is_ok());
        assert!(compress_bound(20).is_ok());
        assert!(compress_bound(i32::MAX).is_err());
        assert!(compress_bound(0x7E000000).is_ok());
        assert!(compress_bound(0x7E000000 + 1).is_err());
    }

    #[test]
    fn test_hc_roundtrip() {
        let data = b"Hello, world! LZ4 compression test.";
        let compressed = compress_hc(data, HcCompressionLevel::MAX).unwrap();
        let decompressed = decompress(&compressed, data.len()).unwrap();
        assert_eq!(data.as_slice(), decompressed.as_slice());
    }

    #[test]
    fn test_hc_empty_roundtrip() {
        let data = b"";
        let compressed = compress_hc(data, HcCompressionLevel::MAX).unwrap();
        let decompressed = decompress(&compressed, data.len()).unwrap();
        assert_eq!(data.as_slice(), decompressed.as_slice());
    }
    #[test]
    fn test_decompress_into_zero_buffer() {
        let data = b"Hello, world! LZ4 compression test.";
        let compressed = compress_hc(data, HcCompressionLevel::MAX).unwrap();
        decompress_into(&compressed, &mut Vec::new()).expect_err("buf should be too small");
    }

    #[test]
    fn test_decompress_into_buffer_too_small() {
        let data = b"Hello, world! LZ4 compression test.";
        let compressed = compress_hc(data, HcCompressionLevel::MAX).unwrap();
        let mut buf = vec![0; data.len() - 1];
        decompress_into(&compressed, &mut buf).expect_err("buf should be too small");
    }

    #[test]
    fn test_decompress_into_large_buffer() {
        let data = b"Hello, world! LZ4 compression test.";
        let compressed = compress_hc(data, HcCompressionLevel::MAX).unwrap();
        let mut buf = vec![0; data.len() + 1];
        let bytes_written = decompress_into(&compressed, &mut buf).unwrap();
        assert_eq!(data.as_slice(), &buf[0..bytes_written]);
    }

    #[test]
    fn test_decompress_into_exact_sized_buffer() {
        let data = b"Hello, world! LZ4 compression test.";
        let compressed = compress_hc(data, HcCompressionLevel::MAX).unwrap();
        let mut buf = vec![0; data.len()];
        let bytes_written = decompress_into(&compressed, &mut buf).unwrap();
        assert_eq!(data.len(), bytes_written);
        assert_eq!(data.as_slice(), buf.as_slice());
    }

    #[test]
    fn test_empty_decompress_into_large_buffer() {
        let mut buf = vec![0; 100];
        let decompressed_bytes =
            decompress_into(&[], &mut buf).expect("decompression should succeed");
        assert_eq!(decompressed_bytes, 0);
    }
}
