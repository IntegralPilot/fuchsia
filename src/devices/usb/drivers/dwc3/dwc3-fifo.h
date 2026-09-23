// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_FIFO_H_
#define SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_FIFO_H_

#include <lib/dma-buffer/buffer.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>

namespace dwc3 {

template <bool manage_lifetime, typename gtest_base>
class TestFixture;

static inline const uint32_t kBufferSize = zx_system_get_page_size();

template <typename T>
class Fifo {
  template <bool manage_lifetime, typename gtest_base>
  friend class TestFixture;

 public:
  virtual zx::result<> Init(zx::bti& bti, bool cached) {
    if (!buffer_) {
      zx_status_t status = dma_buffer::CreateBufferFactory()->CreateContiguous(
          bti, kBufferSize, 12,
          cached ? dma_buffer::CacheOptions::kEnabled : dma_buffer::CacheOptions::kDisabled,
          &buffer_);
      if (status != ZX_OK) {
        fdf::error("dma_buffer init fails: {}", zx_status_get_string(status));
        return zx::error(status);
      }

      first_ = static_cast<T*>(buffer_->virt());
      last_ = first_ + (kBufferSize / sizeof(T));
    }

    write_ = first_;
    read_ = write_;
    return zx::ok();
  }
  void Clear() { read_ = write_; }
  void Release() {
    first_ = write_ = read_ = last_ = nullptr;
    buffer_.reset();
  }

  size_t TotalSlots() const { return first_ ? (last_ - first_) : 0; }
  size_t WriteOffset() const { return first_ ? (write_ - first_) : 0; }
  size_t ReadOffset() const { return first_ ? (read_ - first_) : 0; }
  bool IsEmpty() const { return read_ == write_; }

  size_t AvailableSlots() const {
    if (!first_ || !last_) {
      return 0;
    }
    size_t total_slots = last_ - first_;
    if (write_ >= read_) {
      return total_slots - (write_ - read_) - 1;
    }
    return (read_ - write_) - 1;
  }

  size_t GetActiveCount() const {
    if (!first_ || !last_) {
      return 0;
    }
    return TotalSlots() - AvailableSlots() - 1;
  }

  std::vector<T> Read(size_t count) {
    T* ptr = read_;
    return Read(ptr, count);
  }

  std::vector<T> Read(T*& ptr, size_t count) {
    ZX_ASSERT((ptr >= first_) && (ptr <= last_));
    const zx_off_t offset = (ptr - first_) * sizeof(T);
    const size_t todo = std::min<size_t>(last_ - ptr, count);
    std::vector<T> values(count);
    if (auto status = buffer_->Read(offset, todo * sizeof(T), values.data()); status.is_error()) {
      fdf::error("Read failed: {}", status);
      return {};
    }
    if (count > todo) {
      if (auto status = buffer_->Read(0, (count - todo) * sizeof(T), values.data() + todo);
          status.is_error()) {
        fdf::error("Read failed: {}", status);
        return {};
      }
    }

    return values;
  }

  zx_paddr_t Write(T*& ptr, size_t count = 1) {
    ZX_ASSERT((ptr >= first_) && (ptr <= last_));
    const zx_off_t offset = (ptr - first_) * sizeof(T);
    const size_t todo = std::min<size_t>(last_ - ptr, count);
    if (auto status = buffer_->CacheFlush(offset, todo * sizeof(T)); status.is_error()) {
      fdf::error("CacheFlush failed: {}", status);
      return 0;
    }
    if (count > todo) {
      if (auto status = buffer_->CacheFlush(0, (count - todo) * sizeof(T)); status.is_error()) {
        fdf::error("CacheFlush failed: {}", status);
        return 0;
      }
    }
    return GetPhys(ptr);
  }

  T* Advance(T*& ptr, size_t count = 1) {
    T* cur = ptr;
    ptr += count;
    if (ptr >= last_) {
      ptr = ptr - last_ + first_;
    }
    return cur;
  }

  const T* Advance(const T*& ptr, size_t count = 1) const {
    const T* cur = ptr;
    ptr += count;
    if (ptr >= last_) {
      ptr = ptr - last_ + first_;
    }
    return cur;
  }

 protected:
  zx_paddr_t GetPhys(T* ptr) const {
    ZX_ASSERT((ptr >= first_) && (ptr <= last_));
    return buffer_->phys() + ((ptr - first_) * sizeof(T));
  }

  std::unique_ptr<dma_buffer::ContiguousBuffer> buffer_;

  T* first_{nullptr};  // first slot in the fifo
  T* write_{nullptr};  // next free write slot in the fifo
  T* read_{nullptr};   // next read slot in the fifo
  T* last_{nullptr};   // last slot in the fifo
};

}  // namespace dwc3

#endif  // SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_FIFO_H_
