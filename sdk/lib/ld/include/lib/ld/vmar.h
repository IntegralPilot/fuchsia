// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_VMAR_H_
#define LIB_LD_VMAR_H_

#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <zircon/assert.h>
#include <zircon/syscalls.h>

#include <utility>

namespace ld {

// Helper class to manage a VMAR reservation handle with strict ownership and RAII cleanup.
//
// Example usage:
//
//   zx_info_vmar_t info;
//   parent_vmar->get_info(ZX_INFO_VMAR, &info, sizeof(info), nullptr, nullptr);
//
//   ld::VmarReservation reservation;
//   // reserve_info specifies the bounds of the reservation VMAR.
//   if (auto res = reservation.Init(parent_vmar, info, reserve_info);
//       res.is_error()) {
//     return res;
//   }
//
//   // The reservation VMAR is cleanly destroyed automatically when reservation goes out of scope.
//
class VmarReservation {
 public:
  VmarReservation() = default;
  VmarReservation(VmarReservation&&) = default;

  // If we would take ownership of another vmar while this one has a valid vmar, we must explicitly
  // destroy it since the normal dtor of a zx::vmar won't destroy it.
  VmarReservation& operator=(VmarReservation&& other) {
    if (this != &other) {
      if (vmar_) {
        vmar_.destroy();
      }
      vmar_ = std::move(other.vmar_);
    }
    return *this;
  }

  ~VmarReservation() {
    if (vmar_) {
      vmar_.destroy();
    }
  }

  // Initialize the reservation by allocating a child VMAR in the parent
  // covering the specified bounds. The offset is computed from the absolute
  // bases of the parent and the requested child VMAR.
  zx::result<> Init(zx::unowned_vmar parent, zx_info_vmar_t parent_info,
                    zx_info_vmar_t reserve_info, zx_vm_option_t allocate_flags = 0) {
    uintptr_t child_addr;
    uintptr_t offset = reserve_info.base - parent_info.base;
    zx_status_t status = parent->allocate(ZX_VM_SPECIFIC | allocate_flags, offset, reserve_info.len,
                                          &vmar_, &child_addr);
    if (status != ZX_OK) {
      return zx::error{status};
    }
    ZX_DEBUG_ASSERT(child_addr == reserve_info.base);
    return zx::ok();
  }

  const zx::vmar& vmar() const { return vmar_; }

  explicit operator bool() const { return vmar_.is_valid(); }

 private:
  zx::vmar vmar_;
};

static_assert(std::movable<VmarReservation>);
static_assert(!std::copyable<VmarReservation>);

// Canonically calculate the bounds for the bottom half of a given primary VMAR.
// This supports standard full address spaces where base is low, as well as
// shared-process top-half VMAR configurations where base is high.
// The calculation rounds the half-size up to the nearest page boundary.
constexpr zx_info_vmar_t VmarBottomHalf(zx_info_vmar_t primary, size_t page_size) {
  zx_info_vmar_t result = primary;
  // Half of the primary length, page-aligned up.
  result.len = ((primary.len / 2) + page_size - 1) & -page_size;
  // Base remains the primary base.
  return result;
}

// Canonically calculate the bounds for the top half of a given primary VMAR,
// complementing VmarBottomHalf.
constexpr zx_info_vmar_t VmarTopHalf(zx_info_vmar_t primary, size_t page_size) {
  zx_info_vmar_t bottom = VmarBottomHalf(primary, page_size);
  zx_info_vmar_t result = primary;
  result.base = bottom.base + bottom.len;
  result.len = primary.len - bottom.len;
  return result;
}

}  // namespace ld

#endif  // LIB_LD_VMAR_H_
