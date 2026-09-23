// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Compression algorithms supported by chunked-compression and corresponding compressors and
//! decompressors.
//!
//! The compressors and decompressors are enums rather than traits with multiple implementations
//! because the enums are small and avoid the heap allocation of `Box<dyn Decompressor>`.

use super::{FormatError, ZstdError};
use crate::compression::ChunkedArchiveError;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use storage_ptr_slice::PtrByteSlice;
use zstd::zstd_safe::zstd_sys;

thread_local! {
    static ZSTD_COMPRESSOR: std::cell::RefCell<zstd::zstd_safe::CCtx<'static>> =
        std::cell::RefCell::new({
            let mut cctx = zstd::zstd_safe::CCtx::create();
            cctx.set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true)).unwrap();
            cctx
        });
    static ZSTD_DECOMPRESSOR: std::cell::RefCell<RawDCtx> = {
        // SAFETY: Creating a ZSTD decompression context does not access
        // invalid memory or violate invariants.
        let raw_ptr = unsafe { zstd_sys::ZSTD_createDCtx() };
        let ptr = NonNull::new(raw_ptr).expect("ZSTD_createDCtx failed");
        std::cell::RefCell::new(RawDCtx(ptr))
    };
}

struct RawDCtx(NonNull<zstd_sys::ZSTD_DCtx>);
impl Drop for RawDCtx {
    fn drop(&mut self) {
        // SAFETY: `self.0` is non-null and was allocated via `ZSTD_createDCtx`.
        unsafe {
            zstd_sys::ZSTD_freeDCtx(self.0.as_ptr());
        }
    }
}

/// Decompresses ZSTD using a pointer slice and a raw destination pointer slice.
///
/// # Safety
///
/// `dst` must point to valid memory allocated for writes of at least
/// `dst.len()` bytes.
unsafe fn zstd_decompress_ptr(
    src: PtrByteSlice<'_>,
    dst: *mut [MaybeUninit<u8>],
) -> Result<usize, usize> {
    ZSTD_DECOMPRESSOR.with_borrow_mut(|decompressor| {
        let dctx = decompressor.0.as_ptr();
        // SAFETY: `PtrByteSlice` guarantees `src` is valid for reads. `dst`
        // points to valid memory allocated for writes of at least `dst.len()`
        // bytes (ensured by caller safety preconditions).
        let result = unsafe {
            zstd_sys::ZSTD_decompressDCtx(
                dctx,
                dst as *mut std::os::raw::c_void,
                dst.len(),
                src.as_raw_slice_ptr() as *const std::os::raw::c_void,
                src.len(),
            )
        };
        // SAFETY: Checking an integer return code is pure inspection.
        if unsafe { zstd_sys::ZSTD_isError(result) } != 0 { Err(result) } else { Ok(result) }
    })
}

unsafe extern "C" {
    fn LZ4_decompress_safe(
        src: *const std::os::raw::c_char,
        dst: *mut std::os::raw::c_char,
        compressedSize: std::os::raw::c_int,
        dstCapacity: std::os::raw::c_int,
    ) -> std::os::raw::c_int;
}

/// Decompresses LZ4 using a pointer slice and a raw destination pointer slice.
///
/// # Safety
///
/// `dst` must point to valid memory allocated for writes of at least
/// `dst.len()` bytes.
unsafe fn lz4_decompress_ptr(
    src: PtrByteSlice<'_>,
    dst: *mut [MaybeUninit<u8>],
) -> Result<usize, lz4::Error> {
    if src.is_empty() {
        return Ok(0);
    }
    if dst.len() == 0 {
        return Err(lz4::Error::DecompressionFailed);
    }
    // SAFETY: `PtrByteSlice` guarantees `src` is valid for reads. `dst`
    // points to valid memory allocated for writes of at least `dst.len()`
    // bytes (ensured by caller safety preconditions).
    let result = unsafe {
        LZ4_decompress_safe(
            src.as_raw_slice_ptr() as *const std::os::raw::c_char,
            dst as *mut std::os::raw::c_char,
            src.len().try_into().map_err(|_| lz4::Error::InputTooLarge)?,
            dst.len().try_into().map_err(|_| lz4::Error::InputTooLarge)?,
        )
    };
    if result < 0 { Err(lz4::Error::DecompressionFailed) } else { Ok(result as usize) }
}

/// The compression algorithm used to compress the chunks.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CompressionAlgorithm {
    Zstd = 0,
    Lz4 = 1,
}

