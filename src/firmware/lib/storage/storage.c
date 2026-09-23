// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/storage/storage.h>

#ifdef ENABLE_FIRMWARE_STORAGE_LOG
#define firmware_storage_log printf
#else
#define firmware_storage_log(...)
#endif

#define FIRMWARE_STORAGE_MIN(a, b) (((a) < (b)) ? (a) : (b))
#define FIRMWARE_STORAGE_MAX(a, b) (((a) > (b)) ? (a) : (b))
#define FIRMWARE_STORAGE_IS_ALIGNED(a, b) ((((uintptr_t)(a)) % (uintptr_t)(b)) == 0)

#define FIRMWARE_STORAGE_ROUNDUP(a, b) \
  ({                                   \
    const __typeof((a)) _a = (a);      \
    const __typeof((b)) _b = (b);      \
    ((_a + _b - 1) / _b * _b);         \
  })

#define FIRMWARE_STORAGE_ROUNDDOWN(a, b) \
  ({                                     \
    const __typeof((a)) _a = (a);        \
    const __typeof((b)) _b = (b);        \
    _a - (_a % _b);                      \
  })

static bool ReadWrapper(FuchsiaFirmwareStorage* ops, size_t offset, size_t size, void* dst) {
  size_t offset_blocks = offset / ops->block_size;
  size_t size_blocks = size / ops->block_size;
  if (offset_blocks + size_blocks > ops->total_blocks) {
    firmware_storage_log("Read overflow\n");
    return false;
  }
  return ops->read(ops->ctx, offset_blocks, size_blocks, dst);
}

static bool WriteWrapper(FuchsiaFirmwareStorage* ops, size_t offset, size_t size, const void* src) {
  size_t offset_blocks = offset / ops->block_size;
  size_t size_blocks = size / ops->block_size;
  if (offset_blocks + size_blocks > ops->total_blocks) {
    firmware_storage_log("Write overflow\n");
    return false;
  }
  return ops->write(ops->ctx, offset_blocks, size_blocks, src);
}

// First, relax the `size` alignment requirement, assume aligned offset and buffer:
// offset
//   |~~~~~~~~~size~~~~~~~~~|
//   |---------|---------|---------|
static bool ReadWithAlignedOffsetAndDma(FuchsiaFirmwareStorage* ops, size_t offset, size_t size,
                                        void* dst) {
  // Read the aligned part.
  size_t aligned_read_size = FIRMWARE_STORAGE_ROUNDDOWN(size, ops->block_size);
  if (aligned_read_size && !ReadWrapper(ops, offset, aligned_read_size, dst)) {
    firmware_storage_log("Failed to read aligned part\n");
    return false;
  }

  if (aligned_read_size == size) {
    return true;
  }

  // Read the remaining unaligned part into the scratch buffer
  if (!ReadWrapper(ops, offset + aligned_read_size, ops->block_size, ops->scratch_buffer)) {
    firmware_storage_log("Failed to read unaligned part\n");
    return false;
  }
  memcpy((uint8_t*)dst + aligned_read_size, ops->scratch_buffer, size - aligned_read_size);
  return true;
}

// Second, relax both the `offset` and `size` alignment requirement, assume aligned buffer.
static bool ReadWithAlignedDma(FuchsiaFirmwareStorage* ops, size_t offset, size_t size, void* dst) {
  if (FIRMWARE_STORAGE_IS_ALIGNED(offset, ops->block_size)) {
    return ReadWithAlignedOffsetAndDma(ops, offset, size, dst);
  }
  size_t aligned_offset = FIRMWARE_STORAGE_ROUNDDOWN(offset, ops->block_size);
  // Case 1:
  //            |~~unaligned_read~~|
  //            |~~~~~~~~~~~~~size~~~~~~~~~~~~|
  //         offset
  //        |----------------------|---------------------|
  //   aligned_offset
  //
  // Case 2:
  //            |~~unaligned_read~~|
  //            |~~~~~~~~size~~~~~~|
  //         offset
  //        |-------------------------|------------------------|
  //   aligned_offset
  size_t unaligned_read = FIRMWARE_STORAGE_MIN(size, ops->block_size - (offset - aligned_offset));
  size_t new_offset = offset + unaligned_read;
  size_t new_size = size - unaligned_read;

  // Read the aligned part first, if there is any, so that we can safely reuse the scratch buffer.
  if (new_size) {
    uint8_t* next = (uint8_t*)dst + unaligned_read;
    // If the new output address is still buffer aligned, read into it.
    if (FIRMWARE_STORAGE_IS_ALIGNED(next, FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT)) {
      if (!ReadWithAlignedOffsetAndDma(ops, new_offset, new_size, next)) {
        return false;
      }
    } else {
      // Otherwise, read into `dst` which is assumed buffer aligned and memmove to correct position.
      if (!ReadWithAlignedOffsetAndDma(ops, new_offset, new_size, dst)) {
        return false;
      }
      memmove(next, dst, new_size);
    }
  }

  // Read the unaligned part to scratch buffer.
  if (!ReadWrapper(ops, aligned_offset, ops->block_size, ops->scratch_buffer)) {
    firmware_storage_log("Failed to read unaligned part\n");
    return false;
  }
  memcpy(dst, ops->scratch_buffer + (offset - aligned_offset), unaligned_read);
  return true;
}

