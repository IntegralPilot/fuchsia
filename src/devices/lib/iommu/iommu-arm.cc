// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/iommu/iommu-arm.h"

#include <zircon/syscalls/iommu.h>

namespace iommu {

zx_status_t ArmIommuManager::Init(zx::unowned_resource iommu_resource) {
  zx_iommu_desc_stub_t stub;
  zx_status_t status =
      zx::iommu::create(*iommu_resource, ZX_IOMMU_TYPE_STUB, &stub, sizeof(stub), &stub_iommu_);
  if (status != ZX_OK) {
    return status;
  }

  return ZX_OK;
}

}  // namespace iommu