impl CompressionAlgorithm {
    /// Returns a decompressor that can decompress a chunk compressed with this compression
    /// algorithm.
    pub fn decompressor(&self) -> Decompressor {
        match self {
            Self::Zstd => Decompressor::Zstd,
            Self::Lz4 => Decompressor::Lz4,
        }
    }

    /// Returns a decompressor that can decompress a chunk compressed with this compression
    /// algorithm. Some decompressors require a large state object that is expensive to create but
    /// can be reused for many decompressions. A thread-local decompressor stores the state object
    /// in a thread-local variable.
    pub fn thread_local_decompressor(&self) -> ThreadLocalDecompressor {
        match self {
            Self::Zstd => ThreadLocalDecompressor::Zstd,
            Self::Lz4 => ThreadLocalDecompressor::Lz4,
        }
    }
}

impl From<CompressionAlgorithm> for u8 {
    fn from(value: CompressionAlgorithm) -> Self {
        value as u8
    }
}

impl TryFrom<u8> for CompressionAlgorithm {
    type Error = ChunkedArchiveError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(CompressionAlgorithm::Zstd),
            1 => Ok(CompressionAlgorithm::Lz4),
            _ => Err(ChunkedArchiveError::IntegrityError),
        }
    }
}

/// A decompressor that is capable of decompressing chunks of a compressed archive.
pub enum Decompressor {
    Zstd,
    Lz4,
}

impl Decompressor {
    /// Decompresses a chunk of a chunked-compression archive.
    pub fn decompress<'a>(
        &mut self,
        data: impl Into<PtrByteSlice<'a>>,
        uncompressed_size: usize,
        chunk_index: usize,
    ) -> Result<Vec<u8>, ChunkedArchiveError> {
        let src = data.into();
        let mut buffer = Vec::with_capacity(uncompressed_size);
        let dst = buffer.spare_capacity_mut() as *mut [MaybeUninit<u8>];
        let len = match self {
            Self::Zstd => {
                // SAFETY: `dst` points to `uncompressed_size` bytes of capacity
                // in `buffer`.
                unsafe { zstd_decompress_ptr(src, dst) }.map_err(|code| {
                    ChunkedArchiveError::DecompressionError {
                        index: chunk_index,
                        error: FormatError::Zstd(ZstdError(code)),
                    }
                })?
            }
            Self::Lz4 => {
                // SAFETY: `dst` points to `uncompressed_size` bytes of capacity
                // in `buffer`.
                unsafe { lz4_decompress_ptr(src, dst) }.map_err(|e| {
                    ChunkedArchiveError::DecompressionError {
                        index: chunk_index,
                        error: FormatError::Lz4(e),
                    }
                })?
            }
        };
        // SAFETY: Decompression wrote `len` initialized bytes into `buffer`.
        unsafe {
            buffer.set_len(len);
        }
        Ok(buffer)
    }

    /// Decompresses a chunk of a chunked-compression archive into a pre-allocated buffer.
    pub fn decompress_into<'a>(
        &mut self,
        data: impl Into<PtrByteSlice<'a>>,
        destination: &mut [u8],
        chunk_index: usize,
    ) -> Result<usize, ChunkedArchiveError> {
        let src = data.into();
        let dst = destination as *mut [u8] as *mut [MaybeUninit<u8>];
        match self {
            Self::Zstd => {
                // SAFETY: `dst` points to `destination.len()` bytes of valid
                // memory in `destination`.
                unsafe { zstd_decompress_ptr(src, dst) }.map_err(|code| {
                    ChunkedArchiveError::DecompressionError {
                        index: chunk_index,
                        error: FormatError::Zstd(ZstdError(code)),
                    }
                })
            }
            Self::Lz4 => {
                // SAFETY: `dst` points to `destination.len()` bytes of valid
                // memory in `destination`.
                unsafe { lz4_decompress_ptr(src, dst) }.map_err(|e| {
                    ChunkedArchiveError::DecompressionError {
                        index: chunk_index,
                        error: FormatError::Lz4(e),
                    }
                })
            }
        }
    }
}

#[derive(Copy, Clone)]
/// A decompressor that uses thread-local storage to avoid reallocation of large state objects.
pub enum ThreadLocalDecompressor {
    Zstd,
    Lz4,
}

