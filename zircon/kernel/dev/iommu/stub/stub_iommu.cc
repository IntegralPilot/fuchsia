// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <align.h>
#include <lib/page/size.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <new>

#include <dev/iommu/stub/stub.h>
#include <dev/iommu/stub/stub_bti.h>
#include <fbl/ref_ptr.h>
#include <ktl/algorithm.h>
#include <ktl/utility.h>
#include <vm/vm.h>
#include <vm/vm_object.h>

#include <ktl/enforce.h>

namespace iommu {

StubIommu::StubIommu() {}
StubIommu::~StubIommu() {}

zx::result<fbl::RefPtr<::iommu::Iommu>> StubIommu::Create() {
  fbl::AllocChecker ac;
  fbl::RefPtr<StubIommu> instance = fbl::AdoptRef<StubIommu>(new (&ac) StubIommu());

  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(ktl::move(instance));
}

zx::result<fbl::RefPtr<::iommu::Bti>> StubIommu::CreateBti(uint64_t bti_id) {
  return StubBti::Create(fbl::RefPtr{this}, bti_id);
}

}  // namespace iommu
