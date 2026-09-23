// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_STORAGE_STORAGE_H_
#define SRC_FIRMWARE_LIB_STORAGE_STORAGE_H_

#ifdef FIRMWARE_STORAGE_CUSTOM_SYSDEPS_HEADER
#include <firmware_storage_sysdeps.h>
#else
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

// The macro specifies the buffer alignment requirement for reading/writing device storage. One
// example is DMA alignment. Users should define this according to the actual device. The default
// value is 64.
#ifndef FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT
#define FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT 64
#endif

typedef struct FuchsiaFirmwareStorage {
  // The minimal size for read/write. On EMMC this should be the block size. On NAND this should be
  // the erase block size.
  size_t block_size;

  // The total size of the storage in number of blocks.
  size_t total_blocks;

  // A scratch buffer for unaligned read/write. It must have a capacity at least `block_size` and
  // must be buffer aligned according to FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT.
  void* scratch_buffer;

  // The size of the scratch buffer in bytes.
  size_t scratch_buffer_size_bytes;

  // A buffer used as an optimization for fill writes to storage.
  // Its size must be a multiple of both `block_size` and FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT
  // and must be buffer aligned according to FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT
  void* fill_buffer;

  // The size of the fill buffer in bytes.
  // A buffer much larger than `block_size` can provide an optimization for writing fill data.
  // Empirical testing shows good results with a buffer size of ~4MiB.
  size_t fill_buffer_size_bytes;

  // Context pointer for calling the `read` callback.
  void* ctx;

  // Callback for reading from the storage.
  //
  // Implementation can assume that`dst` is buffer aligned according to
  // FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT. However `block_offset` and `blocks_count` must be
  // treated as logical. It's up to the implementation to convert them into physical offset and size
  // for accessing storage. For example, in the case of NAND device, implementation must take into
  // consideration bad block and may want to compute the corresponding physical offset and size
  // using skip block logic.
  //
  // @ctx: Context pointer. It will always be the `ctx` field above.
  // @block_offset: Logical offset from the storage to read in terms of number of blocks.
  // @blocks_count: Logical size to read in terms of number of blocks.
  // @dst: Output pointer to read to.
  //
  // Return true if success, false otherwise.
  bool (*read)(void* ctx, size_t block_offset, size_t blocks_count, void* dst);

  // Callback for writing to the storage.
  //
  // Similar to read(), implementation can assume that `src` is buffer aligned according to
  // FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT. However `block_offset` and `blocks_count` must be
  // treated as logical.
  //
  // @ctx: Context pointer. It will always be the `ctx` field above.
  // @block_offset: Logical offset from the storage to write in terms of number of blocks.
  // @blocks_count: Logical size to write in terms of number of blocks.
  // @src: Pointer to buffer of data to write.
  //
  // Note: Some storage such as NAND requires data to be erased first before it can write new data.
  // This should be handled in the implementation of this callback.
  //
  // Return true if success, false otherwise.
  bool (*write)(void* ctx, size_t block_offset, size_t blocks_count, const void* src);
} FuchsiaFirmwareStorage;

// Read data from storage
//
// @ops: Pointer to FuchsiaFirmwareStorage
// @offset: offset in number of bytes to read from.
// @size: size in number of bytes to read.
// @dst: Pointer to buffer to read to.
//
// Return true if success, false otherwise.
bool FuchsiaFirmwareStorageRead(FuchsiaFirmwareStorage* ops, size_t offset, size_t size, void* dst);

// Write data to storage
//
// @ops: Pointer to FuchsiaFirmwareStorage
// @offset: offset in number of bytes to write.
// @size: size in number of bytes to write.
// @src: Pointer to buffer of data to write.
//
// Note: `src` needs to be non-const since it might need to be temporarily modified internally
// to optimize performance.
//
// Return true if success, false otherwise.
bool FuchsiaFirmwareStorageWrite(FuchsiaFirmwareStorage* ops, size_t offset, size_t size,
                                 void* src);

// Functionally the same as FuchsiaFirmwareStorageWrite but accepts a const pointer for `src`.
// However there's less optimization that can be done and may be slow when writing large amount
// of data when offset, size or `src` is unaligned.
bool FuchsiaFirmwareStorageWriteConst(FuchsiaFirmwareStorage* ops, size_t offset, size_t size,
                                      const void* src);

#ifdef ENABLE_FIRMWARE_STORAGE_LOG
#define firmware_storage_log printf
#else
#define firmware_storage_log(...)
#endif

#ifdef __cplusplus
}
#endif

#endif  // SRC_FIRMWARE_LIB_STORAGE_STORAGE_H_