impl ThreadLocalDecompressor {
    /// Decompresses a chunk of a chunked-compression archive.
    pub fn decompress<'a>(
        &self,
        data: impl Into<PtrByteSlice<'a>>,
        uncompressed_size: usize,
        chunk_index: usize,
    ) -> Result<Vec<u8>, ChunkedArchiveError> {
        let src = data.into();
        let mut buffer = Vec::with_capacity(uncompressed_size);
        let dst = buffer.spare_capacity_mut() as *mut [MaybeUninit<u8>];
        let len = match self {
            Self::Zstd => {
                // SAFETY: `dst` points to `uncompressed_size` bytes of capacity
                // in `buffer`.
                unsafe { zstd_decompress_ptr(src, dst) }.map_err(|code| {
                    ChunkedArchiveError::DecompressionError {
                        index: chunk_index,
                        error: FormatError::Zstd(ZstdError(code)),
                    }
                })?
            }
            Self::Lz4 => {
                // SAFETY: `dst` points to `uncompressed_size` bytes of capacity
                // in `buffer`.
                unsafe { lz4_decompress_ptr(src, dst) }.map_err(|e| {
                    ChunkedArchiveError::DecompressionError {
                        index: chunk_index,
                        error: FormatError::Lz4(e),
                    }
                })?
            }
        };
        // SAFETY: Decompression wrote `len` initialized bytes into `buffer`.
        unsafe {
            buffer.set_len(len);
        }
        Ok(buffer)
    }

    /// Decompresses a chunk of a chunked-compression archive into a pre-allocated buffer.
    pub fn decompress_into<'a>(
        &self,
        data: impl Into<PtrByteSlice<'a>>,
        destination: &mut [u8],
        chunk_index: usize,
    ) -> Result<usize, ChunkedArchiveError> {
        let src = data.into();
        let dst = destination as *mut [u8] as *mut [MaybeUninit<u8>];
        match self {
            Self::Zstd => {
                // SAFETY: `dst` points to `destination.len()` bytes of valid
                // memory in `destination`.
                unsafe { zstd_decompress_ptr(src, dst) }.map_err(|code| {
                    ChunkedArchiveError::DecompressionError {
                        index: chunk_index,
                        error: FormatError::Zstd(ZstdError(code)),
                    }
                })
            }
            Self::Lz4 => {
                // SAFETY: `dst` points to `destination.len()` bytes of valid
                // memory in `destination`.
                unsafe { lz4_decompress_ptr(src, dst) }.map_err(|e| {
                    ChunkedArchiveError::DecompressionError {
                        index: chunk_index,
                        error: FormatError::Lz4(e),
                    }
                })
            }
        }
    }
}

