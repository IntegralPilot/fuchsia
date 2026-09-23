// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_SPARSE_C_SPARSE_H_
#define SRC_STORAGE_LIB_SPARSE_C_SPARSE_H_

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// An opaque handle to a buffer which can be used to write to a SparseIoInterface.
// This might just point to an in-memory buffer, or some special structure (e.g. a VMO registered
// with a block device for use with the block FIDL protocol).
typedef void* SparseIoBufferHandle;

// Operations that can be performed on a `SparseIoBufferHandle`.
typedef struct SparseIoBufferOps {
  // Returns the size, in bytes, of the handle.
  size_t (*size)(SparseIoBufferHandle handle);

  // Reads `size` bytes from the handle at `offset` into `dst`.
  // Returns false if the read failed.
  bool (*read)(SparseIoBufferHandle handle, uint64_t offset, uint8_t* dst, size_t size);

  // Fills the handle with repeated instances of `payload`.
  // Returns false if the write failed.
  bool (*fill)(SparseIoBufferHandle handle, uint32_t payload);
} SparseIoBufferOps;

typedef struct SparseIoInterface {
  void* ctx;

  // A buffer used to optimize `fill` writing.
  //
  // Warning: the contents of the fill handle may be reused by sparse writing code across
  // function calls. Once the fill handle has been set up, it is NOT safe for the caller
  // to modify it until destruction.
  SparseIoBufferHandle fill_handle;

  SparseIoBufferOps handle_ops;

  // Writes `size` bytes from `src` at `src_offset` to the device at `dev_offset`.
  // Returns false if the write failed.
  // The implementor is not responsible for bounds-checking `src`.
  bool (*write)(void* ctx, uint64_t dev_offset, SparseIoBufferHandle src, uint64_t src_offset,
                size_t size);
} SparseIoInterface;

typedef int (*SparseLogFn)(const char*, ...);

int sparse_nop_logger(const char*, ...);

// Returns true if the passed in buffer contains a well formed sparse image header.
//
// @handle_ops: Ops table.
// @src: Buffer containing sparse image.
//
bool sparse_is_sparse_image(SparseIoBufferOps* handle_ops, SparseIoBufferHandle src);

// Unpacks a sparse image and writes it to the passed I/O sink.
//
// @io: I/O sink.
// @log: An optional log sink for error messages.
// @src: Handle containing the sparse image.
//
// Returns true on success, false otherwise.
//
// Failure can occur if any of the following occur:
// *) Invalid sparse image header
// *) Inconsistent sparse image data, e.g. total_blks disagrees with number of chunks and
//    corresponding chunk_sz
// *) Newer sparse image format than expected
// *) A larger expanded image than the destination partition can support
// *) Checksum mismatch
// *) Write failure to the underlying storage
//
// This function is not thread-safe.
bool sparse_unpack_image(SparseIoInterface* io, SparseLogFn log, SparseIoBufferHandle src);

#ifdef __cplusplus
}  // __cplusplus
#endif

#endif  // SRC_STORAGE_LIB_SPARSE_C_SPARSE_H_
