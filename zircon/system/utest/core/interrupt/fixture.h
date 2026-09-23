// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_INTERRUPT_FIXTURE_H_
#define ZIRCON_SYSTEM_UTEST_CORE_INTERRUPT_FIXTURE_H_

#include <lib/standalone-test/standalone.h>
#include <lib/zx/bti.h>
#include <lib/zx/iommu.h>
#include <lib/zx/msi.h>
#include <lib/zx/resource.h>
#include <lib/zx/thread.h>
#include <zircon/syscalls/iommu.h>

#include <zxtest/zxtest.h>

namespace {

class ResourceFixture : public zxtest::Test {
 public:
  void SetUp() override {
    irq_resource_ = standalone::GetIrqResource();

    zx::unowned_resource system_resource = standalone::GetSystemResource();

    zx_iommu_desc_stub_t desc = {};

    zx::result<zx::resource> get_iommu_resource =
        standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
    ASSERT_OK(get_iommu_resource.status_value());
    iommu_resource_ = std::move(get_iommu_resource.value());

    zx::result<zx::resource> get_msi_resource =
        standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_MSI_BASE);
    ASSERT_OK(get_msi_resource.status_value());
    msi_resource_ = std::move(get_msi_resource.value());

    ASSERT_OK(
        zx::iommu::create(iommu_resource_, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu_));
    ASSERT_OK(zx::bti::create(iommu_, 0, 0xdeadbeef, &bti_));
  }

  bool MsiTestsSupported() {
    zx::msi msi;
    return !(zx::msi::allocate(msi_resource_, 1, &msi) == ZX_ERR_NOT_SUPPORTED);
  }

 protected:
  zx::unowned_bti bti() { return bti_.borrow(); }
  zx::unowned_resource& irq_resource() { return irq_resource_; }
  zx::resource& iommu_resource() { return iommu_resource_; }
  zx::resource& msi_resource() { return msi_resource_; }
  zx::unowned_iommu iommu() { return iommu_.borrow(); }

 private:
  zx::unowned_resource irq_resource_;
  zx::resource iommu_resource_;
  zx::resource msi_resource_;
  zx::iommu iommu_;
  zx::bti bti_;
};

}  // namespace

#endif  // ZIRCON_SYSTEM_UTEST_CORE_INTERRUPT_FIXTURE_H_
