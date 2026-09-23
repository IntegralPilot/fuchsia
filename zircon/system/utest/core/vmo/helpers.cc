// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "helpers.h"

#include <lib/boot-options/boot-options.h>
#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <unistd.h>

#include <zxtest/zxtest.h>

namespace vmo_test {

zx::result<PhysVmo> GetTestPhysVmo(size_t size) {
  // We cannot create any physical VMOs without the mmio resource.
  zx::unowned_resource mmio_resource = maybe_standalone::GetMmioResource();
  if (!mmio_resource->is_valid()) {
    return zx::error_result(ZX_ERR_NOT_SUPPORTED);
  }

  // Fetch the address of the test reserved RAM region.  Even with the mmio
  // resource, we cannot use zx_vmo_create_physical to create a VMO which
  // points to RAM unless someone passed a kernel command line argument telling
  // the kernel to reserve a chunk of RAM for this purpose.
  //
  // If a chunk of RAM was reserved, the kernel will publish its size and
  // physical location in the boot options.  If we have access to the mmio
  // resource, it is because we are running in the standalone boot test.  The boot
  // options command line arguments should be available to us as a VMO.
  //
  // This is an all-or-nothing thing.  If we have the mmio resource, then we
  // should also have some RAM reserved for running these tests.  If we have
  // the mmio resource, but _don't_ have any reserved RAM, it should be
  // considered a test error.

  RamReservation ram;
  const BootOptions* boot_options = maybe_standalone::GetBootOptions();
  EXPECT_TRUE(boot_options->test_ram_reserve);
  ram = boot_options->test_ram_reserve.to_std().value();
  EXPECT_TRUE(ram.paddr);
  if (!ram.paddr) {
    return zx::error_result(ZX_ERR_NO_RESOURCES);
  }

  PhysVmo ret = {.addr = *ram.paddr.to_std(), .size = ram.size};
  if (size > 0) {
    if (size > ret.size) {
      return zx::error_result(ZX_ERR_INVALID_ARGS);
    }
    ret.size = size;
  }

  // Go ahead and create the VMO itself.
  zx_status_t res = zx::vmo::create_physical(*mmio_resource, ret.addr, ret.size, &ret.vmo);
  EXPECT_OK(res);
  if (res != ZX_OK) {
    return zx::error_result(res);
  }

  // The test VMO is, due to being a physical VMO, uncached by default, however this VMO is reused
  // by different tests and these tests may changed it to cached etc. To ensure that every test then
  // gets a clean version of the VMO we do a cache clean so that if the test that is now creating
  // this VMO uses in the default uncached state there is no cached data waiting to be written out
  // as a cache-bomb.
  uintptr_t vaddr;
  res = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ret.vmo, 0, ret.size,
                                   &vaddr);
  EXPECT_OK(res);
  if (res != ZX_OK) {
    return zx::error_result(res);
  }
  auto unmap =
      fit::defer([vaddr, size = ret.size]() { zx::vmar::root_self()->unmap(vaddr, size); });
  res = zx_cache_flush(reinterpret_cast<void*>(vaddr), ret.size, ZX_CACHE_FLUSH_DATA);
  EXPECT_OK(res);
  if (res != ZX_OK) {
    return zx::error_result(res);
  }

  return zx::ok(std::move(ret));
}

zx::bti CreateNamedBti(const zx::iommu& fake_iommu, uint32_t options, uint64_t bti_id,
                       const char* name) {
  zx::bti ret;
  EXPECT_OK(zx::bti::create(fake_iommu, options, bti_id, &ret));

  if (ret.is_valid()) {
    EXPECT_OK(ret.set_property(ZX_PROP_NAME, name, strlen(name)));
  }

  return ret;
}

}  // namespace vmo_test
