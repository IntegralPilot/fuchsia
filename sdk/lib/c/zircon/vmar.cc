// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vmar.h"

#include <lib/fit/defer.h>
#include <zircon/assert.h>
#include <zircon/status.h>

namespace LIBC_NAMESPACE_DECL {

constexpr zx_vm_option_t kVmarOptions =
    ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC;
constexpr zx_vm_option_t kMapOptions =  //
    ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC;

void GuardedPageBlock::Unmap() {
  ZX_DEBUG_ASSERT(*vmar_);
  [[maybe_unused]] zx_status_t status = vmar_->unmap(start_, size_.get());
  ZX_DEBUG_ASSERT(status == ZX_OK);
  start_ = 0;
  size_ = {};
}

template <>
zx::result<std::span<std::byte>> GuardedPageBlock::Allocate<std::byte>(  //
    zx::unowned_vmar allocate_from, AllocationVmo& vmo, PageRoundedSize data_size,
    PageRoundedSize guard_below, PageRoundedSize guard_above) {
  std::optional<PageRoundedSize> vmar_size_opt = guard_below + data_size + guard_above;
  if (!vmar_size_opt) {
    return zx::error{ZX_ERR_NO_RESOURCES};
  }
  PageRoundedSize vmar_size = *vmar_size_opt;

  zx::vmar vmar;
  if (zx::result result = zx::make_result(
          allocate_from->allocate(kVmarOptions, 0, vmar_size.get(), &vmar, &start_));
      result.is_error()) {
    // zx_vmar_allocate will return ZX_ERR_INVALID_ARGS if the requested size
    // is larger than the parent VMAR size. Translate this to ZX_ERR_NO_RESOURCES
    // since this represents resource exhaustion.
    if (result.error_value() == ZX_ERR_INVALID_ARGS) {
      return zx::error{ZX_ERR_NO_RESOURCES};
    }
    return result.take_error();
  }

  auto destroy_on_error = fit::defer([&vmar] {
    [[maybe_unused]] zx_status_t status = vmar.destroy();
    ZX_DEBUG_ASSERT_MSG(status == ZX_OK, "zx_vmar_destroy: %s", zx_status_get_string(status));
  });

  ZX_DEBUG_ASSERT(vmo.vmo);
  ZX_DEBUG_ASSERT(data_size);
  zx_vaddr_t address;
  zx_status_t status =
      vmar.map(kMapOptions, guard_below.get(), vmo.vmo, vmo.offset, data_size.get(), &address);
  if (status != ZX_OK) [[unlikely]] {
    return zx::error{status};
  }
  ZX_DEBUG_ASSERT(address == start_ + guard_below.get());
  vmo.offset += data_size.get();

  // From this point on, this GuardedPageBlock owns the mapping.  The VMAR will
  // be destroyed implicitly by unmapping its whole address range from above.
  // The VMAR can no longer be modified, so the guards cannot be filled in.
  size_ = vmar_size;
  vmar_ = allocate_from;
  destroy_on_error.cancel();

  return zx::ok(std::span{
      reinterpret_cast<std::byte*>(address),
      data_size.get(),
  });
}

}  // namespace LIBC_NAMESPACE_DECL
