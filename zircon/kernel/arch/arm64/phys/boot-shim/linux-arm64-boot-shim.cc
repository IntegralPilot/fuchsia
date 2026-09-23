// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>

#include <phys/boot-shim/devicetree.h>

#include <ktl/enforce.h>

namespace {

constexpr ShimOptions kOptions = {
    .generate_peripheral_ranges = true,
    .enable_mmu = true,
};

using Shim = Arm64StandardBootShimItems::Shim<boot_shim::DevicetreeBootShim>;

constexpr const char* kShimName = "linux-arm64-boot-shim";

}  // namespace

void PhysMain(void* flat_devicetree_blob, arch::EarlyTicks ticks) {
  BootShimHelper<Shim, kOptions> shim_helper(kShimName, flat_devicetree_blob);
  // Items will be initialized, and the matching process will begin.
  ZX_ASSERT(shim_helper.InitItems());
  // Items will be appended to the zbi, and we will boot into the kernel in that zbi.
  shim_helper.Boot();
}
