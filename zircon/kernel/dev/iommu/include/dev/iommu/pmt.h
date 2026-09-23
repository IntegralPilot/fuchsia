// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_PMT_H_
#define ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_PMT_H_

#include <lib/zx/result.h>
#include <stdint.h>
#include <sys/types.h>

#include <dev/iommu/common.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <vm/pinned_vm_object.h>

namespace iommu {
class Pmt;
}  // namespace iommu

extern "C" {
void cpp_pmt_recycle(iommu::Pmt* pmt);
void* cpp_pmt_get_ref_counted(const iommu::Pmt* pmt);
void cpp_pmt_release_pinned_memory(iommu::Pmt* pmt);
void cpp_pmt_on_dispatcher_zero_handles(iommu::Pmt* pmt);
uint64_t cpp_pmt_size(const iommu::Pmt* pmt);
zx_status_t cpp_pmt_query_address(iommu::Pmt* pmt, uint64_t query_offset, size_t query_size,
                                  ffi::Uninitialized<iommu::QueryAddressResult>* out_result);
}

namespace iommu {

class Pmt : public fbl::RefCounted<Pmt> {
 public:
  Pmt(const Pmt&) = delete;
  Pmt(Pmt&&) = delete;
  Pmt& operator=(const Pmt&) = delete;
  Pmt& operator=(Pmt&&) = delete;

  const PinnedVmObject& pinned_vmo() const { return pinned_vmo_; }

  // Queries the information of the pinned VMO. Attempts to find the (single)
  // continuous range from [|query_offset|, |query_offset| + |query_size|) in
  // the device's address space for the pinned VMO managed by this token.
  //
  // On success, returned range might be be less than, equal or greater than the
  // queried size. In the case of being less than, additional contiguous ranges
  // can be found by calling again with a new |query_offset|.
  //
  // Returns ZX_ERR_INVALID_ARGS if:
  //  |query_offset| is not aligned to kPageSize.
  // Returns ZX_ERR_OUT_OF_RANGE if:
  //  [|query_offset|, |query_offset| + |query_size|) is not a valid range in
  //  the mapping.
  //
  virtual zx::result<QueryAddressResult> QueryAddress(uint64_t query_offset, size_t query_size) = 0;

  // Unmap the memory managed by the PMT from its device's address space,
  // revoking device access in the process.  Then release the reference to the
  // memory held by the internal PinnedVmObject instance, potentially unpinning
  // the memory and returning it to the PMM in the process.
  virtual void ReleasePinnedMemory() = 0;

  // Called when the dispatcher which owns this PMT reaches the end of its
  // user-mode life.  Lets the driver level know in case something special needs
  // to be done (such as quarantining the memory).
  virtual void OnDispatcherZeroHandles() = 0;

  uint64_t size() const { return pinned_vmo_.size(); }
  uint64_t pages() const { return pinned_vmo_.size() / kPageSize; }

 protected:
  friend class fbl::RefPtr<Pmt>;  // Our RefPtr type is allowed to destroy us.
  friend void ::cpp_pmt_recycle(iommu::Pmt*);

  Pmt(PinnedVmObject pinned_vmo) : pinned_vmo_{ktl::move(pinned_vmo)} {}
  virtual ~Pmt() {
    // By the time we reach end of life, we should be able to assert that we
    // have released the reference to the VMO which was held by the internal
    // PinnedVmObject instance.
    DEBUG_ASSERT(pinned_vmo_.vmo() == nullptr);
  }

  PinnedVmObject pinned_vmo_;
};

}  // namespace iommu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_PMT_H_
