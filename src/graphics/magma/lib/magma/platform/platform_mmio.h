// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PLATFORM_MMIO_H
#define PLATFORM_MMIO_H

#include <lib/magma/util/dlog.h>
#include <lib/magma/util/macros.h>
#include <lib/mmio-ptr/mmio-ptr.h>
#include <stdint.h>

#include <memory>

namespace magma {

// Created from a PlatformPciDevice.
class PlatformMmio {
 public:
  PlatformMmio(MMIO_PTR void* addr, uint64_t size) : addr_(addr), size_(size) {}

  virtual ~PlatformMmio() {}

  // Gets the physical address of the MMIO. Not implemented for MMIOs from PCI devices.
  virtual uint64_t physical_address() = 0;

  void Write32(uint32_t val, uint64_t offset) {
    MAGMA_DASSERT(offset < size());
    MAGMA_DASSERT((offset & 0x3) == 0);
    MmioWrite32(val, reinterpret_cast<MMIO_PTR volatile uint32_t*>(addr(offset)));
  }

  uint32_t Read32(uint64_t offset) {
    MAGMA_DASSERT(offset < size());
    MAGMA_DASSERT((offset & 0x3) == 0);
    return MmioRead32(reinterpret_cast<MMIO_PTR volatile uint32_t*>(addr(offset)));
  }

  void Write64(uint64_t val, uint64_t offset) {
    MAGMA_DASSERT(offset < size());
    MAGMA_DASSERT((offset & 0x7) == 0);
    MmioWrite64(val, reinterpret_cast<MMIO_PTR volatile uint64_t*>(addr(offset)));
  }

  uint64_t Read64(uint64_t offset) {
    MAGMA_DASSERT(offset < size());
    MAGMA_DASSERT((offset & 0x7) == 0);
    return MmioRead64(reinterpret_cast<MMIO_PTR volatile uint64_t*>(addr(offset)));
  }

  // Posting reads serve to ensure that a previous bus write at the same address has completed.
  uint32_t PostingRead32(uint64_t offset) { return Read32(offset); }
  uint64_t PostingRead64(uint64_t offset) { return Read64(offset); }

  MMIO_PTR void* addr() { return addr_; }
  uint64_t size() { return size_; }

 private:
  MMIO_PTR volatile void* addr(uint64_t offset) {
    MAGMA_DASSERT(offset < size_);
    return reinterpret_cast<MMIO_PTR volatile uint8_t*>(addr_) + offset;
  }

  MMIO_PTR void* addr_;
  uint64_t size_;

  PlatformMmio(const PlatformMmio&) = delete;
  void operator=(const PlatformMmio&) = delete;
};

}  // namespace magma

#endif  // PLATFORM_MMIO_H
