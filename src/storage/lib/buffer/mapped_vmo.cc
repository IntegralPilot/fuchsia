// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/buffer/mapped_vmo.h"

#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <string.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <cstdint>
#include <limits>
#include <string_view>
#include <utility>

#include <fbl/algorithm.h>

namespace storage {

namespace {

zx::result<size_t> RoundUpToPageSize(size_t size) {
  const size_t page_size = zx_system_get_page_size();
  if (size > std::numeric_limits<size_t>::max() - (page_size - 1)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(fbl::round_up(size, page_size));
}

}  // namespace

MappedVmo::MappedVmo(MappedVmo&& other) noexcept {
  start_ = other.start_;
  other.start_ = 0;
  size_ = other.size_;
  other.size_ = 0;
  vmo_ = std::move(other.vmo_);
}

MappedVmo& MappedVmo::operator=(MappedVmo&& other) noexcept {
  if (this != &other) {
    Reset();
    start_ = other.start_;
    other.start_ = 0;
    size_ = other.size_;
    other.size_ = 0;
    vmo_ = std::move(other.vmo_);
  }
  return *this;
}

zx::result<> MappedVmo::CreateAndMap(uint64_t size, std::string_view name) {
  if (size == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (vmo_.is_valid()) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  zx::result<size_t> rounded_size = RoundUpToPageSize(size);
  if (rounded_size.is_error()) {
    return rounded_size.take_error();
  }
  size = rounded_size.value();

  zx::vmo vmo;
  if (zx_status_t status = zx::vmo::create(0, ZX_VMO_UNBOUNDED, &vmo); status != ZX_OK) {
    return zx::error(status);
  }
  if (!name.empty()) {
    if (zx_status_t status = vmo.set_property(ZX_PROP_NAME, name.data(), name.size());
        status != ZX_OK) {
      return zx::error(status);
    }
  }

  if (zx_status_t status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                                      size, &start_)) {
    return zx::error(status);
  }
  vmo_ = std::move(vmo);
  size_ = size;
  return zx::ok();
}

zx::result<> MappedVmo::Grow(size_t size) {
  if (!vmo_.is_valid()) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  zx::result<size_t> rounded_size = RoundUpToPageSize(size);
  if (rounded_size.is_error()) {
    return rounded_size.take_error();
  }
  size = rounded_size.value();
  if (size < size_) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (size == size_) {
    return zx::ok();
  }

  zx::unowned_vmar vmar = zx::vmar::root_self();
  uintptr_t new_start;
  if (zx_status_t status =
          vmar->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo_, 0, size, &new_start)) {
    return zx::error(status);
  }

  if (zx_status_t status = vmar->unmap(start_, size_); status != ZX_OK) {
    // Rollback the new mapping if we failed to unmap the old one.
    [[maybe_unused]] zx_status_t rollback_status = vmar->unmap(new_start, size);
    ZX_DEBUG_ASSERT(rollback_status == ZX_OK);
    return zx::error(status);
  }

  start_ = new_start;
  size_ = size;
  return zx::ok();
}

zx::result<> MappedVmo::Shrink(size_t size) {
  if (!vmo_.is_valid()) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  if (size == 0 || size > size_) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  size = fbl::round_up(size, zx_system_get_page_size());
  if (size == size_) {
    return zx::ok();
  }

  const size_t old_size = size_;
  if (zx_status_t status = zx::vmar::root_self()->unmap(start_ + size, old_size - size);
      status != ZX_OK) {
    return zx::error(status);
  }
  size_ = size;
  if (zx_status_t status = vmo_.op_range(ZX_VMO_OP_DECOMMIT, size, old_size - size, nullptr, 0);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok();
}

void MappedVmo::Reset() {
  if (!vmo_.is_valid()) {
    return;
  }
  vmo_.reset();
  [[maybe_unused]] zx_status_t status = zx::vmar::root_self()->unmap(start_, size_);
  ZX_DEBUG_ASSERT(status == ZX_OK);
  start_ = 0;
  size_ = 0;
}

}  // namespace storage