static uint8_t dma_aligned_scratch_buffer[FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT]
    __attribute__((__aligned__(FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT)));

// Finally, relax all of `offset`, `size` and buffer alignment requirement
bool FuchsiaFirmwareStorageRead(FuchsiaFirmwareStorage* ops, size_t offset, size_t size,
                                void* dst) {
  uintptr_t dst_addr = (uintptr_t)dst;
  if (FIRMWARE_STORAGE_IS_ALIGNED(dst_addr, FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT)) {
    return ReadWithAlignedDma(ops, offset, size, dst);
  }
  // Case 1:
  //     |~~~~~~~to_read~~~~~~|
  //     |~~~~~~~~~~~~~dst buffer~~~~~~~~~~~~|
  //   |----------------------|---------------------|
  //     buffer alignment size
  //
  // Case 2:
  //    |~~~~to_read~~~~~~|
  //    |~~~~dst buffer~~~|
  //  |----------------------|---------------------|
  //    buffer alignment size
  size_t to_read = FIRMWARE_STORAGE_MIN(
      size,
      FIRMWARE_STORAGE_ROUNDUP(dst_addr, FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT) - dst_addr);
  if (!ReadWithAlignedDma(ops, offset, to_read, dma_aligned_scratch_buffer)) {
    return false;
  }
  memcpy(dst, dma_aligned_scratch_buffer, to_read);
  return (to_read == size) ||
         ReadWithAlignedDma(ops, offset + to_read, size - to_read, (uint8_t*)dst + to_read);
}

// A write API that relaxes the `size` alignment requirement, assume aligned offset and DMA:
// offset
//   |~~~~~~~~~size~~~~~~~~~|
//   |---------|---------|---------|
static bool WriteWithAlignedOffsetAndDma(FuchsiaFirmwareStorage* ops, size_t offset, size_t size,
                                         const void* src) {
  // Write the aligned part.
  size_t aligned_write_size = FIRMWARE_STORAGE_ROUNDDOWN(size, ops->block_size);
  if (aligned_write_size && !WriteWrapper(ops, offset, aligned_write_size, src)) {
    firmware_storage_log("Failed to write aligned part\n");
    return false;
  }

  if (aligned_write_size == size) {
    return true;
  }

  // Perform read-copy-write for the unaligned part.
  // Read the block of the unaligned part into the scratch buffer
  if (!ReadWrapper(ops, offset + aligned_write_size, ops->block_size, ops->scratch_buffer)) {
    firmware_storage_log("Failed to read unaligned part\n");
    return false;
  }
  memcpy(ops->scratch_buffer, (const uint8_t*)src + aligned_write_size, size - aligned_write_size);
  if (!WriteWrapper(ops, offset + aligned_write_size, ops->block_size, ops->scratch_buffer)) {
    firmware_storage_log("Failed to write unaligned part\n");
    return false;
  }

  return true;
}

// There's not much optimization we can do other than simply copying chunk by chunk.
// If `src` can be modified, we can optimize the number of write calls similar to read, see
// implementation of `FuchsiaFirmwareStorageWrite()` below
bool FuchsiaFirmwareStorageWriteConst(FuchsiaFirmwareStorage* ops, size_t offset, size_t size,
                                      const void* src) {
  // Round down to block boundary and copy-write block by block.
  size_t curr_block_offset = FIRMWARE_STORAGE_ROUNDDOWN(offset, ops->block_size);
  const uint8_t* data = (const uint8_t*)src;
  while (size) {
    // If at any point `data` becomes buffer aligned and `offset` becomes block aligned, write
    // directly.
    if (FIRMWARE_STORAGE_IS_ALIGNED((uintptr_t)data, FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT) &&
        FIRMWARE_STORAGE_IS_ALIGNED(offset, ops->block_size)) {
      return WriteWithAlignedOffsetAndDma(ops, offset, size, data);
    }
    // Calculate the range (in absolute offset) for copying the data into the scratch buffer.
    // The larger between the current block start and `offset`.
    size_t copy_start = FIRMWARE_STORAGE_MAX(curr_block_offset, offset);
    // The smaller between current block end and `src` end.
    size_t copy_end = FIRMWARE_STORAGE_MIN(offset + size, curr_block_offset + ops->block_size);
    size_t to_copy = copy_end - copy_start;
    if (to_copy < ops->block_size) {
      // Partial copy. Some data on the storage needs to be kept.
      if (!ReadWrapper(ops, curr_block_offset, ops->block_size, ops->scratch_buffer)) {
        firmware_storage_log("Failed to read block\n");
        return false;
      }
    }
    memcpy(ops->scratch_buffer + copy_start - curr_block_offset, data, to_copy);
    if (!WriteWrapper(ops, curr_block_offset, ops->block_size, ops->scratch_buffer)) {
      firmware_storage_log("Failed to write block\n");
      return false;
    }
    curr_block_offset += ops->block_size;  // Move to the next block.
    size -= to_copy;
    offset += to_copy;
    data += to_copy;
  }

  return true;
}

