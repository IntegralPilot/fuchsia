// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BUFFER_VMOID_REGISTRY_H_
#define SRC_STORAGE_LIB_BUFFER_VMOID_REGISTRY_H_

#include <fidl/fuchsia.storage.block/cpp/fidl.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/types.h>

namespace storage {

using vmoid_t = uint16_t;

// A thin wrapper around a vmoid_t that will assert if you forget to detach it.
class Vmoid {
 public:
  Vmoid() = default;
  explicit Vmoid(vmoid_t vmoid) : vmoid_(vmoid) {}

  // Movable but not copyable.
  Vmoid(const Vmoid& other) = delete;
  Vmoid& operator=(const Vmoid& other) = delete;

  Vmoid(Vmoid&& other) noexcept {
    vmoid_ = other.vmoid_;
    other.vmoid_ = fuchsia_storage_block::kVmoidInvalid;
  }
  Vmoid& operator=(Vmoid&& other) noexcept {
    ZX_DEBUG_ASSERT(vmoid_ == fuchsia_storage_block::kVmoidInvalid);
    vmoid_ = other.vmoid_;
    other.vmoid_ = 0;
    return *this;
  }

  ~Vmoid() { ZX_DEBUG_ASSERT_MSG(vmoid_ == fuchsia_storage_block::kVmoidInvalid, " (%u)", vmoid_); }

  vmoid_t get() const { return vmoid_; }
  bool IsAttached() const { return vmoid_ != fuchsia_storage_block::kVmoidInvalid; }
  [[nodiscard]] vmoid_t TakeId() {
    vmoid_t id = vmoid_;
    vmoid_ = fuchsia_storage_block::kVmoidInvalid;
    return id;
  }

 private:
  vmoid_t vmoid_ = fuchsia_storage_block::kVmoidInvalid;
};

// An interface which controls attaching and detaching VMOs with the underlying device.
class VmoidRegistry {
 public:
  virtual ~VmoidRegistry() = default;

  // Allocates a vmoid registering a VMO with the underlying block device.
  virtual zx_status_t BlockAttachVmo(const zx::vmo& vmo, Vmoid* out) = 0;

  // Releases an allocated vmoid.
  virtual zx_status_t BlockDetachVmo(Vmoid vmoid) = 0;
};

}  // namespace storage

#endif  // SRC_STORAGE_LIB_BUFFER_VMOID_REGISTRY_H_
