// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BUFFER_VMO_BITMAP_STORAGE_H_
#define SRC_STORAGE_LIB_BUFFER_VMO_BITMAP_STORAGE_H_

#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <cstddef>

#include "src/storage/lib/buffer/mapped_vmo.h"

namespace storage {

// VmoBitmapStorage implements the storage interface required by bitmap::RawBitmapGeneric backed by
// a MappedVmo.
//
// It allows the bitmap to grow dynamically. Since it is backed by MappedVmo, growing the storage
// only resizes the virtual memory mapping, not the underlying VMO itself, which is created as
// unbounded.
class VmoBitmapStorage {
 public:
  VmoBitmapStorage() = default;

  VmoBitmapStorage(VmoBitmapStorage&& other) noexcept = default;
  VmoBitmapStorage& operator=(VmoBitmapStorage&& other) noexcept = default;
  VmoBitmapStorage(const VmoBitmapStorage&) = delete;
  VmoBitmapStorage& operator=(const VmoBitmapStorage&) = delete;

  // Storage interface required by bitmap::RawBitmapGeneric:
  zx_status_t Allocate(size_t size) {
    return mapped_vmo_.CreateAndMap(size, "vmo-bitmap").status_value();
  }
  zx_status_t Grow(size_t size) { return mapped_vmo_.Grow(size).status_value(); }
  void* GetData() { return mapped_vmo_.start(); }
  const void* GetData() const { return mapped_vmo_.start(); }

  const zx::vmo& vmo() const { return mapped_vmo_.vmo(); }

 private:
  MappedVmo mapped_vmo_;
};

}  // namespace storage
   //
#endif  // SRC_STORAGE_LIB_BUFFER_VMO_BITMAP_STORAGE_H_
