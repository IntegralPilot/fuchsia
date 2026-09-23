// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_BOARD_DRIVERS_QEMU_RISCV64_PCIROOT_H_
#define SRC_DEVICES_BOARD_DRIVERS_QEMU_RISCV64_PCIROOT_H_

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/ddk/device.h>
#include <lib/pci/pciroot.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>
#include <zircon/status.h>

#include <ddktl/device.h>

namespace board_qemu_riscv64 {
class QemuRiscv64Pciroot;
using QemuRiscv64PcirootType = ddk::Device<QemuRiscv64Pciroot, ddk::GetProtocolable>;
class QemuRiscv64Pciroot : public QemuRiscv64PcirootType,
                           public PcirootBase,
                           public ddk::PcirootProtocol<QemuRiscv64Pciroot> {
 public:
  struct Context {
    zx::vmo ecam;
  };
  virtual ~QemuRiscv64Pciroot() = default;
  static zx::result<> Create(PciRootHost* root_host, QemuRiscv64Pciroot::Context ctx,
                             zx_device_t* parent, const char* name);
  // fuchsia.hardware.pciroot implementation fill-ins
  using PcirootBase::PcirootAllocateMsi;
  using PcirootBase::PcirootDriverShouldProxyConfig;
  using PcirootBase::PcirootGetAddressSpace;
  using PcirootBase::PcirootGetMsiHandle;
  using PcirootBase::PcirootReadConfig16;
  using PcirootBase::PcirootReadConfig32;
  using PcirootBase::PcirootReadConfig8;
  using PcirootBase::PcirootWriteConfig16;
  using PcirootBase::PcirootWriteConfig32;
  using PcirootBase::PcirootWriteConfig8;
  zx_status_t PcirootGetBti(uint32_t bdf, uint32_t index, zx::bti* bti);
  zx_status_t PcirootGetPciPlatformInfo(pci_platform_info_t* info);

  // DFv1 DDK methods
  void DdkRelease() { delete this; }
  zx_status_t DdkGetProtocol(uint32_t proto_id, void* out) {
    if (proto_id != ZX_PROTOCOL_PCIROOT) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    auto* proto = static_cast<ddk::AnyProtocol*>(out);
    proto->ops = &pciroot_protocol_ops_;
    proto->ctx = this;
    return ZX_OK;
  }

 private:
  QemuRiscv64Pciroot(PciRootHost* root_host, QemuRiscv64Pciroot::Context context,
                     zx_device_t* parent, const char* name)
      : QemuRiscv64PcirootType(parent), PcirootBase(root_host), context_(std::move(context)) {}
  zx::result<> CreateInterrupts();

  async_dispatcher_t* dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();
  Context context_;
  std::vector<pci_legacy_irq_t> interrupts_;
  std::vector<pci_irq_routing_entry_t> irq_routing_entries_;
};

}  // namespace board_qemu_riscv64

#endif  // SRC_DEVICES_BOARD_DRIVERS_QEMU_RISCV64_PCIROOT_H_
