// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_APPLE_AIC_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_APPLE_AIC_H_

#include <lib/boot-shim/devicetree.h>

namespace boot_shim {

class AppleDevicetreeAic3Item
    : public DevicetreeItemBase<AppleDevicetreeAic3Item, 1>,
      public SingleOptionalItem<zbi_dcfg_apple_aic3_t, ZBI_TYPE_KERNEL_DRIVER,
                                ZBI_KERNEL_DRIVER_APPLE_AIC3> {
 public:
  template <class Shim>
  void Init(const Shim& shim) {
    DevicetreeItemBase::Init(shim);
    mmio_observer_ = &shim.mmio_observer();
  }

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);

 private:
  const DevicetreeBootShimMmioObserver* mmio_observer_ = nullptr;
};

}  // namespace boot_shim

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_APPLE_AIC_H_
