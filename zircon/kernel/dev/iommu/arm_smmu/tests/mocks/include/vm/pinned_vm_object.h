// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_VM_PINNED_VM_OBJECT_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_VM_PINNED_VM_OBJECT_H_

#include <lib/fit/function.h>
#include <stddef.h>
#include <sys/types.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <ktl/utility.h>

class VmObject : public fbl::RefCounted<VmObject> {
 public:
  virtual ~VmObject() = default;
  virtual zx_status_t LookupContiguous(size_t offset, size_t size, paddr_t* out_paddr) {
    if (lookup_func_) {
      return lookup_func_(offset, size, out_paddr);
    }
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Basic mock hook
  void SetLookupHook(fit::function<zx_status_t(size_t, size_t, paddr_t*)> func) {
    lookup_func_ = ktl::move(func);
  }

 private:
  fit::function<zx_status_t(size_t, size_t, paddr_t*)> lookup_func_;
};

class PinnedVmObject {
 public:
  PinnedVmObject() = default;
  PinnedVmObject(fbl::RefPtr<VmObject> vmo, size_t offset, size_t size)
      : vmo_(ktl::move(vmo)), offset_(offset), size_(size) {}

  PinnedVmObject(PinnedVmObject&&) = default;
  PinnedVmObject& operator=(PinnedVmObject&&) = default;

  const fbl::RefPtr<VmObject>& vmo() const { return vmo_; }
  size_t offset() const { return offset_; }
  size_t size() const { return size_; }

 private:
  fbl::RefPtr<VmObject> vmo_;
  size_t offset_ = 0;
  size_t size_ = 0;

  DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(PinnedVmObject);
};

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_VM_PINNED_VM_OBJECT_H_
