// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BUFFER_MAPPED_VMO_H_
#define SRC_STORAGE_LIB_BUFFER_MAPPED_VMO_H_

#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <cstddef>
#include <cstdint>
#include <string_view>

namespace storage {

// MappedVmo is a helper class that manages a VMO mapped into the process's address space.
//
// Unlike fzl::ResizeableVmoMapper, MappedVmo creates the underlying VMO with the ZX_VMO_UNBOUNDED
// flag. This allows the mapping to be resized (grown or shrunk) without needing to resize the VMO
// itself.
//
// When |Grow| or |Shrink| are called, only the VMAR mapping is modified. Shrinking the mapping also
// decommits the unused pages in the VMO to free memory.
class MappedVmo {
 public:
  MappedVmo() = default;
  ~MappedVmo() { Reset(); }

  MappedVmo(const MappedVmo&) = delete;
  MappedVmo& operator=(const MappedVmo&) = delete;

  MappedVmo(MappedVmo&& other) noexcept;
  MappedVmo& operator=(MappedVmo&& other) noexcept;

  // Creates an unbounded VMO and maps it with the given initial size.
  //
  // |size| must be greater than 0. It is rounded up to the nearest page size. |name| is an optional
  // name for the VMO.
  //
  // Returns ZX_ERR_INVALID_ARGS if size is 0.
  // Returns ZX_ERR_BAD_STATE if the MappedVmo is already initialized.
  zx::result<> CreateAndMap(uint64_t size, std::string_view name);

  // Grows the mapping to the new |size|.
  //
  // Since the underlying VMO is unbounded, this only resizes the mapping, not the VMO. The virtual
  // address of the mapping (|start()|) may change. Existing data is preserved.
  //
  // |size| must be greater than or equal to the current size. It is rounded up to the nearest page
  // size.
  //
  // Returns ZX_ERR_BAD_STATE if the MappedVmo is not initialized.
  // Returns ZX_ERR_INVALID_ARGS if |size| is less than the current size.
  zx::result<> Grow(size_t size);

  // Shrinks the mapping to the new |size|.
  //
  // The excess mapping is unmapped and the corresponding pages in the VMO are decommitted. The
  // virtual address of the mapping (|start()|) does not change. Existing data in the remaining
  // portion is preserved.
  //
  // |size| must be less than or equal to the current size, and greater than 0. It is rounded up to
  // the nearest page size.
  //
  // Returns ZX_ERR_BAD_STATE if the MappedVmo is not initialized.
  // Returns ZX_ERR_INVALID_ARGS if |size| is greater than the current size, or if |size| is 0.
  zx::result<> Shrink(size_t size);

  size_t size() const { return size_; }
  void* start() { return reinterpret_cast<void*>(start_); }
  const void* start() const { return reinterpret_cast<const void*>(start_); }
  const zx::vmo& vmo() const { return vmo_; }

 private:
  void Reset();

  zx::vmo vmo_;
  uintptr_t start_ = 0;
  uint64_t size_ = 0;
};

}  // namespace storage

#endif  // SRC_STORAGE_LIB_BUFFER_MAPPED_VMO_H_
