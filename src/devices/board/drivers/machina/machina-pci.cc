// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/hardware/pciroot/c/banjo.h>
#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <lib/pci/pciroot.h>
#include <lib/pci/root_host.h>
#include <lib/zx/bti.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/iommu.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <stdint.h>
#include <zircon/errors.h>
#include <zircon/syscalls/iommu.h>
#include <zircon/syscalls/types.h>
#include <zircon/types.h>

#include <array>
#include <memory>
#include <vector>

#include <ddktl/device.h>
#include <fbl/alloc_checker.h>

#include "machina.h"

namespace machina {

class MachinaPciroot;
using MachinaPcirootType = ddk::Device<MachinaPciroot, ddk::GetProtocolable>;

class MachinaPciroot : public MachinaPcirootType,
                       public PcirootBase,
                       public ddk::PcirootProtocol<MachinaPciroot> {
 public:
  struct Context {
    pci_platform_info_t info;
  };

  ~MachinaPciroot() override = default;

  static zx_status_t Create(PciRootHost* root_host, MachinaPciroot::Context ctx,
                            zx_device_t* parent, const char* name) {
    auto pciroot = std::make_unique<MachinaPciroot>(root_host, ctx, parent, name);
    if (zx::result<> result = pciroot->CreateInterrupts(); result.is_error()) {
      zxlogf(ERROR, "Failed to create legacy PCI interrupts: %s", result.status_string());
      return result.status_value();
    }

    zx_status_t status =
        pciroot->DdkAdd(ddk::DeviceAddArgs(name).set_proto_id(ZX_PROTOCOL_PCIROOT));
    if (status == ZX_OK) {
      // Driver Framework owns MachinaPciroot, object is intentionally leaked on success.
      [[maybe_unused]] auto ptr = pciroot.release();
    }
    return status;
  }

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

  zx_status_t PcirootGetBti(uint32_t bdf, uint32_t index, zx::bti* bti) {
    // Stub IOMMU: machina has no real IOMMU driver yet, so every bus-mastering
    // device shares a pass-through iommu keyed by BDF.
    zx::unowned_resource iommu_resource(get_iommu_resource(parent()));
    zx_iommu_desc_stub_t desc{};
    zx::iommu iommu;
    if (zx_status_t status =
            zx::iommu::create(*iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu);
        status != ZX_OK) {
      return status;
    }
    return zx::bti::create(iommu, /*options=*/0, bdf, bti);
  }

  zx_status_t PcirootGetPciPlatformInfo(pci_platform_info_t* info) {
    *info = context_.info;
    info->legacy_irqs_list = interrupts_.data();
    info->legacy_irqs_count = interrupts_.size();
    info->irq_routing_list = irq_routing_entries_.data();
    info->irq_routing_count = irq_routing_entries_.size();
    return ZX_OK;
  }

  // DDK implementations
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

  MachinaPciroot(PciRootHost* root_host, MachinaPciroot::Context ctx, zx_device_t* parent,
                 const char* name)
      : MachinaPcirootType(parent), PcirootBase(root_host), context_(ctx) {}

 private:
  zx::result<> CreateInterrupts() {
    zx::unowned_resource irq_resource(get_irq_resource(parent()));

    // 32 interrupts starting from PCIE_INT_BASE (32).
    for (uint32_t pin = 0; pin < 32; pin++) {
      const uint32_t vector = PCIE_INT_BASE + pin;
      zx::interrupt interrupt;
      if (zx_status_t status = zx::interrupt::create(*irq_resource, vector,
                                                     ZX_INTERRUPT_MODE_LEVEL_HIGH, &interrupt);
          status != ZX_OK) {
        zxlogf(ERROR, "Failed to create interrupt for vector %u: %s", vector,
               zx_status_get_string(status));
        return zx::error(status);
      }
      interrupts_[pin] = pci_legacy_irq_t{
          .interrupt = interrupt.release(),
          .vector = vector,
      };
    }

    // Each device dev_id swizzles to vector PCIE_INT_BASE + dev_id.
    for (uint8_t device_id = 0; device_id < 32; device_id++) {
      pci_irq_routing_entry_t entry = {
          .port_device_id = PCI_IRQ_ROUTING_NO_PARENT,
          .port_function_id = PCI_IRQ_ROUTING_NO_PARENT,
          .device_id = device_id,
      };
      for (uint32_t pin = 0; pin < 4; pin++) {
        entry.pins[pin] = static_cast<uint8_t>(PCIE_INT_BASE + device_id);
      }
      irq_routing_entries_[device_id] = entry;
    }

    return zx::ok();
  }

  Context context_;
  std::array<pci_legacy_irq_t, 32> interrupts_;
  std::array<pci_irq_routing_entry_t, 32> irq_routing_entries_;
};

zx_status_t machina_pci_init(zx_device_t* parent, machina_board_t* board) {
  board->pci_root_host = std::make_unique<PciRootHost>(
      zx::unowned_resource(get_msi_resource(parent)),
      zx::unowned_resource(get_mmio_resource(parent)),
      zx::unowned_resource(get_ioport_resource(parent)), PCI_ADDRESS_SPACE_MEMORY);

  zx_status_t status = board->pci_root_host->Mmio32().AddRegion(
      {.base = PCIE_MMIO_BASE_PHYS, .size = PCIE_MMIO_SIZE}, RegionAllocator::AllowOverlap::No);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to add MMIO region { %#lx - %#lx } to PCI root allocator: %s",
           PCIE_MMIO_BASE_PHYS, PCIE_MMIO_BASE_PHYS + PCIE_MMIO_SIZE, zx_status_get_string(status));
    return status;
  }

  McfgAllocation pci0_mcfg = {
      .address = PCIE_ECAM_BASE_PHYS,
      .pci_segment = 0,
      .start_bus_number = 0,
      .end_bus_number = 0,
  };

  board->pci_root_host->mcfgs().push_back(pci0_mcfg);

  // Create the userspace PCI root device
  std::array<char, 8> name = {"pci0"};
  MachinaPciroot::Context ctx = {};
  ctx.info.start_bus_num = pci0_mcfg.start_bus_number;
  ctx.info.end_bus_num = pci0_mcfg.end_bus_number;
  ctx.info.segment_group = pci0_mcfg.pci_segment;
  memcpy(ctx.info.name, name.data(), name.size());

  const size_t vmo_size = fbl::round_up<size_t>(PCIE_ECAM_SIZE, zx_system_get_page_size());
  zx::vmo ecam_vmo = {};
  status = zx::vmo::create_physical(*zx::unowned_resource(get_mmio_resource(parent)),
                                    PCIE_ECAM_BASE_PHYS, vmo_size, &ecam_vmo);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to create physical VMO for ECAM: %s", zx_status_get_string(status));
    return status;
  }

  ctx.info.cam = {.vmo = ecam_vmo.release(), .is_extended = true};
  status = MachinaPciroot::Create(board->pci_root_host.get(), ctx, parent, name.data());
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to create MachinaPciroot: %s", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

}  // namespace machina
