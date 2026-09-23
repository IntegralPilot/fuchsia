// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_IOMMU_H_
#define ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_IOMMU_H_

// #include <lib/zircon-internal/thread_annotations.h>
#include <sys/types.h>

// #include <fbl/intrusive_double_list.h>
#include <lib/zx/result.h>

#include <dev/iommu/bti.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

namespace iommu {

class Iommu;

}  // namespace iommu

extern "C" void cpp_iommu_recycle(iommu::Iommu* iommu);
extern "C" void* cpp_iommu_get_ref_counted(const iommu::Iommu* iommu);

namespace iommu {

class Bti;  // fwd decl, object declaration in iommu/bti.h

class Iommu : public fbl::RefCounted<Iommu> {
 public:
  // Creates a new Bus Transaction Initiator in this specific Iommu instance.
  // |bus_txn_id| encodes a packed form of the ID of the initiator indicated to
  // the IOMMU (along with a dev_vaddr_t) during a HW initiated transaction.
  // The precise encoding of the |bus_txn_id| depends on the specific IOMMU
  // hardware being used, or is an arbitrary placeholder in the case of the
  // StubIommu implementation.
  virtual zx::result<fbl::RefPtr<Bti>> CreateBti(uint64_t bus_txn_id) = 0;

 protected:
  friend class fbl::RefPtr<Iommu>;
  friend void ::cpp_iommu_recycle(iommu::Iommu*);
  virtual ~Iommu() {}
};

}  // namespace iommu
#endif  // ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_IOMMU_H_
