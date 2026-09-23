// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FZL_VMAR_MANAGER_H_
#define LIB_FZL_VMAR_MANAGER_H_

#include <lib/zx/result.h>
#include <lib/zx/vmar.h>

#include <fbl/macros.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

namespace fzl {

// VmarManager
//
// A small utility class which manages the lifecycle of a VMAR intended to be
// shared among a collection of users.  VmarManager will handle simple tasks such as
// automatically destroying the VMAR at end-of-life in addition to releasing the
// handle.
//
// Currently, the primary use case for a VmarManager is to be used to create a
// COMPACT sub-vmar in order to hold a number of VMO mappings while minimizing
// page table fragmentation..
//
// See fzl::VmoMapper.
class VmarManager : public fbl::RefCounted<VmarManager> {
 public:
  // Create a new VmarManager (creating the underlying VMAR object in the
  // process)
  //
  // size    : the size of the VMAR region to create.
  // parent  : the parent of this VMAR, or nullptr to use the root VMAR.
  // options : creation options to pass to vmar_allocate
  static fbl::RefPtr<VmarManager> Create(size_t size, fbl::RefPtr<VmarManager> parent = nullptr,
                                         zx_vm_option_t options = ZX_VM_COMPACT |
                                                                  ZX_VM_CAN_MAP_READ |
                                                                  ZX_VM_CAN_MAP_WRITE);

  // Create a new VmarManager that wraps an existing vmar, without making a child vmar inside
  // of it. This is meant to facilitate the use of a non-root vmars, that like a root vmar, are
  // guaranteed to never be destroyed while the process is running. An example of this is the
  // vmar provided to drivers by the driver framework.
  //
  // WARNING: This vmar must NOT be destroyed while this VmarManager, or any children of it,
  // are alive inside the process.
  //
  // vmar  : the VMAR to use
  static zx::result<fbl::RefPtr<VmarManager>> Use(const zx::unowned_vmar& vmar);

  const zx::vmar& vmar() const { return vmar_; }
  void* start() const { return start_; }
  uint64_t size() const { return size_; }
  const fbl::RefPtr<VmarManager>& parent() const { return parent_; }

 private:
  friend class fbl::RefPtr<VmarManager>;

  VmarManager() = default;
  ~VmarManager() {
    if (vmar_.is_valid() && !unowned_vmar_) {
      vmar_.destroy();
    }
  }

  // suppress default constructors
  DISALLOW_COPY_ASSIGN_AND_MOVE(VmarManager);

  zx::vmar vmar_;
  void* start_ = nullptr;
  uint64_t size_ = 0;
  fbl::RefPtr<VmarManager> parent_;
  bool unowned_vmar_ = false;
};

}  // namespace fzl

#endif  // LIB_FZL_VMAR_MANAGER_H_
