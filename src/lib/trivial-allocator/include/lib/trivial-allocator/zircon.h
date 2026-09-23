// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_TRIVIAL_ALLOCATOR_INCLUDE_LIB_TRIVIAL_ALLOCATOR_ZIRCON_H_
#define SRC_LIB_TRIVIAL_ALLOCATOR_INCLUDE_LIB_TRIVIAL_ALLOCATOR_ZIRCON_H_

#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <bit>
#include <cassert>
#include <tuple>
#include <utility>

namespace trivial_allocator {

// trivial_allocator::ZirconVmar holds a zx::unowned_vmar and uses it to meet
// the Memory API for trivial_allocator::PageAllocator.
//
// The optional first template parameter can give a lower bound on the
// page_size() used, which is the granularity of allocation.  It's the greater
// of this parameter and the system's runtime page size.  Setting a floor
// reduces the number of VMOs and VMARs created and the number of system calls
// made, while perhaps using more address space than is actually needed.
//
// The optional second template parameter can give additional flags for the
// zx::vmar::map operations, usually ZX_VM_MAP_RANGE.
template <size_t MinPageSize = 1, zx_vm_option_t ExtraMapFlags = 0>
  requires(std::has_single_bit(MinPageSize))
class ZirconVmar {
 public:
  // We use a sub-VMAR as a capability for each allocation so that once it's
  // been sealed, its protections cannot be changed again.  (It can still be
  // unmapped and something else mapped in the same location.)
  using Capability = zx::vmar;

  ZirconVmar() = default;
  ZirconVmar(const ZirconVmar&) = default;

  explicit ZirconVmar(const zx::vmar& vmar) : vmar_(vmar) { assert(vmar_->is_valid()); }

  ZirconVmar& operator=(const ZirconVmar&) = default;

  const zx::vmar& vmar() const { return *vmar_; }

  [[gnu::const]] size_t page_size() const {
    return std::max<size_t>(MinPageSize, zx_system_get_page_size());
  }

  [[nodiscard]] std::pair<void*, zx::vmar> Allocate(size_t size) {
    assert(vmar_->is_valid());
    zx::vmo vmo;
    zx_status_t status = zx::vmo::create(size, 0, &vmo);
    if constexpr (kAllocateFlags & ZX_VM_MAP_RANGE) {
      // ZX_VM_MAP_RANGE is only effective if the pages are already committed
      // in the VMO.  It's still cheaper to make an additional syscall to
      // commit them than to take the page fault later.
      if (status == ZX_OK) {
        status = vmo.op_range(ZX_VMO_OP_COMMIT, 0, size, nullptr, 0);
      }
    }
    if (status == ZX_OK) {
      zx::vmar sub_vmar;
      uintptr_t vmar_address;
      status = vmar_->allocate(kAllocateFlags, 0, size, &sub_vmar, &vmar_address);
      if (status == ZX_OK) {
        uintptr_t address;
        status = sub_vmar.map(kMapFlags, 0, vmo, 0, size, &address);
        if (status == ZX_OK) {
          assert(address >= vmar_address);
          return {reinterpret_cast<void*>(address), std::move(sub_vmar)};
        }
      }
    }
    return {};
  }

  void Deallocate(zx::vmar sub_vmar, void* ptr, size_t size) {
    // Destruction of the VMAR object cleans up the mapping.
    assert(sub_vmar.is_valid());
    [[maybe_unused]] zx_status_t status = sub_vmar.destroy();
    assert(status == ZX_OK);
  }

  void Release(zx::vmar sub_vmar, void* ptr, size_t size) { std::ignore = sub_vmar.release(); }

  // The VMAR handle is consumed here, so there will no longer be any way to
  // "unseal" this allocation (that is, change page protections on the memory).
  void Seal(zx::vmar sub_vmar, void* ptr, size_t size) {
    assert(sub_vmar.is_valid());
    [[maybe_unused]] zx_status_t status =
        sub_vmar.protect(ZX_VM_PERM_READ, reinterpret_cast<uintptr_t>(ptr), size);
    assert(status == ZX_OK);
  }

 private:
  static constexpr zx_vm_option_t kAllocateFlags = ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE;
  static constexpr zx_vm_option_t kMapFlags = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ExtraMapFlags;

  zx::unowned_vmar vmar_;
};

}  // namespace trivial_allocator

#endif  // SRC_LIB_TRIVIAL_ALLOCATOR_INCLUDE_LIB_TRIVIAL_ALLOCATOR_ZIRCON_H_
