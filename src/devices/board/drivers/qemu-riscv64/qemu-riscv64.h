// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_BOARD_DRIVERS_QEMU_RISCV64_QEMU_RISCV64_H_
#define SRC_DEVICES_BOARD_DRIVERS_QEMU_RISCV64_QEMU_RISCV64_H_

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <lib/pci/root_host.h>
#include <lib/zx/result.h>
#include <threads.h>

#include <ddktl/device.h>

namespace board_qemu_riscv64 {

// BTI IDs for our devices
enum {
  BTI_SYSMEM,
};

// From QEMU hw/riscv/virt.c's memmap[VIRT_RTC].
constexpr zx_paddr_t kGoldfishRtcMmioBase = 0x101000;
constexpr size_t kGoldfishRtcMmioSize = 0x1000;

class QemuRiscv64;
using QemuRiscv64Type = ddk::Device<QemuRiscv64, ddk::Initializable>;

class QemuRiscv64 : public QemuRiscv64Type {
 public:
  QemuRiscv64(zx_device_t* parent, fdf::ClientEnd<fuchsia_hardware_platform_bus::PlatformBus> pbus)
      : QemuRiscv64Type(parent),
        pbus_(std::move(pbus)),
        pci_root_host_(zx::unowned_resource(get_msi_resource(parent)),
                       zx::unowned_resource(get_mmio_resource(parent)),
                       zx::unowned_resource(get_ioport_resource(parent)),
                       PCI_ADDRESS_SPACE_MEMORY) {}

  static zx_status_t Create(void* ctx, zx_device_t* parent);
  void DdkInit(ddk::InitTxn txn);
  void DdkRelease() { delete this; }

 private:
  zx::result<> PcirootInit();
  zx::result<> RtcInit();
  void SysinfoInit();

  // TODO(https://fxbug.dev/42059490): Switch to fdf::SyncClient once it's supported.
  fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus> pbus_;
  PciRootHost pci_root_host_;
};

}  // namespace board_qemu_riscv64

#endif  // SRC_DEVICES_BOARD_DRIVERS_QEMU_RISCV64_QEMU_RISCV64_H_