// Circular rotation of memory data to the left.
static void RotateMemoryLeft(uint8_t* mem, size_t size, size_t rotate);

// A write API that relaxes the `size` and `offset` alignment requirement and only assumes aligned
// buffer. The `src` buffer needs to be non-const because it needs to be temporarily modified
// internally.
static bool WriteWithAlignedDma(FuchsiaFirmwareStorage* ops, size_t offset, size_t size,
                                void* src) {
  if (FIRMWARE_STORAGE_IS_ALIGNED(offset, ops->block_size)) {
    return WriteWithAlignedOffsetAndDma(ops, offset, size, src);
  }

  size_t aligned_offset = FIRMWARE_STORAGE_ROUNDDOWN(offset, ops->block_size);
  // Case 1:
  //            |~~unaligned_write~|
  //            |~~~~~~~~~~~~~size~~~~~~~~~~~~|
  //         offset
  //        |----------------------|---------------------|
  //   aligned_offset
  //
  // Case 2:
  //            |~~unaligned_write~|
  //            |~~~~~~~~size~~~~~~|
  //         offset
  //        |-------------------------|------------------------|
  //   aligned_offset
  size_t unaligned_write = FIRMWARE_STORAGE_MIN(size, ops->block_size - (offset - aligned_offset));
  size_t new_offset = offset + unaligned_write;
  size_t new_size = size - unaligned_write;

  // Write the aligned part first so that scratch buffer can be re-used
  if (new_size) {
    uint8_t* next = (uint8_t*)src + unaligned_write;
    // If the new data address is still buffer aligned, write it directly.
    if (FIRMWARE_STORAGE_IS_ALIGNED(next, FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT)) {
      if (!WriteWithAlignedOffsetAndDma(ops, new_offset, new_size, next)) {
        return false;
      }
    } else {
      // Otherwise, rotate `src` left by `unaligned_write`, so that the data to write starts
      // at `src` which is assumed buffer aligned.
      RotateMemoryLeft(src, size, unaligned_write);
      bool res = WriteWithAlignedOffsetAndDma(ops, new_offset, new_size, src);
      RotateMemoryLeft(src, size, new_size);  // Rotate back to restore the data.
      if (!res) {
        return false;
      }
    }
  }

  // Read-copy-write the unaligned part
  if (!ReadWrapper(ops, aligned_offset, ops->block_size, ops->scratch_buffer)) {
    firmware_storage_log("Failed to read unaligned part\n");
    return false;
  }
  memcpy(ops->scratch_buffer + (offset - aligned_offset), src, unaligned_write);
  if (!WriteWrapper(ops, aligned_offset, ops->block_size, ops->scratch_buffer)) {
    firmware_storage_log("Failed to write unaligned part\n");
    return false;
  }

  return true;
}

// A Write API that relaxes all of `offset`, `size` and buffer alignment requirement
bool FuchsiaFirmwareStorageWrite(FuchsiaFirmwareStorage* ops, size_t offset, size_t size,
                                 void* src) {
  uintptr_t src_addr = (uintptr_t)src;
  if (FIRMWARE_STORAGE_IS_ALIGNED(src_addr, FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT)) {
    return WriteWithAlignedDma(ops, offset, size, src);
  }
  // Case 1:
  //     |~~~~~~~to_write~~~~~|
  //     |~~~~~~~~~~~~~src buffer~~~~~~~~~~~~|
  //   |----------------------|---------------------|
  //      buffer alignment size
  //
  // Case 2:
  //    |~~~~to_write~~~~~|
  //    |~~~~src buffer~~~|
  //  |----------------------|---------------------|
  //     buffer alignment size
  size_t to_write = FIRMWARE_STORAGE_MIN(
      size,
      FIRMWARE_STORAGE_ROUNDUP(src_addr, FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT) - src_addr);
  memcpy(dma_aligned_scratch_buffer, src, to_write);
  if (!WriteWithAlignedDma(ops, offset, to_write, dma_aligned_scratch_buffer)) {
    return false;
  }
  return (to_write == size) ||
         WriteWithAlignedDma(ops, offset + to_write, size - to_write, (uint8_t*)src + to_write);
}

// Reverse a memory specified by [start, end)
static void ReverseMemory(uint8_t* start, uint8_t* end) {
  while (start < end) {
    uint8_t temp = *start;
    *(start++) = *(--end);
    *end = temp;
  }
}

static void RotateMemoryLeft(uint8_t* mem, size_t size, size_t rotate) {
  ReverseMemory(mem, mem + rotate);         // Reverse the left part.
  ReverseMemory(mem + rotate, mem + size);  // Reverse the right part.
  ReverseMemory(mem, mem + size);           // Reverse the entire array.
}