/// A compressor that is capable of compressing chunks of a chunked-compression archive.
pub enum Compressor {
    Zstd(zstd::zstd_safe::CCtx<'static>),
    Lz4 { compression_level: lz4::HcCompressionLevel },
}

impl Compressor {
    /// Compresses a chunk of a chunked-compression archive.
    pub fn compress(
        &mut self,
        data: &[u8],
        chunk_index: usize,
    ) -> Result<Vec<u8>, ChunkedArchiveError> {
        match self {
            Self::Zstd(cctx) => {
                let buffer_len = zstd::zstd_safe::compress_bound(data.len());
                let mut buffer = Vec::with_capacity(buffer_len);
                match cctx.compress2(&mut buffer, data) {
                    Ok(_) => Ok(buffer),
                    Err(code) => Err(ChunkedArchiveError::CompressionError {
                        index: chunk_index,
                        error: FormatError::Zstd(ZstdError(code)),
                    }),
                }
            }
            Self::Lz4 { compression_level } => {
                lz4::compress_hc(data, *compression_level).map_err(|error| {
                    ChunkedArchiveError::CompressionError {
                        index: chunk_index,
                        error: FormatError::Lz4(error),
                    }
                })
            }
        }
    }
}

#[derive(Copy, Clone)]
/// A compressor that uses thread-local storage to avoid reallocation of large state objects.
pub enum ThreadLocalCompressor {
    Zstd { compression_level: i32 },
    Lz4 { compression_level: lz4::HcCompressionLevel },
}

impl ThreadLocalCompressor {
    /// Compresses a chunk of a chunked-compression archive.
    pub fn compress(
        &self,
        data: &[u8],
        chunk_index: usize,
    ) -> Result<Vec<u8>, ChunkedArchiveError> {
        match self {
            Self::Zstd { compression_level } => ZSTD_COMPRESSOR.with_borrow_mut(|cctx| {
                cctx.set_parameter(zstd::zstd_safe::CParameter::CompressionLevel(
                    *compression_level,
                ))
                .expect("setting the compression level should never fail");
                let buffer_len = zstd::zstd_safe::compress_bound(data.len());
                let mut buffer = Vec::with_capacity(buffer_len);
                match cctx.compress2(&mut buffer, data) {
                    Ok(_) => Ok(buffer),
                    Err(code) => Err(ChunkedArchiveError::CompressionError {
                        index: chunk_index,
                        error: FormatError::Zstd(ZstdError(code)),
                    }),
                }
            }),
            Self::Lz4 { compression_level } => {
                lz4::compress_hc(data, *compression_level).map_err(|error| {
                    ChunkedArchiveError::CompressionError {
                        index: chunk_index,
                        error: FormatError::Lz4(error),
                    }
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::ChunkedArchiveOptions;

    const TEST_DATA: &[u8] = b"hello world this is some test data to compress and decompress";

    #[test]
    fn test_zstd_roundtrip() {
        let options =
            ChunkedArchiveOptions::V3 { compression_algorithm: CompressionAlgorithm::Zstd };
        let mut compressor = options.compressor();
        let compressed = compressor.compress(TEST_DATA, 0).unwrap();

        let mut decompressor = CompressionAlgorithm::Zstd.decompressor();
        let decompressed = decompressor.decompress(&compressed, TEST_DATA.len(), 0).unwrap();

        assert_eq!(decompressed, TEST_DATA);
    }

    #[test]
    fn test_lz4_roundtrip() {
        let options =
            ChunkedArchiveOptions::V3 { compression_algorithm: CompressionAlgorithm::Lz4 };
        let mut compressor = options.compressor();
        let compressed = compressor.compress(TEST_DATA, 0).unwrap();

        let mut decompressor = CompressionAlgorithm::Lz4.decompressor();
        let decompressed = decompressor.decompress(&compressed, TEST_DATA.len(), 0).unwrap();

        assert_eq!(decompressed, TEST_DATA);
    }

    #[test]
    fn test_thread_local_zstd_roundtrip() {
        let options =
            ChunkedArchiveOptions::V3 { compression_algorithm: CompressionAlgorithm::Zstd };
        let compressor = options.thread_local_compressor();
        let compressed = compressor.compress(TEST_DATA, 0).unwrap();

        let decompressor = CompressionAlgorithm::Zstd.thread_local_decompressor();
        let decompressed = decompressor.decompress(&compressed, TEST_DATA.len(), 0).unwrap();

        assert_eq!(decompressed, TEST_DATA);
    }

    #[test]
    fn test_thread_local_lz4_roundtrip() {
        let options =
            ChunkedArchiveOptions::V3 { compression_algorithm: CompressionAlgorithm::Lz4 };
        let compressor = options.thread_local_compressor();
        let compressed = compressor.compress(TEST_DATA, 0).unwrap();

        let decompressor = CompressionAlgorithm::Lz4.thread_local_decompressor();
        let decompressed = decompressor.decompress(&compressed, TEST_DATA.len(), 0).unwrap();

        assert_eq!(decompressed, TEST_DATA);
    }

    #[test]
    fn test_decompress_into() {
        let options = ChunkedArchiveOptions::V2 {
            minimum_chunk_size: 0,
            chunk_alignment: 0,
            compression_level: 1,
        };
        let mut compressor = options.compressor();
        let compressed = compressor.compress(TEST_DATA, 0).unwrap();

        let mut decompressor = CompressionAlgorithm::Zstd.decompressor();
        let mut buffer = vec![0u8; TEST_DATA.len()];
        let len = decompressor.decompress_into(&compressed, &mut buffer, 0).unwrap();

        assert_eq!(len, TEST_DATA.len());
        assert_eq!(buffer, TEST_DATA);
    }

    #[test]
    fn test_algorithm_conversion() {
        assert_eq!(u8::from(CompressionAlgorithm::Zstd), 0);
        assert_eq!(u8::from(CompressionAlgorithm::Lz4), 1);

        assert_eq!(CompressionAlgorithm::try_from(0).unwrap(), CompressionAlgorithm::Zstd);
        assert_eq!(CompressionAlgorithm::try_from(1).unwrap(), CompressionAlgorithm::Lz4);
        assert!(CompressionAlgorithm::try_from(2).is_err());
    }

    #[test]
    fn test_decompress_into_ptr() {
        for algorithm in [CompressionAlgorithm::Zstd, CompressionAlgorithm::Lz4] {
            let options = ChunkedArchiveOptions::V3 { compression_algorithm: algorithm };
            let compressor = options.thread_local_compressor();
            let compressed = compressor.compress(TEST_DATA, 0).unwrap();

            let decompressor = algorithm.thread_local_decompressor();
            let mut buffer = vec![0u8; TEST_DATA.len()];
            let len = decompressor
                .decompress_into(PtrByteSlice::from(compressed.as_slice()), &mut buffer, 0)
                .unwrap();

            assert_eq!(len, TEST_DATA.len());
            assert_eq!(buffer, TEST_DATA);
        }
    }
}
