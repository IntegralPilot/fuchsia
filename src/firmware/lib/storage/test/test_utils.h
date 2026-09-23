// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_STORAGE_TEST_TEST_UTILS_H_
#define SRC_FIRMWARE_LIB_STORAGE_TEST_TEST_UTILS_H_

#include <lib/storage/storage.h>

#include <memory>
#include <vector>

// A helper class that creates a buffer meeting `FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT`.
class AlignedBuffer {
 public:
  explicit AlignedBuffer(size_t size)
      : buffer_(size + FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT - 1) {}

  uint8_t* get() {
    uintptr_t addr = reinterpret_cast<uintptr_t>(buffer_.data());
    uintptr_t gap = (addr + FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT - 1) /
                        FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT *
                        FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT -
                    addr;
    return buffer_.data() + gap;
  }

  size_t size() const { return buffer_.size() - (FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT - 1); }

 private:
  std::vector<uint8_t> buffer_;
};

// A helper class to constructs a FuchsiaFirmwareStorage backed in-memory buffer.
// Usage:
//
//   TestFuchsiaFirmwareStorage test_storage(<storage_size>, <block_size>);
//   FuchsiaFIrmwareStorage storage = test_storage.GetFuchsiaFirmwareStorage();
//   ...
class TestFuchsiaFirmwareStorage {
 public:
  TestFuchsiaFirmwareStorage(size_t size, size_t block_size)
      : scratch_buffer_(block_size), fill_buffer_(2 * block_size), block_size_(block_size) {
    // Initialize buffer data;
    for (size_t i = 0; i < size; i++) {
      buffer_.push_back(static_cast<uint8_t>(i));
    }
  }

  FuchsiaFirmwareStorage GetFuchsiaFirmwareStorage() {
    return FuchsiaFirmwareStorage{
        .block_size = block_size_,
        .total_blocks = buffer_.size() / block_size_,
        .scratch_buffer = scratch_buffer_.get(),
        .scratch_buffer_size_bytes = scratch_buffer_.size(),
        .fill_buffer = fill_buffer_.get(),
        .fill_buffer_size_bytes = fill_buffer_.size(),
        .ctx = this,
        .read = Read,
        .write = Write,
    };
  }

  std::vector<uint8_t>& buffer() { return buffer_; }

 private:
  static bool Read(void* ctx, size_t block_offset, size_t blocks_count, void* dst) {
    TestFuchsiaFirmwareStorage* ptr = static_cast<TestFuchsiaFirmwareStorage*>(ctx);
    size_t offset = block_offset * ptr->block_size_, size = blocks_count * ptr->block_size_;
    if ((reinterpret_cast<uintptr_t>(dst) % FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT) ||
        offset + size > ptr->buffer_.size()) {
      return false;
    }
    memcpy(dst, ptr->buffer_.data() + offset, size);
    return true;
  }

  static bool Write(void* ctx, size_t block_offset, size_t blocks_count, const void* src) {
    TestFuchsiaFirmwareStorage* ptr = static_cast<TestFuchsiaFirmwareStorage*>(ctx);
    size_t offset = block_offset * ptr->block_size_, size = blocks_count * ptr->block_size_;
    if ((reinterpret_cast<uintptr_t>(src) % FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT) ||
        offset + size > ptr->buffer_.size()) {
      return false;
    }
    memcpy(ptr->buffer_.data() + offset, src, size);
    return true;
  }

  AlignedBuffer scratch_buffer_;
  AlignedBuffer fill_buffer_;
  std::vector<uint8_t> buffer_;
  size_t block_size_;
};

#endif  // SRC_FIRMWARE_LIB_STORAGE_TEST_TEST_UTILS_H_
